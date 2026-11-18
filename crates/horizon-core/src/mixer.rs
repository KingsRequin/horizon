//! Panneau Audio : périphériques (sorties, micros), volume par appli, favoris épinglés et
//! appariement de la lecture en cours (SMTC) avec la session audio de son appli (WASAPI).

use crate::apps::exe_key;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    Output,
    Input,
}

/// Périphérique audio actif.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    /// Identifiant d'endpoint Windows, stable d'un démarrage à l'autre.
    pub id: String,
    pub name: String,
    pub flow: Flow,
    /// Épinglé en tête de liste par l'utilisateur.
    pub pinned: bool,
}

/// Volume (0–1) et coupure d'un périphérique ou d'une appli.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Level {
    pub volume: f32,
    pub muted: bool,
}

/// Appli qui a une session audio sur la sortie par défaut (sessions regroupées par exe).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppAudio {
    /// Nom de l'exe en minuscules (`vivaldi.exe`) : toutes ses sessions sont réglées ensemble.
    pub key: String,
    pub name: String,
    /// Chemin de l'exe (icône).
    pub exe: String,
    #[serde(flatten)]
    pub level: Level,
    /// Une de ses sessions joue en ce moment.
    pub active: bool,
    /// AUMID connus de l'appli (raccourci du menu Démarrer, paquet MSIX), pour la relier à la
    /// lecture en cours.
    pub aumids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct MixerSnapshot {
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    pub default_output: Option<String>,
    pub default_input: Option<String>,
    /// Volume général (sortie par défaut) ; `None` sans sortie.
    pub master: Option<Level>,
    pub apps: Vec<AppAudio>,
}

/// Ordre d'affichage : favoris d'abord (dans l'ordre où ils ont été épinglés), puis les autres
/// par nom. Marque `pinned`.
pub fn sort_devices(devices: &mut [Device], pins: &[String]) {
    for d in devices.iter_mut() {
        d.pinned = pins.contains(&d.id);
    }
    let rank = |d: &Device| pins.iter().position(|p| *p == d.id).unwrap_or(usize::MAX);
    devices.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

/// Périphérique qui suit `current` (« sortie suivante ») : parmi les favoris s'il y en a au
/// moins deux, sinon parmi tous, dans l'ordre d'affichage ; on revient au premier après le
/// dernier. `None` s'il n'y a pas d'autre choix.
pub fn next_device<'a>(devices: &'a [Device], current: Option<&str>) -> Option<&'a Device> {
    let pinned: Vec<&Device> = devices.iter().filter(|d| d.pinned).collect();
    let pool: Vec<&Device> = if pinned.len() >= 2 { pinned } else { devices.iter().collect() };
    let at = pool.iter().position(|d| Some(d.id.as_str()) == current);
    let next = match at {
        Some(i) => pool[(i + 1) % pool.len()],
        None => *pool.first()?,
    };
    (Some(next.id.as_str()) != current).then_some(next)
}

/// Applis : celles qui jouent d'abord, puis par nom.
pub fn sort_apps(apps: &mut [AppAudio]) {
    apps.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

/// Favoris enregistrés (`%APPDATA%\Horizon\audio.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pins {
    #[serde(default)]
    pub pinned: Vec<String>,
}

impl Pins {
    /// Épingle (à la fin) ou désépingle ; renvoie vrai si la liste a changé.
    pub fn set(&mut self, id: &str, pinned: bool) -> bool {
        let has = self.pinned.iter().any(|p| p == id);
        match (pinned, has) {
            (true, false) => self.pinned.push(id.to_owned()),
            (false, true) => self.pinned.retain(|p| p != id),
            _ => return false,
        }
        true
    }
}

