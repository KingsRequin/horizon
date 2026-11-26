//! Télécommande : abonnements des téléphones, commandes, état du média, pochette et volume
//! envoyés aux téléphones qui les regardent. Décisions pures : `horizon_core::phone::remote`.
//! Rien ne tourne tant qu'aucun téléphone ne regarde : les crochets appelés par le média et le
//! mixeur rendent aussitôt.

use crate::util::lock;
use base64::Engine;
use horizon_core::media::{MediaInfo, position_at, worth_sending};
use horizon_core::phone::proto::{FromPhone, ToPhone};
use horizon_core::phone::remote::{self, Command, Topic, art_key, media_message};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Pochette envoyée : côté en pixels et poids au plus. 47 Kio de JPEG font 64 172 caractères de
/// base64 : avec l'enveloppe JSON, la trame tient dans les 64 Kio du protocole.
const ART_SIDE: u32 = 300;
const ART_MAX: usize = 47 * 1024;

/// Ce que la télécommande demande au reste d'Horizon (en test : un faux).
pub trait Controls: Send + Sync {
    fn play_pause(&self);
    fn next(&self);
    fn previous(&self);
    fn seek_relative(&self, ms: i64);
    fn select(&self, app_id: &str);
    fn set_volume(&self, volume: f32);
    fn set_mute(&self, muted: bool);
    /// Lecture en cours du moment.
    fn media(&self) -> MediaNow;
    /// Volume général du moment ; `None` sans sortie.
    fn volume(&self) -> Option<(f32, bool)>;
    /// Le mixeur relit l'état tant que quelqu'un regarde le volume.
    fn watch_volume(&self, on: bool);
}

/// Lecture en cours et lectures entre lesquelles choisir (identifiant, nom).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaNow {
    pub info: Option<MediaInfo>,
    pub sessions: Vec<(String, String)>,
}

/// Ce que le média dit de la pochette à chaque relecture.
pub enum ArtChange<'a> {
    Unchanged,
    /// Plus de pochette.
    None,
    Bytes(&'a [u8]),
}

/// Pochette du morceau `key` : l'image reçue, et son JPEG en base64 une fois encodé.
struct Art {
    key: String,
    raw: Arc<Vec<u8>>,
    jpeg: Option<Option<String>>,
}

/// Dernier état envoyé : lecture, lectures, clé de pochette.
type Sent = (Option<MediaInfo>, Vec<(String, String)>, Option<String>);

/// Envoie un message à un téléphone (en vrai : par le serveur ; en test : dans une liste).
pub type Sender = Box<dyn Fn(&str, &ToPhone) + Send + Sync>;

/// Droits actuels d'un téléphone (en vrai : le registre). Lus au moment où ils servent, jamais
/// d'avance : un abonnement ne peut pas se réinstaller avec des droits qui viennent de changer.
pub type Rights = Box<dyn Fn(&str) -> Vec<String> + Send + Sync>;

#[derive(Default)]
struct Inner {
    /// Ce que chaque téléphone a demandé (`watch`), avant le tri par ses droits : rétabli quand
    /// un droit revient, l'appli ne le redemandant pas.
    requested: HashMap<String, Vec<String>>,
    /// Ce qu'il regarde vraiment : la demande triée par ses droits du moment.
    watch: HashMap<String, Vec<Topic>>,
    /// Le mixeur a été réveillé pour le volume (décidé sous le verrou : deux fils ne peuvent pas
    /// le réveiller ou l'endormir deux fois).
    volume_on: bool,
    sent: Option<Sent>,
    art: Option<Art>,
    volume: Option<(f32, bool)>,
}

impl Inner {
    fn watching(&self, topic: Topic) -> Vec<String> {
        let mut ids: Vec<String> = self.watch.iter().filter(|(_, t)| t.contains(&topic)).map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids
    }

    /// JPEG en base64 de la pochette du morceau `info`, encodé au premier besoin.
    fn art_for(&mut self, info: Option<&MediaInfo>) -> Option<(String, String)> {
        let key = info.map(|i| art_key(&i.app_id, &i.title, &i.artist))?;
        let art = self.art.as_mut().filter(|a| a.key == key)?;
        let jpeg = art.jpeg.get_or_insert_with(|| {
            let encoded = horizon_sys::art::jpeg_square(&art.raw, ART_SIDE, ART_MAX).map(|j| base64::engine::general_purpose::STANDARD.encode(j));
            if encoded.is_none() {
                log::debug!("téléphone : pochette illisible ou trop lourde, non envoyée");
            }
            encoded
        });
        jpeg.clone().map(|j| (key, j))
    }
}

