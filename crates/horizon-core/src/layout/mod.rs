//! Disposition des barres : quel widget, dans quelle zone, sur quel écran.
//!
//! `defaults` : disposition de départ ; `migrations` : ajouts aux dispositions anciennes ;
//! `profiles` : profils de disposition ; `monitors` : écrans branchés ; `undo` : annulation.

mod defaults;
mod migrations;
mod monitors;
mod profiles;
mod undo;

pub(crate) use defaults::role_of;
pub use profiles::{Profile, ProfileFollower};
pub use undo::UndoStack;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version du format de `layout.json`.
pub(crate) const LAYOUT_VERSION: u32 = 1;

/// Hauteur de la bande du haut d'une barre (zones gauche, centre, droite), en pixels logiques :
/// c'est aussi la hauteur d'une barre normale.
pub const BAND_HEIGHT: u32 = 40;
/// Hauteur maximale d'une barre (pixels logiques) ; sous la bande, la zone des tuiles.
pub const MAX_BAR_HEIGHT: u32 = 900;

/// Hauteur ramenée entre la bande seule et le maximum.
pub fn clamp_height(h: u32) -> u32 {
    h.clamp(BAND_HEIGHT, MAX_BAR_HEIGHT)
}

/// A = îlots flottants, C = sombre graphique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarStyle {
    Islands,
    Graphic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    Left,
    Center,
    Right,
    /// Sous la bande du haut d'une barre haute : chaque widget y montre son panneau.
    Tiles,
}

impl Zone {
    pub const ALL: [Zone; 4] = [Zone::Left, Zone::Center, Zone::Right, Zone::Tiles];
}

/// Genre d'un widget : l'identifiant de son dossier dans l'interface (`app/src/widgets/<id>/`),
/// enregistré tel quel dans `layout.json`. Un genre inconnu (widget retiré) est gardé tel quel :
/// la disposition ne perd rien et l'interface l'ignore. Le nom affiché est dans le manifeste du
/// widget (`widget.json`), pas ici.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetKind(&'static str);

impl WidgetKind {
    /// Genres dont la logique pure a besoin (disposition de départ, migrations, alertes…).
    pub const MEDIA: WidgetKind = WidgetKind("media");
    pub const SHORTCUTS: WidgetKind = WidgetKind("shortcuts");
    pub const NOTIFICATIONS: WidgetKind = WidgetKind("notifications");
    pub const SYSTEM: WidgetKind = WidgetKind("system");
    pub const MIC_KRISP: WidgetKind = WidgetKind("mic_krisp");
    pub const CLOCK: WidgetKind = WidgetKind("clock");
    pub const CLAUDE_SESSIONS: WidgetKind = WidgetKind("claude_sessions");
    pub const CLAUDE_QUOTA: WidgetKind = WidgetKind("claude_quota");
    pub const SUGGESTIONS: WidgetKind = WidgetKind("suggestions");
    pub const HEADSET: WidgetKind = WidgetKind("headset");
    pub const LIGHTING: WidgetKind = WidgetKind("lighting");
    pub const WEATHER: WidgetKind = WidgetKind("weather");
    pub const CALENDAR: WidgetKind = WidgetKind("calendar");
    pub const BATTERIES: WidgetKind = WidgetKind("batteries");
    pub const DISCORD_VOICE: WidgetKind = WidgetKind("discord_voice");
    pub const SERVERS: WidgetKind = WidgetKind("servers");
    pub const WINDOWS: WidgetKind = WidgetKind("windows");
    pub const ESSAIM: WidgetKind = WidgetKind("essaim");

    /// Genre d'identifiant `id` (texte gardé une fois pour toute la durée du programme).
    pub fn new(id: &str) -> WidgetKind {
        WidgetKind(intern(id))
    }

    /// Identifiant (nom du dossier du widget).
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for WidgetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for WidgetKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for WidgetKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let id = String::deserialize(d)?;
        if id.is_empty() {
            return Err(serde::de::Error::custom("genre de widget vide"));
        }
        Ok(WidgetKind::new(&id))
    }
}

