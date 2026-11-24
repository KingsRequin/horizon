//! Catalogue des widgets : un dossier `ui/widgets/<id>/` par widget, découvert à la compilation
//! (`build.rs`). Son `widget.json` est le même que celui de la version Tauri
//! (`app/src/widgets/<id>/widget.json`). Mode d'emploi : `natif/README.md`.
//!
//! Un widget porté a aussi son module ici (service repris de `app/src-tauri/src/widgets/<id>`,
//! liaison à l'interface) et son [`Backend`] dans [`BACKENDS`]. Son panneau surgissant
//! (`ui/widgets/<id>/panel.slint`, trouvé par `ui/build.rs` : [`PANELS`]) est relié par
//! [`Backend::panel`].

pub mod calendar;
pub mod claude_sessions;
pub mod clock;
pub mod discord_voice;
pub mod headset;
pub mod media;
pub mod mic_krisp;
pub mod notifications;
pub mod phone;
pub mod shortcuts;
pub mod suggestions;
pub mod system;
pub mod weather;
pub mod windows;

use crate::host::{AppHandle, Manager};
use crate::ui::BarWindow;
use crate::ui::panel_hosts::Host;
use horizon_core::layout::{BarLayout, Interaction, WidgetInstance};

/// Contenu de `widget.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest {
    pub id: &'static str,
    /// Nom affiché (édition partagée : `shareLabel` s'il y en a un).
    pub label: &'static str,
    /// Icône Phosphor (nom de `icons/phosphor/<nom>.svg`).
    pub icon: &'static str,
    /// Barre trop chargée : rang de masquage, 1 = masqué le premier. `None` = jamais masqué.
    pub squeeze: Option<u64>,
    /// Reste cliquable sur une barre « Survol seul ».
    pub clickable_on_hover: bool,
    /// Grandit avec son nombre d'éléments : limité quand la barre manque de place, il montre
    /// « +N » et le reste au survol. Sa plus petite largeur (« +N » seul, px logiques).
    pub grows: Option<u16>,
}

include!(concat!(env!("OUT_DIR"), "/widgets.rs"));

pub use horizon_ui::PANELS;

/// Manifeste du widget `id` ; `None` pour un genre inconnu (dossier retiré : la disposition le
/// garde, rien n'est affiché) et pour le widget d'un module intégré désactivé
/// (`crate::modules`). Un plugin chargeable (`plugin:<id>`) a le sien
/// (`plugins::widget_manifest`).
pub fn manifest(id: &str) -> Option<&'static Manifest> {
    MANIFESTS
        .iter()
        .find(|m| m.id == id)
        .filter(|m| crate::modules::enabled(m.id))
        .or_else(|| crate::plugins::widget_manifest(id))
}

/// Le widget `id` grandit avec son nombre d'éléments (`grows` de son manifeste).
pub fn grows(id: &str) -> bool {
    grow_min(id).is_some()
}

/// Plus petite largeur d'un widget extensible (« +N » seul).
pub fn grow_min(id: &str) -> Option<f32> {
    manifest(id).and_then(|m| m.grows).map(f32::from)
}

/// Démarrage d'une fonction (fils, connexions) ; un échec est journalisé sans arrêter Horizon.
pub type Start = fn(&AppHandle) -> anyhow::Result<()>;

/// Liaison d'un widget à une barre qui vient d'être créée (fil de l'interface), avant qu'elle
/// reçoive ses emplacements : son global Slint rempli (valeur initiale, un rang par exemplaire,
/// voir [`instances`]), ses abonnements (`host::listen` avec l'étiquette de la barre) et ses
/// rappels. Arguments : fenêtre, étiquette, disposition de la barre.
pub type Bind = fn(&AppHandle, &BarWindow, &str, &BarLayout);

/// Ce qu'un widget porté apporte (comme `Backend` de `app/src-tauri/src/widgets/mod.rs`).
pub struct Backend {
    /// Identifiant (dossier du widget).
    pub id: &'static str,
    /// Nom en clair (journal).
    pub name: &'static str,
    /// Enregistre ses états (`manage`), avant tout démarrage et toute barre.
    pub manage: fn(&AppHandle),
    /// Lance ses fils, après ceux du cœur et avant le suivi du premier plan (dont il peut être
    /// abonné).
    pub start: Option<Start>,
    /// Appelé pour chaque barre qui montre au moins un exemplaire du widget.
    pub bind: Option<Bind>,
    /// Horizon se ferme (enregistrer ce qui attend).
    pub exit: Option<fn(&AppHandle)>,
    /// Son panneau surgissant (`ui/widgets/<id>/panel.slint`).
    pub panel: Option<Panel>,
}

