//! Réglages globaux de `%APPDATA%\Horizon\settings.toml` (modifiables à la main).
//!
//! Le fichier appartient à l'utilisateur : Horizon ne l'écrit que s'il n'existe pas, ou quand
//! la fenêtre « Réglages d'Horizon » enregistre (`save`, texte commenté de `write`). Un fichier
//! illisible (TOML invalide, encodage) est copié en `settings.toml.bak`, laissé tel quel, et
//! les réglages par défaut servent en attendant qu'il soit corrigé : il n'est jamais écrasé.

mod alerts;
mod discord;
mod features;
mod music;
mod write;

pub use alerts::AlertSettings;
pub use discord::DiscordSettings;
pub use features::FeatureSettings;
pub use music::{Backdrop, MusicSettings, Pulse};

use crate::night::NightSettings;
use crate::palette::PaletteSettings;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Section `[timer]` : durées proposées par le panneau du minuteur et pomodoro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimerSettings {
    /// Durées proposées, en minutes (6 au plus).
    pub presets: Vec<u32>,
    /// Pomodoro : minutes de travail.
    pub focus: u32,
    /// Pomodoro : minutes de pause.
    pub rest: u32,
}

impl TimerSettings {
    pub(crate) const MAX_PRESETS: usize = 6;
}

impl Default for TimerSettings {
    fn default() -> Self {
        TimerSettings { presets: vec![5, 15, 25, 50], focus: 25, rest: 5 }
    }
}

/// Section `[autohide]` : réapparition de la barre au-dessus d'un plein écran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutohideSettings {
    /// La souris arrêtée en haut de l'écran fait réapparaître la barre. Coupé : elle ne revient
    /// qu'à la fin du plein écran (jeux dont les menus touchent le haut de l'écran).
    pub reveal: bool,
    /// Hauteur de la zone, en haut de l'écran, où la souris fait réapparaître la barre
    /// (pixels logiques). Assez haute pour s'y arrêter quand un écran est au-dessus.
    pub reveal_px: u32,
    /// Temps de présence de la souris dans la zone avant la réapparition (millisecondes) :
    /// la traverser pour passer à l'écran du dessus ne fait rien apparaître.
    pub reveal_ms: u64,
}

impl AutohideSettings {
    /// Bornes de `reveal_px` et `reveal_ms` : au-delà, la barre deviendrait impossible à
    /// faire réapparaître ou apparaîtrait au moindre passage.
    pub(crate) const MAX_REVEAL_PX: u32 = 200;
    pub(crate) const MAX_REVEAL_MS: u64 = 5000;
}

impl Default for AutohideSettings {
    fn default() -> Self {
        AutohideSettings { reveal: true, reveal_px: 12, reveal_ms: 150 }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub night: NightSettings,
    pub autohide: AutohideSettings,
    pub palette: PaletteSettings,
    pub alerts: AlertSettings,
    pub timer: TimerSettings,
    pub features: FeatureSettings,
    pub music: MusicSettings,
    pub discord: DiscordSettings,
}

/// État du fichier, pour la page « État » et l'enregistrement.
#[derive(Debug, Clone, PartialEq)]
// Une valeur de plus par lecture du fichier : la taille de la variante ne compte pas.
#[allow(clippy::large_enum_variant)]
pub enum SettingsFile {
    /// Absent (créé à la première lecture).
    Missing,
    Valid(Settings),
    /// Présent mais illisible : la raison (jamais écrasé).
    Invalid(String),
}

impl Settings {
    /// Lit le TOML ; `None` si le texte n'est pas valide. Les valeurs sont bornées.
    pub fn parse(text: &str) -> Option<Settings> {
        match toml::from_str::<Settings>(text) {
            Ok(s) => Some(s.bounded()),
            Err(e) => {
                log::error!("settings.toml illisible ({e}) : réglages par défaut");
                None
            }
        }
    }