/// Texte `'static` pour `id` : chaque identifiant distinct n'est alloué qu'une fois (il y en a
/// une poignée : un par dossier de widget).
fn intern(id: &str) -> &'static str {
    use std::sync::Mutex;
    static IDS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut ids = IDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = ids.iter().find(|k| **k == id) {
        return known;
    }
    let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
    ids.push(leaked);
    leaked
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetInstance {
    pub id: String,
    pub kind: WidgetKind,
    #[serde(default)]
    pub settings: Value,
}

/// Rôle d'un écran dans la disposition de départ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarRole {
    Main,
    Top,
    Portrait,
    Side,
}

/// Ce que la souris peut faire sur une barre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interaction {
    /// Cliquable : panneaux au survol, actions au clic, molette = volume.
    #[default]
    Click,
    /// Survol seul : les panneaux s'ouvrent au survol, les clics ne font rien.
    Hover,
    /// Information seule : rien ne réagit à la souris (hors mode édition et clic droit).
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawBar")]
pub struct BarLayout {
    pub monitor_id: String,
    pub monitor_pos: (i32, i32),
    pub role: BarRole,
    pub style: BarStyle,
    pub interaction: Interaction,
    /// La barre se cache quand une fenêtre plein écran occupe son écran.
    pub autohide: bool,
    /// Hauteur en pixels logiques (`BAND_HEIGHT` = barre normale, sans tuiles).
    pub height: u32,
    pub left: Vec<WidgetInstance>,
    pub center: Vec<WidgetInstance>,
    pub right: Vec<WidgetInstance>,
    /// Tuiles sous la bande, montrées seulement si la barre est plus haute que la bande
    /// (gardées sinon, pour quand elle le redevient).
    pub tiles: Vec<WidgetInstance>,
}

/// Barre telle qu'enregistrée, formats anciens compris : `interactive` (booléen, avant le mode
/// « Survol seul ») devient `interaction` ; `autohide` absent vaut vrai ; `height` absente (barre
/// d'avant les tuiles) vaut la bande seule, une hauteur hors bornes y est ramenée.
#[derive(Deserialize)]
struct RawBar {
    monitor_id: String,
    monitor_pos: (i32, i32),
    role: BarRole,
    style: BarStyle,
    #[serde(default)]
    interaction: Option<Interaction>,
    #[serde(default)]
    interactive: Option<bool>,
    #[serde(default)]
    autohide: Option<bool>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    left: Vec<WidgetInstance>,
    #[serde(default)]
    center: Vec<WidgetInstance>,
    #[serde(default)]
    right: Vec<WidgetInstance>,
    #[serde(default)]
    tiles: Vec<WidgetInstance>,
}