impl Backend {
    /// Widget sans rien : base des `BACKEND` (`..Backend::NONE`).
    pub const NONE: Backend = Backend { id: "", name: "", manage: |_| {}, start: None, bind: None, exit: None, panel: None };
}

/// Panneau d'un widget, côté Rust (fil de l'interface), montré par la fenêtre surgissante et par
/// les tuiles des barres hautes (`ui::panel_hosts`, une copie de ses globaux par fenêtre).
pub struct Panel {
    /// Relie son global dans une fenêtre, une fois (la surgissante à sa création, une barre au
    /// premier affichage d'une de ses tuiles) : valeurs de départ, rappels vers le service. Les
    /// abonnements aux événements du service se prennent une seule fois, dans la fenêtre
    /// surgissante (`host.is_popup()`, `host::listen(EV_…, Some(popup::LABEL), …)`) ; ils
    /// écrivent dans chaque fenêtre qui montre le panneau (`panel_hosts::each`).
    pub bind: fn(&AppHandle, &Host),
    /// Le panneau du genre (« media ») commence à être montré quelque part (surgissant ou
    /// tuile d'une barre affichée) : données à rafraîchir, suivi plus fréquent, minuteurs (comme
    /// `panel_opened` de la version Tauri, et le montage du `Panel.svelte`).
    pub opened: Option<fn(&AppHandle, &str)>,
    /// Il n'est plus montré nulle part : ce qu'`opened` a lancé s'arrête.
    pub closed: Option<fn(&AppHandle, &str)>,
    /// Une fenêtre commence à montrer le panneau `kind` (« weather:w1 » : une tuile a toujours
    /// son exemplaire) : ce qui dépend d'elle (exemplaire, écran de la barre).
    pub shown: Option<fn(&AppHandle, &Host, &str)>,
    /// Elle cesse de le montrer.
    pub hidden: Option<fn(&AppHandle, &Host, &str)>,
}

impl Panel {
    /// Panneau sans crochet d'ouverture ni de fermeture.
    pub const fn new(bind: fn(&AppHandle, &Host)) -> Panel {
        Panel { bind, opened: None, closed: None, shown: None, hidden: None }
    }
}

/// Backend du widget `id` (porté).
pub fn by_id(id: &str) -> Option<&'static Backend> {
    BACKENDS.iter().copied().find(|b| b.id == id)
}

/// Le widget `id` a un panneau (`panel.slint`), disponible dans cette édition.
pub fn has_panel(id: &str) -> bool {
    (PANELS.contains(&id) && manifest(id).is_some()) || id == crate::plugins::PANEL_KIND || crate::plugins::has_panel(id)
}

/// Appelle `f` avec le panneau du widget de `kind` (« weather:w1 » → météo), s'il en a un.
pub fn panel_hook(kind: &str, f: impl FnOnce(&Panel) -> Option<()>) {
    let base = crate::popup::base_of(kind);
    if base == crate::plugins::PANEL_KIND {
        f(&crate::plugins::PANEL);
    } else if let Some(p) = by_id(base).and_then(|b| b.panel.as_ref()) {
        f(p);
    }
}

/// Widgets portés (ordre alphabétique).
pub const BACKENDS: &[&Backend] = &[
    &calendar::BACKEND,
    &claude_sessions::BACKEND,
    &clock::BACKEND,
    &discord_voice::BACKEND,
    &headset::BACKEND,
    &media::BACKEND,
    &mic_krisp::BACKEND,
    &notifications::BACKEND,
    &phone::BACKEND,
    &shortcuts::BACKEND,
    &suggestions::BACKEND,
    &system::BACKEND,
    &weather::BACKEND,
    &windows::BACKEND,
];

/// Enregistre un état (raccourci pour `Backend::manage`).
pub fn manage<T: Send + Sync + 'static>(app: &AppHandle, state: T) {
    app.manage(state);
}

/// Enregistre les états de tous les widgets.
pub fn manage_all(app: &AppHandle) {
    for b in BACKENDS {
        (b.manage)(app);
    }
}

/// Widgets dont le service est démarré (un service ne démarre qu'une fois).
static STARTED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