    /// Ramène chaque valeur numérique dans son domaine (opacité dans [0, 1]…).
    pub fn bounded(mut self) -> Settings {
        let o = self.night.opacity;
        self.night.opacity = if o.is_finite() { o.clamp(0.0, 1.0) } else { NightSettings::default().opacity };
        self.autohide.reveal_px = self.autohide.reveal_px.min(AutohideSettings::MAX_REVEAL_PX);
        self.autohide.reveal_ms = self.autohide.reveal_ms.min(AutohideSettings::MAX_REVEAL_MS);
        self.alerts = self.alerts.bounded();
        let minutes = |m: u32| m.clamp(crate::timer::MIN_MINUTES, crate::timer::MAX_MINUTES);
        let t = &mut self.timer;
        t.presets = t.presets.iter().map(|&m| minutes(m)).collect();
        t.presets.dedup();
        t.presets.truncate(TimerSettings::MAX_PRESETS);
        if t.presets.is_empty() {
            t.presets = TimerSettings::default().presets;
        }
        (t.focus, t.rest) = (minutes(t.focus), minutes(t.rest));
        self
    }

    /// Lit `path` sans rien créer ni copier : son état.
    pub fn inspect(path: &Path) -> std::io::Result<SettingsFile> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SettingsFile::Missing),
            Err(e) => return Err(e),
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(SettingsFile::Invalid("le fichier n'est pas en UTF-8".into()));
        };
        Ok(match toml::from_str::<Settings>(text) {
            Ok(s) => SettingsFile::Valid(s.bounded()),
            Err(e) => SettingsFile::Invalid(e.message().to_owned()),
        })
    }

    /// Enregistre ces réglages dans `path` (texte commenté, écriture atomique). Refusé si le
    /// fichier présent est illisible : il appartient à l'utilisateur, qui doit le corriger.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let SettingsFile::Invalid(why) = Settings::inspect(path)? {
            anyhow::bail!("settings.toml est illisible ({why}) : corrige-le ou supprime-le d'abord");
        }
        crate::config::write_atomic(path, self.clone().bounded().to_toml().as_bytes())?;
        Ok(())
    }

    /// Lit `path`. Absent → créé avec les valeurs par défaut (toutes les options visibles).
    /// Illisible (TOML, encodage) → copié en `.bak` (l'original reste en place), valeurs par
    /// défaut : résultat durable tant que le fichier ne change pas. Lecture impossible
    /// (verrouillé un instant…) → `Err` : passager, à ne pas garder en cache.
    pub fn read(path: &Path) -> std::io::Result<Settings> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let s = Settings::default();
                if let Err(e) = write_new(path, &s) {
                    log::warn!("création de {} : {e:#}", path.display());
                }
                return Ok(s);
            }
            Err(e) => return Err(e),
        };
        let parsed = std::str::from_utf8(&bytes).ok().and_then(Settings::parse);
        Ok(parsed.unwrap_or_else(|| {
            keep_aside(path, &bytes);
            Settings::default()
        }))
    }

    /// Comme [`Settings::read`], valeurs par défaut si la lecture est impossible.
    pub fn load(path: &Path) -> Settings {
        Settings::read(path).unwrap_or_else(|e| {
            log::error!("lecture de {} : {e} : réglages par défaut", path.display());
            Settings::default()
        })
    }
}