/// Télécommande des téléphones : un objet pour tout le module.
pub struct Remote {
    controls: Box<dyn Controls>,
    send: Sender,
    rights: Rights,
    inner: Mutex<Inner>,
}

fn app_name(now: &MediaNow) -> String {
    let Some(info) = &now.info else { return String::new() };
    now.sessions.iter().find(|(id, _)| *id == info.app_id).map_or_else(|| info.app_id.clone(), |(_, name)| name.clone())
}

impl Remote {
    pub fn new(controls: Box<dyn Controls>, send: Sender, rights: Rights) -> Remote {
        Remote { controls, send, rights, inner: Mutex::default() }
    }

    /// Message d'un téléphone dont la session est ouverte ; vrai si c'en est un de la
    /// télécommande (accepté ou non).
    pub fn message(&self, phone_id: &str, msg: &FromPhone, now_ms: u64) -> bool {
        if let FromPhone::Watch { topics } = msg {
            self.watch(phone_id, topics, now_ms);
            return true;
        }
        if !matches!(msg, FromPhone::MediaCommand { .. } | FromPhone::MediaSeek { .. } | FromPhone::MediaSelect { .. } | FromPhone::VolumeSet { .. } | FromPhone::VolumeMute { .. }) {
            return false;
        }
        match remote::command(msg, &(self.rights)(phone_id)) {
            // Cible bornée au morceau ; un direct (durée inconnue) ne se déplace pas.
            Some(Command::SeekTo(_)) if self.controls.media().info.is_none_or(|i| i.duration_ms == 0) => {}
            Some(Command::PlayPause) => self.controls.play_pause(),
            Some(Command::Next) => self.controls.next(),
            Some(Command::Previous) => self.controls.previous(),
            Some(Command::SeekTo(target)) => {
                if let Some(info) = self.controls.media().info {
                    let target = target.min(info.duration_ms);
                    let delta = i64::try_from(target).unwrap_or(i64::MAX).saturating_sub(i64::try_from(position_at(&info, now_ms)).unwrap_or(i64::MAX));
                    self.controls.seek_relative(delta);
                }
            }
            Some(Command::Select(app_id)) => self.controls.select(&app_id),
            Some(Command::SetVolume(v)) => self.controls.set_volume(v),
            Some(Command::SetMute(m)) => self.controls.set_mute(m),
            None => log::debug!("téléphone : commande de {phone_id} refusée"),
        }
        true
    }

    /// Vrai si `phone_id` regarde quelque chose : sa session doit alors relayer sans attendre.
    pub fn watching(&self, phone_id: &str) -> bool {
        lock(&self.inner).watch.contains_key(phone_id)
    }

    /// Ce que `phone_id` demande désormais à regarder.
    fn watch(&self, phone_id: &str, requested: &[String], now_ms: u64) {
        lock(&self.inner).requested.insert(phone_id.to_owned(), requested.to_vec());
        self.apply(phone_id, now_ms);
    }

    /// Ce que `phone_id` regarde : sa demande, triée par ses droits du moment. Ce qu'il vient de
    /// commencer à regarder lui est envoyé tout de suite.
    fn apply(&self, phone_id: &str, now_ms: u64) {
        let added = {
            let mut inner = lock(&self.inner);
            // Droits lus sous le verrou : deux changements rapprochés (abonnement, droit retiré ou
            // rendu) ne peuvent pas s'appliquer dans le désordre.
            let requested = inner.requested.get(phone_id).cloned().unwrap_or_default();
            let topics = remote::topics(&requested, &(self.rights)(phone_id));
            let before = inner.watch.get(phone_id).cloned().unwrap_or_default();
            let added: Vec<Topic> = topics.iter().copied().filter(|t| !before.contains(t)).collect();
            if topics.is_empty() {
                inner.watch.remove(phone_id);
            } else {
                inner.watch.insert(phone_id.to_owned(), topics);
            }
            added
        };
        self.sync_volume();
        if added.contains(&Topic::Media) {
            let now = self.controls.media();
            let mut inner = lock(&self.inner);
            let art = inner.art_for(now.info.as_ref());
            let state = media_message(now.info.as_ref(), &app_name(&now), &now.sessions, now_ms, art.is_some());
            inner.sent = Some((now.info.clone(), now.sessions.clone(), art.as_ref().map(|(k, _)| k.clone())));
            (self.send)(phone_id, &state);
            if let Some((art_key, jpeg)) = art {
                (self.send)(phone_id, &ToPhone::MediaArt { art_key, jpeg });
            }
        }
        if added.contains(&Topic::Volume)
            && let Some((volume, muted)) = self.controls.volume()
        {
            lock(&self.inner).volume = Some((volume, muted));
            (self.send)(phone_id, &ToPhone::VolumeState { volume, muted });
        }
    }

