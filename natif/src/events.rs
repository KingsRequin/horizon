//! Événements du cœur envoyés aux fenêtres (mêmes noms que `app/src-tauri/src/events.rs` ; ceux
//! d'un widget seront dans son module). Les autres arrivent avec leurs modules.

/// Couleur d'accent des barres (`[r, g, b]` ou `null` = orange d'origine).
pub const EV_ACCENT: &str = "horizon://accent";
/// Mode nuit.
pub const EV_NIGHT: &str = "horizon://night";
/// Disposition d'une barre à jour, envoyée à sa fenêtre.
pub const EV_LAYOUT: &str = "horizon://layout";
/// Sortie de veille (données en ligne à relire).
pub const EV_RESUME: &str = "horizon://resume";
/// État audio (sorties, micros, volumes des applis) pour les panneaux (`mixer.rs`).
pub const EV_MIXER: &str = "horizon://mixer";
/// « Ne pas déranger » (`dnd.rs`).
pub const EV_DND: &str = "horizon://dnd";
/// Minuteur et pomodoro (`timer.rs`).
pub const EV_TIMER: &str = "horizon://timer";
/// Alerte de l'îlot dynamique (`alerts.rs`).
pub const EV_ALERT: &str = "horizon://alert";
/// Intégrations en échec (`status.rs`).
pub const EV_HEALTH: &str = "horizon://health";
/// Volume ou média à afficher dans la barre principale (`osd.rs`).
pub const EV_OSD: &str = "horizon://osd";
/// Réglages modifiés (`options.rs`).
pub const EV_OPTIONS: &str = "horizon://options";
/// Mode édition d'une barre demandé (`true`) ou terminé, envoyé à sa fenêtre (`edit.rs`).
pub const EV_EDIT: &str = "horizon://edit";
/// Catalogue de la palette reconstruit : la recherche en cours est relancée (`palette`).
pub const EV_PALETTE_ENTRIES: &str = "horizon://palette-entries";
/// Fenêtre de réglages ouverte (`true`) ou fermée (`settings_window`).
pub const EV_SETTINGS_WINDOW: &str = "horizon://settings-window";
/// Profils (liste, actif, activé par une appli) : `profiles.rs`.
pub const EV_PROFILES: &str = "horizon://profiles";