impl From<RawBar> for BarLayout {
    fn from(r: RawBar) -> BarLayout {
        let interaction = r.interaction.unwrap_or(match r.interactive {
            Some(false) => Interaction::Info,
            _ => Interaction::Click,
        });
        BarLayout {
            monitor_id: r.monitor_id,
            monitor_pos: r.monitor_pos,
            role: r.role,
            style: r.style,
            interaction,
            autohide: r.autohide.unwrap_or(true),
            height: clamp_height(r.height.unwrap_or(BAND_HEIGHT)),
            left: r.left,
            center: r.center,
            right: r.right,
            tiles: r.tiles,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub version: u32,
    /// Barres du profil actif.
    pub bars: Vec<BarLayout>,
    /// Migrations déjà appliquées : un ajout fait une fois n'est jamais refait, même si
    /// l'utilisateur retire ensuite le widget.
    #[serde(default)]
    pub migrations: Vec<String>,
    /// Tous les profils, actif compris (au moins un après `ensure_profiles`).
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub active_profile: String,
    /// Profil choisi par l'utilisateur, repris quand l'appli qui a activé le profil actuel
    /// quitte le premier plan ; `None` = le profil actif est celui choisi par l'utilisateur.
    #[serde(default)]
    pub auto_from: Option<String>,
}

/// Identifiant unique d'instance de widget.
pub(crate) fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("w{nanos:x}{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

pub(super) fn widget(kind: WidgetKind, settings: Value) -> WidgetInstance {
    WidgetInstance { id: new_id(), kind, settings }
}

/// Options d'une barre à changer (fenêtre « Réglages d'Horizon », menu de la barre) ;
/// `None` = inchangée.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BarOptionsPatch {
    pub style: Option<BarStyle>,
    pub interaction: Option<Interaction>,
    pub autohide: Option<bool>,
    pub height: Option<u32>,
}

impl BarLayout {
    /// Les clics de la souris agissent sur cette barre.
    pub fn clickable(&self) -> bool {
        self.interaction == Interaction::Click
    }

    /// La barre est plus haute que sa bande : la zone des tuiles est affichée.
    pub fn has_tiles_area(&self) -> bool {
        self.height > BAND_HEIGHT
    }

    pub(crate) fn zone(&self, zone: Zone) -> &[WidgetInstance] {
        match zone {
            Zone::Left => &self.left,
            Zone::Center => &self.center,
            Zone::Right => &self.right,
            Zone::Tiles => &self.tiles,
        }
    }

    pub(crate) fn zone_mut(&mut self, zone: Zone) -> &mut Vec<WidgetInstance> {
        match zone {
            Zone::Left => &mut self.left,
            Zone::Center => &mut self.center,
            Zone::Right => &mut self.right,
            Zone::Tiles => &mut self.tiles,
        }
    }

    /// Tous les widgets de la barre, tuiles comprises (même cachées par une barre normale).
    pub fn widgets(&self) -> impl Iterator<Item = &WidgetInstance> {
        Zone::ALL.into_iter().flat_map(|z| self.zone(z))
    }

    pub fn widgets_mut(&mut self) -> impl Iterator<Item = &mut WidgetInstance> {
        self.left.iter_mut().chain(self.center.iter_mut()).chain(self.right.iter_mut()).chain(self.tiles.iter_mut())
    }

    /// Widgets montrés en tuile (aucun sur une barre normale).
    pub fn shown_tiles(&self) -> &[WidgetInstance] {
        if self.has_tiles_area() { &self.tiles } else { &[] }
    }
}

impl Layout {
    /// Widgets des barres du profil actif.
    pub fn widgets(&self) -> impl Iterator<Item = &WidgetInstance> {
        self.bars.iter().flat_map(BarLayout::widgets)
    }

    /// Widgets d'un genre, sur les barres du profil actif.
    pub fn widgets_of(&self, kind: WidgetKind) -> impl Iterator<Item = &WidgetInstance> {
        self.widgets().filter(move |w| w.kind == kind)
    }

    /// Premier widget d'un genre (celui dont les réglages valent pour tous, comme les options
    /// de « Ne pas déranger » du widget Notifications).
    pub fn first_of(&self, kind: WidgetKind) -> Option<&WidgetInstance> {
        self.widgets_of(kind).next()
    }

    /// Widget du profil actif, par identifiant.
    pub fn widget(&self, id: &str) -> Option<&WidgetInstance> {
        self.widgets().find(|w| w.id == id)
    }

    /// Retire de toutes les barres, profils inactifs compris, les widgets de ces genres
    /// (édition sans les applis personnelles) ; `true` si quelque chose a été retiré.
    pub fn remove_kinds(&mut self, kinds: &[&str]) -> bool {
        let mut removed = false;
        for bar in self.bars.iter_mut().chain(self.profiles.iter_mut().flat_map(|p| p.bars.iter_mut())) {
            for zone in [&mut bar.left, &mut bar.center, &mut bar.right, &mut bar.tiles] {
                let before = zone.len();
                zone.retain(|w| !kinds.contains(&w.kind.as_str()));
                removed |= zone.len() != before;
            }
        }
        removed
    }

    /// Tous les widgets, profils inactifs compris, modifiables.
    fn all_widgets_mut(&mut self) -> impl Iterator<Item = &mut WidgetInstance> {
        self.bars
            .iter_mut()
            .chain(self.profiles.iter_mut().flat_map(|p| p.bars.iter_mut()))
            .flat_map(BarLayout::widgets_mut)
    }

    fn bar_index(&self, monitor_id: &str) -> anyhow::Result<usize> {
        self.bars
            .iter()
            .position(|b| b.monitor_id == monitor_id)
            .ok_or_else(|| anyhow::anyhow!("écran inconnu : {monitor_id}"))
    }

    pub(super) fn find(&self, widget_id: &str) -> Option<(usize, Zone, usize)> {
        self.bars.iter().enumerate().find_map(|(b, bar)| {
            Zone::ALL
                .into_iter()
                .find_map(|z| bar.zone(z).iter().position(|w| w.id == widget_id).map(|i| (b, z, i)))
        })
    }

    pub fn set_style(&mut self, monitor_id: &str, style: BarStyle) -> anyhow::Result<()> {
        let b = self.bar_index(monitor_id)?;
        self.bars[b].style = style;
        Ok(())
    }

    /// Options d'une barre ; une hauteur hors bornes est ramenée entre la bande seule et
    /// `MAX_BAR_HEIGHT`.
    pub fn set_bar_options(&mut self, monitor_id: &str, patch: BarOptionsPatch) -> anyhow::Result<()> {
        let i = self.bar_index(monitor_id)?;
        let b = &mut self.bars[i];
        if let Some(s) = patch.style {
            b.style = s;
        }
        if let Some(i) = patch.interaction {
            b.interaction = i;
        }
        if let Some(a) = patch.autohide {
            b.autohide = a;
        }
        if let Some(h) = patch.height {
            b.height = clamp_height(h);
        }
        Ok(())
    }

    pub fn set_widget_settings(&mut self, widget_id: &str, settings: Value) -> anyhow::Result<()> {
        let w = self
            .widget_mut(widget_id)
            .ok_or_else(|| anyhow::anyhow!("widget inconnu : {widget_id}"))?;
        let previous = std::mem::replace(&mut w.settings, settings.clone());
        let kind = w.kind;
        // Ville de la météo, adresses de l'agenda : un réglage partagé. Les autres exemplaires
        // (autre écran, autre profil) qui n'étaient pas encore réglés ou qui avaient le même
        // réglage le suivent ; un exemplaire réglé autrement garde le sien.
        if matches!(kind, WidgetKind::WEATHER | WidgetKind::CALENDAR) {
            self.all_widgets_mut()
                .filter(|w| w.kind == kind && w.id != widget_id && (w.settings.is_null() || w.settings == previous))
                .for_each(|w| w.settings = settings.clone());
        }
        Ok(())
    }

    /// Instance d'un widget, modifiable (réglages).
    pub(crate) fn widget_mut(&mut self, widget_id: &str) -> Option<&mut WidgetInstance> {
        let (b, z, i) = self.find(widget_id)?;
        Some(&mut self.bars[b].zone_mut(z)[i])
    }

    pub fn move_widget(
        &mut self,
        widget_id: &str,
        to_monitor_id: &str,
        zone: Zone,
        index: usize,
    ) -> anyhow::Result<()> {
        let target = self.bar_index(to_monitor_id)?;
        let w = self.remove_widget(widget_id)?;
        let v = self.bars[target].zone_mut(zone);
        let i = index.min(v.len());
        v.insert(i, w);
        Ok(())
    }

    /// Envoie un widget sur la barre d'un autre écran, à la même place (même zone, même rang,
    /// borné à la fin de la zone). Une tuile envoyée sur une barre sans zone de tuiles va au
    /// bout de sa zone droite, où elle reste visible.
    pub fn move_to_monitor(&mut self, widget_id: &str, to_monitor_id: &str) -> anyhow::Result<()> {
        let (_, zone, index) = self.find(widget_id).ok_or_else(|| anyhow::anyhow!("widget inconnu : {widget_id}"))?;
        let target = &self.bars[self.bar_index(to_monitor_id)?];
        let (zone, index) =
            if zone == Zone::Tiles && !target.has_tiles_area() { (Zone::Right, usize::MAX) } else { (zone, index) };
        self.move_widget(widget_id, to_monitor_id, zone, index)
    }


    pub fn add_widget(
        &mut self,
        monitor_id: &str,
        zone: Zone,
        kind: WidgetKind,
    ) -> anyhow::Result<String> {
        let b = self.bar_index(monitor_id)?;
        let w = widget(kind, Value::Null);
        let id = w.id.clone();
        self.bars[b].zone_mut(zone).push(w);
        Ok(id)
    }

    pub fn remove_widget(&mut self, widget_id: &str) -> anyhow::Result<WidgetInstance> {
        let (b, z, i) = self
            .find(widget_id)
            .ok_or_else(|| anyhow::anyhow!("widget inconnu : {widget_id}"))?;
        Ok(self.bars[b].zone_mut(z).remove(i))
    }
}

#[cfg(test)]
pub(crate) mod tests;