fn alnum(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Appli dont la session audio porte la lecture en cours `smtc_id` (identifiant SMTC : AUMID
/// comme `com.github.th-ch.youtube-music`, `Vivaldi.XYZ`, `…_8wekyb3d8bbwe!App`, ou nom d'exe).
///
/// Dans l'ordre : AUMID connu de l'appli ; nom d'exe ; sinon nom de l'exe contenu dans
/// l'identifiant (4 lettres au moins, le plus long gagne, ex æquo = pas de réponse). `None`
/// quand rien ne correspond sûrement : mieux vaut pas de curseur qu'un curseur sur la
/// mauvaise appli.
pub fn media_app<'a>(smtc_id: &str, apps: &'a [AppAudio]) -> Option<&'a AppAudio> {
    if smtc_id.is_empty() {
        return None;
    }
    if let Some(a) = apps.iter().find(|a| a.aumids.iter().any(|m| m.eq_ignore_ascii_case(smtc_id))) {
        return Some(a);
    }
    if smtc_id.to_ascii_lowercase().ends_with(".exe") {
        let key = exe_key(smtc_id);
        return apps.iter().find(|a| a.key == key);
    }
    let id = alnum(smtc_id);
    let mut best: Option<(&AppAudio, usize)> = None;
    let mut tie = false;
    for a in apps {
        let stem = alnum(a.key.strip_suffix(".exe").unwrap_or(&a.key));
        if stem.len() < 4 || !id.contains(&stem) {
            continue;
        }
        match best {
            Some((_, n)) if n > stem.len() => {}
            Some((_, n)) if n == stem.len() => tie = true,
            _ => {
                best = Some((a, stem.len()));
                tie = false;
            }
        }
    }
    if tie { None } else { best.map(|(a, _)| a) }
}