    /// Réveille ou rendort le mixeur quand le nombre de téléphones qui regardent le volume passe
    /// par zéro. Le passage est décidé sous le verrou ; l'appel au mixeur se fait hors du verrou
    /// (le mixeur rappelle `volume_changed`).
    fn sync_volume(&self) {
        let change = {
            let mut inner = lock(&self.inner);
            let on = !inner.watching(Topic::Volume).is_empty();
            (on != inner.volume_on).then(|| {
                inner.volume_on = on;
                on
            })
        };
        if let Some(on) = change {
            self.controls.watch_volume(on);
        }
    }

    /// La session de `phone_id` est finie : il ne regarde plus rien.
    pub fn closed(&self, phone_id: &str) {
        {
            let mut inner = lock(&self.inner);
            inner.requested.remove(phone_id);
            inner.watch.remove(phone_id);
        }
        self.sync_volume();
    }

    /// Les droits de `phone_id` ont changé (déjà enregistrés) : il le sait, et regarde ce qu'il
    /// a demandé et peut désormais voir (un droit rendu rétablit son abonnement).
    pub fn rights_changed(&self, phone_id: &str, granted: &[String], now_ms: u64) {
        (self.send)(phone_id, &ToPhone::Rights { granted: granted.to_vec() });
        self.apply(phone_id, now_ms);
    }

    /// La lecture a changé (fil du média). La pochette reçue est gardée ; l'état ne part que
    /// vers les téléphones qui regardent, et seulement s'il a changé.
    pub fn media_changed(&self, now: &MediaNow, art: ArtChange, now_ms: u64) {
        let mut inner = lock(&self.inner);
        match art {
            ArtChange::Unchanged => {}
            ArtChange::None => inner.art = None,
            ArtChange::Bytes(bytes) => {
                inner.art = now.info.as_ref().map(|i| Art { key: art_key(&i.app_id, &i.title, &i.artist), raw: Arc::new(bytes.to_vec()), jpeg: None });
            }
        }
        let watchers = inner.watching(Topic::Media);
        if watchers.is_empty() {
            return;
        }
        let art = inner.art_for(now.info.as_ref());
        let key = art.as_ref().map(|(k, _)| k.clone());
        let changed = inner.sent.as_ref().is_none_or(|(info, sessions, sent_key)| {
            worth_sending(info.as_ref(), now.info.as_ref(), now_ms) || *sessions != now.sessions || *sent_key != key
        });
        if !changed {
            return;
        }
        let new_art = inner.sent.as_ref().is_none_or(|(_, _, sent_key)| *sent_key != key);
        inner.sent = Some((now.info.clone(), now.sessions.clone(), key));
        drop(inner);
        let state = media_message(now.info.as_ref(), &app_name(now), &now.sessions, now_ms, art.is_some());
        for id in &watchers {
            (self.send)(id, &state);
        }
        if let Some((art_key, jpeg)) = art.filter(|_| new_art) {
            let msg = ToPhone::MediaArt { art_key, jpeg };
            for id in &watchers {
                (self.send)(id, &msg);
            }
        }
    }