/// Démarre les fils des widgets (pas ceux d'un module intégré désactivé) ; l'échec de l'un
/// n'empêche pas les suivants (il est noté pour le témoin d'état, `status.rs`).
pub fn start_all(app: &AppHandle) {
    for b in BACKENDS {
        if manifest(b.id).is_some() {
            start_backend(app, b);
        }
    }
}

/// Démarre le service du widget `id` s'il ne l'est pas déjà (module intégré activé en cours de
/// route).
pub fn start_one(app: &AppHandle, id: &str) {
    if let Some(b) = by_id(id) {
        start_backend(app, b);
    }
}

fn start_backend(app: &AppHandle, b: &'static Backend) {
    {
        let mut started = crate::util::lock(&STARTED);
        if started.contains(&b.id) {
            return;
        }
        started.push(b.id);
    }
    if let Some(start) = b.start
        && let Err(e) = start(app)
    {
        log::error!("{} : démarrage impossible : {e:#}", b.name);
        app.state::<crate::status::Health>().start_failed(b.name, format!("{e:#}"));
    }
}

/// Widgets dont la liaison à une barre dépend de sa disposition (exemplaires et leurs réglages,
/// style) : elle est refaite quand celle-ci change (`rebind`). Les autres sont liés une fois par
/// barre, à l'arrivée du premier exemplaire.
pub const LAYOUT_BOUND: &[&str] = &["calendar", "clock", "media", "mic_krisp", "shortcuts", "suggestions", "weather"];

/// Widgets qui ont une page de réglages (« Réglages… » de leurs menus) : ceux qui ont un
/// `Settings.svelte` dans la version Tauri (pages : `ui/widget-settings/`, `ui::widget_settings`).
const SETTINGS: &[&str] = &[
    "calendar",
    "claude_sessions",
    "headset",
    "mic_krisp",
    "notifications",
    "shortcuts",
    "suggestions",
    "weather",
];

/// Le widget `id` a des réglages (`settings` du registre de la version Tauri).
pub fn has_settings(id: &str) -> bool {
    if horizon_core::plugin::id_of_kind(id).is_some() {
        return crate::plugins::has_settings(id);
    }
    SETTINGS.contains(&id) && manifest(id).is_some()
}

/// Portée des abonnements posés par la liaison du widget `id` à la barre `label`.
fn scope(label: &str, id: &str) -> String {
    format!("{label}#{id}")
}

/// Ce dont dépend la liaison d'un widget de `LAYOUT_BOUND` : style, interaction, exemplaires
/// (identifiants et réglages, dans l'ordre).
fn signature<'a>(bar: &'a BarLayout, id: &'a str) -> (horizon_core::layout::BarStyle, Interaction, Vec<(&'a str, &'a serde_json::Value)>) {
    (bar.style, bar.interaction, instances(bar, id).map(|w| (w.id.as_str(), &w.settings)).collect())
}

/// Relie les widgets de la barre `label` (fil de l'interface) : ceux qu'elle montre. Renvoie les
/// genres liés (à garder pour `rebind`).
pub fn bind_all(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) -> std::collections::HashSet<&'static str> {
    let mut bound = std::collections::HashSet::new();
    rebind(app, window, label, None, bar, &mut bound);
    bound
}

/// La disposition de la barre `label` a changé (`old` → `new`) : les widgets qui y arrivent
/// sont liés, ceux de `LAYOUT_BOUND` dont les exemplaires, les réglages ou le style ont changé
/// sont reliés (leurs anciens abonnements retirés). `bound` : genres déjà liés à la barre.
pub fn rebind(
    app: &AppHandle,
    window: &BarWindow,
    label: &str,
    old: Option<&BarLayout>,
    new: &BarLayout,
    bound: &mut std::collections::HashSet<&'static str>,
) {
    for b in BACKENDS {
        let Some(bind) = b.bind else { continue };
        if manifest(b.id).is_none() || instances(new, b.id).next().is_none() {
            continue;
        }
        let again = bound.contains(b.id);
        if again && !(LAYOUT_BOUND.contains(&b.id) && old.is_none_or(|o| signature(o, b.id) != signature(new, b.id))) {
            continue;
        }
        let s = scope(label, b.id);
        if again {
            crate::host::unlisten_scope(&s);
        }
        crate::host::scoped(&s, || bind(app, window, label, new));
        bound.insert(b.id);
    }
}

/// Arrêt d'Horizon.
pub fn exit_all(app: &AppHandle) {
    for b in BACKENDS {
        if let Some(exit) = b.exit {
            exit(app);
        }
    }
}