/// Fabricant et produit USB d'un identifiant d'instance d'appareil (`USB\VID_046D&PID_0AF7&MI_00\…`),
/// pour relier une sortie audio au récepteur HID du même casque.
pub fn usb_ids(instance_id: &str) -> Option<(u16, u16)> {
    let up = instance_id.to_ascii_uppercase();
    let hex = |tag: &str| -> Option<u16> {
        let rest = &up[up.find(tag)? + tag.len()..];
        u16::from_str_radix(rest.get(..4)?, 16).ok()
    };
    Some((hex("VID_")?, hex("PID_")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_ids_of_an_instance_id() {
        assert_eq!(usb_ids(r"USB\VID_046D&PID_0AF7&MI_00\8&20B174B6&0&0000"), Some((0x046D, 0x0AF7)));
        assert_eq!(usb_ids(r"{1}.usb\vid_046d&pid_0af7&mi_00"), Some((0x046D, 0x0AF7)));
        assert_eq!(usb_ids(r"HDAUDIO\FUNC_01&VEN_10EC&DEV_0897"), None);
        assert_eq!(usb_ids(r"USB\VID_04"), None);
    }
    use crate::apps::display_name;

    fn dev(id: &str, name: &str) -> Device {
        Device { id: id.into(), name: name.into(), flow: Flow::Output, pinned: false }
    }

    fn app(key: &str, aumids: &[&str]) -> AppAudio {
        AppAudio {
            key: key.into(),
            name: display_name(key, None),
            exe: format!(r"C:\x\{key}"),
            level: Level { volume: 1.0, muted: false },
            active: false,
            aumids: aumids.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn sortie_suivante() {
        let mut d = vec![dev("a", "A"), dev("b", "B"), dev("c", "C")];
        assert_eq!(next_device(&d, Some("a")).unwrap().id, "b");
        assert_eq!(next_device(&d, Some("c")).unwrap().id, "a");
        assert_eq!(next_device(&d, None).unwrap().id, "a");
        assert_eq!(next_device(&d, Some("inconnu")).unwrap().id, "a");
        // Deux favoris : on ne tourne qu'entre eux.
        d[0].pinned = true;
        d[2].pinned = true;
        assert_eq!(next_device(&d, Some("a")).unwrap().id, "c");
        assert_eq!(next_device(&d, Some("b")).unwrap().id, "a");
        // Un seul favori : tous comptent.
        d[2].pinned = false;
        assert_eq!(next_device(&d, Some("a")).unwrap().id, "b");
        assert_eq!(next_device(&d[..1], Some("a")), None);
        assert_eq!(next_device(&[], None), None);
    }

    #[test]
    fn favoris_en_tete_dans_l_ordre_d_epinglage() {
        let mut d = vec![dev("a", "Haut-parleurs"), dev("b", "CABLE Input"), dev("c", "PRO X 2"), dev("d", "écran")];
        sort_devices(&mut d, &["c".into(), "a".into()]);
        let ids: Vec<_> = d.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b", "d"]);
        assert!(d[0].pinned && d[1].pinned && !d[2].pinned);
    }

    #[test]
    fn favori_absent_ignore() {
        let mut d = vec![dev("b", "B"), dev("a", "A")];
        sort_devices(&mut d, &["zz".into()]);
        assert_eq!(d[0].id, "a");
        assert!(d.iter().all(|x| !x.pinned));
    }

    #[test]
    fn applis_qui_jouent_d_abord() {
        let mut a = vec![app("zed.exe", &[]), app("alpha.exe", &[]), app("mid.exe", &[])];
        a[0].active = true;
        sort_apps(&mut a);
        let keys: Vec<_> = a.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["zed.exe", "alpha.exe", "mid.exe"]);
    }

    #[test]
    fn epingler_et_desepingler() {
        let mut p = Pins::default();
        assert!(p.set("a", true));
        assert!(!p.set("a", true));
        assert!(p.set("b", true));
        assert_eq!(p.pinned, ["a", "b"]);
        assert!(p.set("a", false));
        assert!(!p.set("a", false));
        assert_eq!(p.pinned, ["b"]);
        let back: Pins = serde_json::from_str("{}").unwrap();
        assert!(back.pinned.is_empty());
    }

    #[test]
    fn appariement_par_aumid_connu() {
        let apps = [app("youtube music.exe", &["com.github.th-ch.youtube-music"]), app("firefox.exe", &["308046B0AF4A39CB"])];
        assert_eq!(media_app("com.github.th-ch.youtube-music", &apps).unwrap().key, "youtube music.exe");
        assert_eq!(media_app("308046b0af4a39cb", &apps).unwrap().key, "firefox.exe");
    }

    #[test]
    fn appariement_par_nom_d_exe() {
        let apps = [app("spotify.exe", &[]), app("vlc.exe", &[])];
        assert_eq!(media_app("Spotify.exe", &apps).unwrap().key, "spotify.exe");
        assert_eq!(media_app(r"C:\Program Files\VideoLAN\VLC\vlc.exe", &apps).unwrap().key, "vlc.exe");
        // Un nom d'exe sans session : rien (pas d'appariement approché).
        assert!(media_app("mpv.exe", &apps).is_none());
    }

    #[test]
    fn appariement_approche_navigateurs_et_paquets() {
        let apps = [app("vivaldi.exe", &[]), app("discord.exe", &[]), app("spotify.exe", &[])];
        assert_eq!(media_app("Vivaldi.UXZQYFRS3MBEXOZ7KC3EG6NFKE", &apps).unwrap().key, "vivaldi.exe");
        assert_eq!(media_app("com.squirrel.Discord.Discord", &apps).unwrap().key, "discord.exe");
        assert_eq!(media_app("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", &apps).unwrap().key, "spotify.exe");
        assert_eq!(media_app("com.github.th-ch.youtube-music", &[app("youtube music.exe", &[])]).unwrap().key, "youtube music.exe");
    }

    #[test]
    fn pas_d_appariement_douteux() {
        // Trop court pour être sûr, aucune appli, identifiant vide, ex æquo.
        assert!(media_app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic", &[app("mu.exe", &[])]).is_none());
        assert!(media_app("Chrome", &[]).is_none());
        assert!(media_app("", &[app("vivaldi.exe", &[])]).is_none());
        let twins = [app("abcd.exe", &[]), app("wxyz.exe", &[])];
        assert!(media_app("abcd.wxyz", &twins).is_none());
        // Le plus long l'emporte.
        let nested = [app("music.exe", &[]), app("youtubemusic.exe", &[])];
        assert_eq!(media_app("com.youtubemusic", &nested).unwrap().key, "youtubemusic.exe");
    }
}