    /// Le volume général a changé (fil du mixeur) : il part vers ceux qui le regardent.
    pub fn volume_changed(&self, volume: Option<(f32, bool)>) {
        let watchers = {
            let mut inner = lock(&self.inner);
            if inner.volume == volume {
                return;
            }
            inner.volume = volume;
            inner.watching(Topic::Volume)
        };
        if let Some((volume, muted)) = volume {
            for id in &watchers {
                (self.send)(id, &ToPhone::VolumeState { volume, muted });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::media::MediaInfo;
    use horizon_core::phone::proto::{FromPhone, ToPhone};
    use std::sync::{Arc, Mutex};

    /// Commandes reçues et état servi, pour les tests.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        media: Mutex<MediaNow>,
        volume: Mutex<Option<(f32, bool)>>,
        watched: Mutex<Vec<bool>>,
        /// Droits de tous les téléphones du test, lus par la télécommande au moment voulu.
        granted: Mutex<Vec<String>>,
    }

    impl Controls for Arc<Fake> {
        fn play_pause(&self) {
            self.calls.lock().unwrap().push("play_pause".into());
        }
        fn next(&self) {
            self.calls.lock().unwrap().push("next".into());
        }
        fn previous(&self) {
            self.calls.lock().unwrap().push("previous".into());
        }
        fn seek_relative(&self, ms: i64) {
            self.calls.lock().unwrap().push(format!("seek {ms}"));
        }
        fn select(&self, app_id: &str) {
            self.calls.lock().unwrap().push(format!("select {app_id}"));
        }
        fn set_volume(&self, v: f32) {
            self.calls.lock().unwrap().push(format!("volume {v}"));
        }
        fn set_mute(&self, m: bool) {
            self.calls.lock().unwrap().push(format!("mute {m}"));
        }
        fn media(&self) -> MediaNow {
            self.media.lock().unwrap().clone()
        }
        fn volume(&self) -> Option<(f32, bool)> {
            *self.volume.lock().unwrap()
        }
        fn watch_volume(&self, on: bool) {
            self.watched.lock().unwrap().push(on);
        }
    }

    type Sent = Arc<Mutex<Vec<(String, ToPhone)>>>;

    fn remote() -> (Remote, Arc<Fake>, Sent) {
        let fake = Arc::new(Fake::default());
        *fake.granted.lock().unwrap() = rights();
        let sent: Sent = Arc::default();
        let s = sent.clone();
        let f = fake.clone();
        let r = Remote::new(
            Box::new(fake.clone()),
            Box::new(move |id: &str, msg: &ToPhone| s.lock().unwrap().push((id.to_owned(), msg.clone()))),
            Box::new(move |_: &str| f.granted.lock().unwrap().clone()),
        );
        (r, fake, sent)
    }

    fn rights() -> Vec<String> {
        ["battery", "media", "sound"].iter().map(|s| (*s).to_owned()).collect()
    }

    fn watch(topics: &[&str]) -> FromPhone {
        FromPhone::Watch { topics: topics.iter().map(|s| (*s).to_owned()).collect() }
    }

    fn song(title: &str, playing: bool, position_ms: u64) -> MediaNow {
        let info = MediaInfo { app_id: "Spotify.exe".into(), title: title.into(), artist: "SPLATA".into(), playing, position_ms, duration_ms: 233_000, timeline_at_ms: 1_000 };
        MediaNow { info: Some(info), sessions: vec![("Spotify.exe".into(), "Spotify".into())] }
    }

    fn types(sent: &Sent) -> Vec<String> {
        sent.lock().unwrap().iter().map(|(id, m)| format!("{id} {}", serde_json::from_str::<serde_json::Value>(&m.to_json()).unwrap()["type"].as_str().unwrap())).collect()
    }

    /// Une petite image PNG valable.
    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 60, 20])).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn a_command_runs_only_with_the_media_right() {
        let (r, fake, _) = remote();
        *fake.granted.lock().unwrap() = vec!["battery".into()];
        assert!(r.message("tel-1", &FromPhone::MediaCommand { action: "next".into() }, 0));
        assert!(fake.calls.lock().unwrap().is_empty(), "sans droit : rien");
        *fake.granted.lock().unwrap() = rights();
        r.message("tel-1", &FromPhone::MediaCommand { action: "next".into() }, 0);
        r.message("tel-1", &FromPhone::VolumeSet { volume: 3.0 }, 0);
        r.message("tel-1", &FromPhone::VolumeMute { muted: true }, 0);
        r.message("tel-1", &FromPhone::MediaSelect { app_id: "Chrome".into() }, 0);
        assert_eq!(*fake.calls.lock().unwrap(), vec!["next", "volume 1", "mute true", "select Chrome"]);
        assert!(!r.message("tel-1", &FromPhone::Battery { devices: vec![] }, 0), "pas un message de la télécommande");
    }

    #[test]
    fn seek_to_becomes_a_relative_seek_from_the_current_position() {
        let (r, fake, _) = remote();
        // Lecture : 60 s lues à 1 s ; à 9 s, on est à 68 s.
        *fake.media.lock().unwrap() = song("FROGZ", true, 60_000);
        r.message("tel-1", &FromPhone::MediaSeek { position_ms: 30_000 }, 9_000);
        assert_eq!(*fake.calls.lock().unwrap(), vec!["seek -38000"]);
        *fake.media.lock().unwrap() = MediaNow::default();
        r.message("tel-1", &FromPhone::MediaSeek { position_ms: 30_000 }, 9_000);
        assert_eq!(fake.calls.lock().unwrap().len(), 1, "rien ne joue : pas de saut");
    }

    #[test]
    fn watching_media_sends_the_state_at_once_then_only_changes() {
        let (r, fake, sent) = remote();
        *fake.media.lock().unwrap() = song("FROGZ", true, 60_000);
        r.message("tel-1", &watch(&["media"]), 2_000);
        assert_eq!(types(&sent), vec!["tel-1 media.state"]);
        r.media_changed(&song("FROGZ", true, 61_000), ArtChange::Unchanged, 2_000);
        assert_eq!(sent.lock().unwrap().len(), 1, "même morceau, position cohérente : rien");
        r.media_changed(&song("FROGZ", false, 61_000), ArtChange::Unchanged, 2_000);
        r.media_changed(&song("AUTRE", true, 0), ArtChange::Unchanged, 2_000);
        assert_eq!(types(&sent), vec!["tel-1 media.state", "tel-1 media.state", "tel-1 media.state"]);
        let (r2, _, sent2) = remote();
        r2.media_changed(&song("FROGZ", true, 0), ArtChange::Unchanged, 0);
        assert!(sent2.lock().unwrap().is_empty(), "personne ne regarde : rien");
    }

    #[test]
    fn the_volume_is_watched_while_one_phone_looks_at_it() {
        let (r, fake, sent) = remote();
        *fake.volume.lock().unwrap() = Some((0.5, false));
        r.message("tel-1", &watch(&["media", "volume"]), 0);
        r.message("tel-2", &watch(&["volume"]), 0);
        assert_eq!(*fake.watched.lock().unwrap(), vec![true], "le mixeur n'est réveillé qu'une fois");
        assert!(types(&sent).contains(&"tel-2 volume.state".to_owned()), "le volume du moment part à l'abonnement");
        r.volume_changed(Some((0.6, false)));
        r.volume_changed(Some((0.6, false)));
        assert_eq!(types(&sent).iter().filter(|t| t.ends_with("volume.state")).count(), 4, "deux à l'abonnement, deux au changement, rien pour une répétition");
        r.message("tel-1", &watch(&["media"]), 0);
        assert_eq!(*fake.watched.lock().unwrap(), vec![true]);
        r.closed("tel-2");
        assert_eq!(*fake.watched.lock().unwrap(), vec![true, false], "plus personne : le mixeur se rendort");
    }

    #[test]
    fn a_right_withdrawn_reaches_the_phone_and_its_watch_stops() {
        let (r, fake, sent) = remote();
        r.message("tel-1", &watch(&["media", "volume"]), 0);
        let only_battery: Vec<String> = vec!["battery".into()];
        *fake.granted.lock().unwrap() = only_battery.clone();
        r.rights_changed("tel-1", &only_battery, 0);
        assert_eq!(sent.lock().unwrap().last().unwrap().1, ToPhone::Rights { granted: only_battery });
        assert_eq!(*fake.watched.lock().unwrap(), vec![true, false]);
        sent.lock().unwrap().clear();
        r.media_changed(&song("FROGZ", true, 0), ArtChange::Unchanged, 0);
        assert!(sent.lock().unwrap().is_empty(), "plus abonné");
    }

    #[test]
    fn the_cover_is_sent_once_per_track() {
        let (r, fake, sent) = remote();
        *fake.media.lock().unwrap() = song("FROGZ", true, 0);
        r.message("tel-1", &watch(&["media"]), 0);
        let art = png();
        r.media_changed(&song("FROGZ", true, 0), ArtChange::Bytes(&art), 0);
        let t = types(&sent);
        assert_eq!(t, vec!["tel-1 media.state", "tel-1 media.state", "tel-1 media.art"], "la clé de pochette change l'état, puis la pochette suit");
        // Une copie : un emprunt de la liste garderait son verrou jusqu'à la fin du test.
        let msgs: Vec<ToPhone> = sent.lock().unwrap().iter().map(|(_, m)| m.clone()).collect();
        let ToPhone::MediaState { media: Some(m), .. } = &msgs[1] else { panic!() };
        let ToPhone::MediaArt { art_key, jpeg } = &msgs[2] else { panic!() };
        assert_eq!(m.art_key.as_deref(), Some(art_key.as_str()));
        assert!(jpeg.starts_with("/9j/"), "JPEG en base64");
        r.media_changed(&song("FROGZ", true, 0), ArtChange::Unchanged, 0);
        assert_eq!(sent.lock().unwrap().len(), 3, "rien de neuf");
        r.message("tel-2", &watch(&["media"]), 0);
        assert_eq!(types(&sent)[3..], ["tel-2 media.state".to_owned(), "tel-2 media.art".to_owned()], "un nouvel abonné reçoit la pochette");
        r.media_changed(&song("AUTRE", true, 0), ArtChange::None, 0);
        let last = sent.lock().unwrap().last().unwrap().1.clone();
        let ToPhone::MediaState { media: Some(m), .. } = last else { panic!() };
        assert_eq!(m.art_key, None, "autre morceau sans pochette");
    }

    #[test]
    fn a_watch_reads_the_rights_of_the_moment() {
        // Pas de droits lus d'avance par l'appelant : un abonnement qui arriverait juste après un
        // retrait ne peut pas se réinstaller avec les droits d'avant.
        let (r, fake, sent) = remote();
        *fake.granted.lock().unwrap() = vec!["battery".into()];
        r.message("tel-1", &watch(&["media", "volume"]), 0);
        assert!(sent.lock().unwrap().is_empty());
        assert!(fake.watched.lock().unwrap().is_empty());
        r.media_changed(&song("FROGZ", true, 0), ArtChange::Unchanged, 0);
        assert!(sent.lock().unwrap().is_empty(), "pas abonné");
    }

    #[test]
    fn a_right_given_back_brings_the_watch_back() {
        // L'appli ne renvoie pas `watch` quand ses droits reviennent : le PC garde ce qu'elle a
        // demandé et le rétablit lui-même.
        let (r, fake, sent) = remote();
        *fake.media.lock().unwrap() = song("FROGZ", true, 0);
        r.message("tel-1", &watch(&["media"]), 0);
        let only_battery: Vec<String> = vec!["battery".into()];
        *fake.granted.lock().unwrap() = only_battery.clone();
        r.rights_changed("tel-1", &only_battery, 0);
        sent.lock().unwrap().clear();
        *fake.granted.lock().unwrap() = rights();
        r.rights_changed("tel-1", &rights(), 0);
        assert_eq!(types(&sent), vec!["tel-1 rights", "tel-1 media.state"], "l'état du moment repart");
        r.media_changed(&song("AUTRE", true, 0), ArtChange::Unchanged, 0);
        assert_eq!(types(&sent).last().map(String::as_str), Some("tel-1 media.state"), "et les changements suivent");
    }

    #[test]
    fn a_seek_stays_inside_the_track() {
        let (r, fake, _) = remote();
        // 60 s lues à 1 s, morceau de 233 s : à 1 s, une cible démesurée s'arrête à la fin.
        *fake.media.lock().unwrap() = song("FROGZ", true, 60_000);
        r.message("tel-1", &FromPhone::MediaSeek { position_ms: u64::MAX }, 1_000);
        assert_eq!(*fake.calls.lock().unwrap(), vec!["seek 173000"]);
        let mut live = song("RADIO", true, 60_000);
        live.info.as_mut().unwrap().duration_ms = 0;
        *fake.media.lock().unwrap() = live;
        r.message("tel-1", &FromPhone::MediaSeek { position_ms: 10_000 }, 1_000);
        assert_eq!(fake.calls.lock().unwrap().len(), 1, "un direct ne se déplace pas");
    }

    #[test]
    fn the_cover_fits_in_one_frame() {
        // Une trame de plus de 64 Kio ferme la connexion (docs/PHONE.md) : le JPEG en base64 et
        // l'enveloppe JSON (type, clé) doivent y tenir.
        assert!(ART_MAX.div_ceil(3) * 4 + 128 <= 64 * 1024, "{} octets de base64", ART_MAX.div_ceil(3) * 4);
    }

    #[test]
    fn a_closed_session_stops_watching() {
        let (r, _, sent) = remote();
        r.message("tel-1", &watch(&["media"]), 0);
        r.closed("tel-1");
        sent.lock().unwrap().clear();
        r.media_changed(&song("FROGZ", true, 0), ArtChange::Unchanged, 0);
        assert!(sent.lock().unwrap().is_empty());
    }
}