/// Démarre plusieurs parties d'un widget : l'échec de l'une n'empêche pas les suivantes ; les
/// erreurs sont réunies.
pub fn start_each(app: &AppHandle, parts: &[(&str, Start)]) -> anyhow::Result<()> {
    let errors: Vec<String> =
        parts.iter().filter_map(|(name, start)| start(app).err().map(|e| format!("{name} : {e:#}"))).collect();
    if errors.is_empty() { Ok(()) } else { Err(anyhow::anyhow!(errors.join(" ; "))) }
}

/// Exemplaires du genre `kind` dans la bande d'une barre (gauche, centre, droite) : leur rang
/// dans cette suite est leur `Slot.data` (rangée de leurs données dans le global du widget).
pub fn instances<'a>(bar: &'a BarLayout, kind: &'a str) -> impl Iterator<Item = &'a WidgetInstance> {
    bar.widgets().filter(move |w| w.kind.as_str() == kind)
}

/// Rang de l'exemplaire `id` parmi ceux de son genre (voir [`instances`]).
pub fn data_index(bar: &BarLayout, w: &WidgetInstance) -> i32 {
    instances(bar, w.kind.as_str()).position(|x| x.id == w.id).unwrap_or(0) as i32
}

/// Le widget réagit aux clics (comme `clicksOn` de lib/interaction.ts) : barre cliquable, ou
/// barre « Survol seul » et widget cliquable par choix (`clickableOnHover`).
pub fn clicks_on(interaction: Interaction, clickable_on_hover: bool) -> bool {
    interaction == Interaction::Click || (interaction == Interaction::Hover && clickable_on_hover)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_matches_the_widget_folders() {
        assert_eq!(MANIFESTS.len(), 14);
        let clock = manifest("clock").unwrap();
        assert_eq!((clock.label, clock.icon, clock.squeeze), ("Horloge", "clock", None));
        assert_eq!(SQUEEZE_ORDER.first(), Some(&"suggestions"), "masqué le premier");
        assert_eq!(SQUEEZE_ORDER.last(), Some(&"system"));
        assert!(manifest("inconnu").is_none());
    }

    #[test]
    fn every_backend_is_a_widget_folder() {
        for b in BACKENDS {
            assert!(MANIFESTS.iter().any(|m| m.id == b.id), "{} sans dossier ui/widgets", b.id);
        }
        let mut ids: Vec<&str> = BACKENDS.iter().map(|b| b.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), BACKENDS.len(), "un seul Backend par widget, en ordre alphabétique");
        assert!(ids.is_sorted());
    }

    #[test]
    fn every_panel_is_bound_by_its_backend() {
        for id in PANELS {
            assert!(MANIFESTS.iter().any(|m| m.id == *id), "{id} : panel.slint sans widget.json");
            assert!(by_id(id).is_some_and(|b| b.panel.is_some()), "{id} : panel.slint sans Backend::panel");
        }
        for b in BACKENDS {
            assert!(b.panel.is_none() || PANELS.contains(&b.id), "{} : Backend::panel sans panel.slint", b.id);
        }
    }

    #[test]
    fn layout_bound_and_settings_name_widgets() {
        for id in LAYOUT_BOUND.iter().chain(SETTINGS) {
            assert!(MANIFESTS.iter().any(|m| m.id == *id), "{id} : pas de widget");
        }
        assert!(has_settings("weather") && !has_settings("clock"));
    }

    #[test]
    fn clicks_follow_the_interaction() {
        assert!(clicks_on(Interaction::Click, false));
        assert!(!clicks_on(Interaction::Hover, false));
        assert!(clicks_on(Interaction::Hover, true));
        assert!(!clicks_on(Interaction::Info, true));
    }

    #[test]
    fn data_index_counts_instances_of_the_same_kind() {
        let bar: BarLayout = serde_json::from_value(serde_json::json!({
            "monitor_id": "m", "monitor_pos": [0, 0], "role": "main", "style": "islands",
            "left": [{ "id": "a", "kind": "weather" }, { "id": "b", "kind": "clock" }],
            "right": [{ "id": "c", "kind": "weather" }],
        }))
        .unwrap();
        let c = bar.right[0].clone();
        assert_eq!(data_index(&bar, &c), 1);
        assert_eq!(data_index(&bar, &bar.left[1].clone()), 0);
        assert_eq!(instances(&bar, "weather").count(), 2);
    }
}