/// Crée le fichier seulement s'il n'existe toujours pas (jamais d'écrasement).
fn write_new(path: &Path, s: &Settings) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = s.to_toml();
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// Copie du fichier illisible en `settings.toml.bak` (refaite seulement si son contenu change).
fn keep_aside(path: &Path, bytes: &[u8]) {
    let bak = path.with_extension("toml.bak");
    if std::fs::read(&bak).is_ok_and(|b| b == bytes) {
        return;
    }
    match std::fs::write(&bak, bytes) {
        Ok(()) => log::error!("copie du fichier illisible dans {}", bak.display()),
        Err(e) => log::error!("copie de {} : {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn unreadable_file_is_a_transient_error() {
        let d = TestDir::new("settings-locked");
        let p = d.join("settings.toml");
        std::fs::create_dir_all(&p).unwrap();
        assert!(Settings::read(&p).is_err());
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn settings_defaults_and_partial_file() {
        assert_eq!(Settings::parse(""), Some(Settings::default()));
        let s = Settings::parse("[night]\nstart = \"23:30\"\n").unwrap();
        assert_eq!(s.night.start, "23:30");
        assert_eq!(s.night.end, "07:00");
        assert_eq!(s.autohide, AutohideSettings::default());
        assert_eq!(Settings::parse("pas = = du toml"), None);
    }

    #[test]
    fn palette_section_is_read() {
        assert_eq!(Settings::parse("").unwrap().palette, PaletteSettings::default());
        let s = Settings::parse("[palette]\nhotkey = \"Win+Shift+P\"\n").unwrap();
        assert_eq!(s.palette.hotkey, "Win+Shift+P");
    }

    #[test]
    fn autohide_section_is_read() {
        let s = Settings::parse("[autohide]\nreveal_px = 20\n").unwrap();
        assert_eq!(s.autohide.reveal_px, 20);
        assert_eq!(s.autohide.reveal_ms, 150);
        assert!(s.autohide.reveal, "réapparition gardée quand le fichier n'en dit rien");
        assert!(!Settings::parse("[autohide]\nreveal = false\n").unwrap().autohide.reveal);
    }

    #[test]
    fn numeric_values_are_bounded() {
        let s = Settings::parse("[night]\nopacity = 3.5\n[autohide]\nreveal_px = 90000\nreveal_ms = 999999\n").unwrap();
        assert_eq!(s.night.opacity, 1.0);
        assert_eq!(s.autohide.reveal_px, AutohideSettings::MAX_REVEAL_PX);
        assert_eq!(s.autohide.reveal_ms, AutohideSettings::MAX_REVEAL_MS);
        assert_eq!(Settings::parse("[night]\nopacity = -0.2\n").unwrap().night.opacity, 0.0);
        assert_eq!(Settings::parse("[night]\nopacity = nan\n").unwrap().night.opacity, NightSettings::default().opacity);
    }

    #[test]
    fn missing_file_is_created_with_every_option() {
        let d = TestDir::new("settings-missing");
        let p = d.join("settings.toml");
        assert_eq!(Settings::load(&p), Settings::default());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("[night]") && text.contains("[autohide]") && text.contains("[palette]"));
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn invalid_file_is_never_overwritten() {
        let d = TestDir::new("settings-invalid");
        let p = d.join("settings.toml");
        let mine = "# mes réglages\n[night]\nstart = \"23:00\"\nopacity = = 0.4\n";
        std::fs::write(&p, mine).unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), mine);
        assert_eq!(std::fs::read_to_string(d.join("settings.toml.bak")).unwrap(), mine);
    }

    #[test]
    fn non_utf8_file_is_kept_and_copied() {
        let d = TestDir::new("settings-bytes");
        let p = d.join("settings.toml");
        std::fs::write(&p, [0xff, 0xfe, 0x5b]).unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        assert_eq!(std::fs::read(&p).unwrap(), vec![0xff, 0xfe, 0x5b]);
        assert_eq!(std::fs::read(d.join("settings.toml.bak")).unwrap(), vec![0xff, 0xfe, 0x5b]);
    }

    #[test]
    fn valid_partial_file_is_left_untouched() {
        let d = TestDir::new("settings-partial");
        let p = d.join("settings.toml");
        let mine = "# commentaire gardé\n[night]\nstart = \"23:30\"\n";
        std::fs::write(&p, mine).unwrap();
        assert_eq!(Settings::load(&p).night.start, "23:30");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), mine);
        assert!(!d.join("settings.toml.bak").exists());
    }
}
