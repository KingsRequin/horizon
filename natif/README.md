# Horizon natif

La barre d'Horizon en Rust + Slint 1.18 (rendu logiciel, fenêtres « layered »), sans WebView2.
Elle a remplacé en 2026 une première version Tauri + Svelte (non publiée) : les « repris de
`app/src-tauri/…` » ou « `app/src/…` » des en-têtes renvoient à cette version, et seulement à
titre d'historique. Données :
`%APPDATA%\Horizon` (`settings.toml`, `layout.json`…) ; journal `%APPDATA%\Horizon\horizon.log`.

## Lancer

```
scripts\verifier.ps1 -Exe               (clippy, tests, exe de debug ; interface interprétée)
cargo build -p horizon-natif             (le même exe de debug, seul)
cargo build -p horizon-natif --no-default-features [--release]   (interface compilée, comme
                                         l'exe publié ; obligatoire avec --release)
%LOCALAPPDATA%\Programs\Horizon\horizon.exe --quit      (les deux réservent le haut des écrans)
<dossier de compilation>\{debug|release}\horizon.exe [--essai]   (target\ par défaut ; en
                                         debug : la dernière des deux formes compilées)
horizon.exe --quit                       (ferme l'instance en cours)
Start-Process "$env:LOCALAPPDATA\Programs\Horizon\horizon.exe"   (toujours relancer l'installée)
```

Une compilation lancée hors du dossier d'installation n'a pas d'identité de paquet : la cloche
des notifications y reste estompée.

`--essai` ouvre aussi deux fenêtres d'essai de la plateforme (ordinaire et sans cadre) : saisie
de texte, focus, clavier, clics, double-clic, clic droit, molette, curseurs.

## Organisation

```
natif/
  build.rs              catalogues (widgets, icônes), manifeste et icône de l'exe
  ui/                   interface Slint, dans son paquet (horizon-ui) : Cargo.toml, lib.rs,
                        build.rs (compile main.slint, trouve les panneaux)
    theme.slint         jetons d'app/src/app.css (Theme, Styles.islands / Styles.graphic)
    components.slint    Icon, Island, Slot / SlotView (emplacement nommé)
    widget-view.slint   WidgetView : le widget d'un emplacement selon son genre
    widget-frame.slint  WidgetFrame (cadre d'un widget), BarButton, StatusDot
    panels.slint        Panels (accroche des panneaux et réglages), PanelArea
    popup.slint         PanelWindow : la fenêtre surgissante (cadre, fondus, survol, Échap)
    settings-window.slint  SettingsWindow : la fenêtre de réglages (pages : « options »…)
    settings/           briques des fenêtres de réglages, une par fichier (voir « Réglages »)
    options/            page « Réglages d'Horizon » : OptionsData, navigation, une section par fichier
    palette.slint       PaletteWindow : la palette de commandes (Ctrl+Alt+P)
    panel/              briques des panneaux, une par fichier (voir « Panneaux »)
    gauge.slint         Eyebrow, Track, Gauge
    tooltip.slint       Tip (infobulles)
    app-button.slint    AppButton, AppTray (boutons d'applis)
    wheel-hint.slint    bulle de la molette
    bar.slint           une barre : bande (gauche, centre, droite), tuiles d'une barre haute
    tile.slint          Tile : tuile d'une barre haute (le panneau d'un widget, `PanelFill`)
    dashed.slint        DashedFrame : contour pointillé (mode édition)
    essai.slint         fenêtre d'essai de la plateforme
    main.slint          ce qui est exporté vers Rust
    widgets/<id>/       UN DOSSIER PAR WIDGET (widget.json, widget.slint, panel.slint)
  fonts/                Inter, Space Grotesk, JetBrains Mono (TTF) et leurs licences OFL
  icons/phosphor/       icônes Phosphor utilisées (SVG, licence MIT), noms de lib/icons.ts
  icons/simple-icons/   logos de marques (raccourcis, notifications), CC0
  src/
    platform/           plateforme Slint maison : fenêtres Win32, boucle, clavier, presse-papiers
    host/               ce qui imite Tauri (AppHandle, Manager, Emitter, événements)
    state.rs, util.rs, bars.rs, autohide.rs, resume.rs, presence.rs, foreground.rs,
    game.rs, comfort.rs, tray.rs     modules repris de app/src-tauri/src (voir plus bas)
    relay.rs, wsclient.rs, web.rs, brands.rs, mixer.rs   services partagés
    alerts.rs, status.rs   îlot dynamique, état des intégrations
    modules.rs          modules intégrés (Sessions Claude Code, Téléphone) : activés ou non
    accent.rs, osd.rs   accent (pochette, fond d'écran, Prisme, Windows) et affichage du volume
    options.rs          « Réglages d'Horizon » : état, enregistrement, options des barres,
                        raccourci ; règles pures testées ;
                        réglages relus toutes les 5 s, EV_OPTIONS ; motion.rs : animations réduites
    settings_window.rs  pages, titres et placement de la fenêtre de réglages (repris de Tauri)
    palette/            palette : catalogue, recherche, actions, raccourci global (RegisterHotKey)
    popup.rs            règles du panneau surgissant (placement, passage d'un widget à l'autre)
    edit.rs, profiles.rs  mode édition (disposition, menus, annuler) et profils (repris de Tauri)
    drag.rs, undoable.rs  cible du glisser-déposer, retrait annulable : logique pure
    overflow.rs         barre trop chargée (règle de lib/overflow.ts), logique pure
    instance.rs         une seule instance, --quit
    journal.rs          journal
    ui/                 liaison des fenêtres Slint (bar.rs, popup.rs, essai.rs) et briques
    widgets/            catalogue (manifestes, BACKENDS) et un module par widget
    plugins/            hôte des plugins, bibliothèque (voir « Plugins »)
```

## Plateforme (`src/platform`)

- Une seule plateforme par processus ; toutes les fenêtres passent par elle. Elle crée une
  fenêtre Win32 par fenêtre Slint (`NativeWindow`, adaptateur `WindowAdapter`).
- `platform::configure(component.window(), WindowSpec { … })` **avant** `show()` : cadre
  (`Frame::Layered` sans cadre, alpha par pixel ; `Frame::Normal` fenêtre Windows ordinaire,
  opaque, redimensionnable), rectangle physique ou taille logique centrée, `focusable` (faux :
  ne prend jamais le focus, même cliquée), `topmost`, `taskbar`. Renvoie le HWND. Sans
  `configure`, `show()` crée une fenêtre ordinaire centrée de 640 × 480.
- Rendu : seul le rectangle modifié est redessiné et présenté ; une fenêtre cachée ne dessine
  rien. La boucle dort jusqu'au prochain minuteur Slint ou message ; ~60 images/s seulement
  pendant une animation Slint (qui doit se terminer : jamais d'animation sans fin).
- **Un pixel d'alpha nul laisse passer la souris** (fenêtre layered). Une zone qui doit capter
  la souris sans rien montrer prend un fond d'alpha 1/255 (`#00000001`, fond des barres).
- Souris : survol, sortie, boutons (capture pendant l'appui), double-clic (calculé par Slint),
  clic droit, molette (60 px logiques par cran, verticale et horizontale), curseurs demandés
  par Slint (`mouse-cursor` d'un `TouchArea`, I du `TextInput`).
- Clavier (`keys.rs`, testé) : touches spéciales et modificateurs (gauche/droite, AltGr) par
  `WM_KEYDOWN`, texte par `WM_CHAR` (accents, touches mortes, AltGr, surrogates), raccourcis
  `Ctrl+lettre` en minuscule (« z » pour annuler), modificateurs resynchronisés au focus.
  Presse-papiers texte (Ctrl+C / Ctrl+V des champs).
- DPI par écran (manifeste PerMonitorV2) : l'échelle suit l'écran de la fenêtre
  (`WM_DPICHANGED`) ; une fenêtre sans cadre (barre) est replacée par son propriétaire.
- `on_context(fenêtre, f)` : `f(x, y)` après chaque clic droit relâché (menu de toute la
  fenêtre : le `TouchArea` d'un élément garde le clic pour lui) ; `set_focusable(fenêtre, b)` :
  la fenêtre peut prendre le focus (mode édition d'une barre) sans toucher au reste de sa
  description.
- IME (`platform/ime.rs`) : `input_method_request` place la fenêtre de composition et les
  candidats au curseur du champ actif ; le texte validé arrive par `WM_CHAR`. Icône des
  fenêtres : ressource 1 de l'exe (`horizon.rc`).
- Pas encore : composition affichée dans le champ (elle reste dans la fenêtre de l'IME),
  curseurs dessinés, minuteurs Slint pendant qu'on déplace une fenêtre ordinaire à la souris (boucle modale de
  Windows), accessibilité.

## Hôte compatible (`src/host`) : différences avec l'API de Tauri

Un module de `app/src-tauri/src` se reprend en changeant ses `use tauri::…` en
`use crate::host::…`, et :

| Tauri | Horizon natif |
| --- | --- |
| `tauri::AppHandle`, `Manager::{manage, state, try_state}`, `State<'_, T>` (`.inner()`) | identiques (`host::AppHandle`, `host::Manager`, `host::State`) |
| `Emitter::{emit, emit_to, emit_filter}` | identiques ; la charge doit être `Send + Sync + 'static` (elle traverse vers le fil de l'interface sans être sérialisée) ; `emit_to` prend l'étiquette (`&str`) |
| `#[tauri::command] fn x(app: AppHandle, …)` | fonction ordinaire, appelée par le rappel Slint (`window.on_x(…)`) ; arguments en `snake_case` |
| écouter un événement dans la page (`on(...)`) | `host::listen(EV_…, Some(label), |e| …)` sur le fil de l'interface ; charge typée `e.payload::<T>()` (ou `e.json()`) ; retiré avec la fenêtre (`unregister_window`) |
| `app.get_webview_window(label)` → `.hwnd()` | `app.get_window(label)` → `host::Window` (`label()`, `hwnd()` en `isize`, `app_handle()`) |
| `WebviewWindowBuilder` | composant Slint `X::new()` + `platform::configure` + `show()` (fil de l'interface), puis `app.register_window(label, hwnd)` |
| `app.run_on_main_thread(f)`, `app.exit(code)` | identiques |
| `util::Published<T>` | `T: Clone + PartialEq + Serialize + Send + Sync + 'static` |
| `util::data_url` | absent : une image va à Slint en `slint::Image` (`SharedPixelBuffer`) |
| `tauri::async_runtime` | absent : fils (`util::start_thread`) ; à décider si un widget en a vraiment besoin |
| ressources d'un module de widget : `Backend { manage, start, exit }` | `widgets::Backend { id, name, manage, start, bind, exit }` dans `BACKENDS` (`bind` : liaison à une barre) |
| `State::inner()` | rend `&'a T` pour toute la durée de l'emprunt de l'`AppHandle`, comme Tauri (`&app.state::<S>().inner().x` peut être rendu) |
| (rien) | `host::scoped(portée, f)` : les abonnements posés par `f` partent ensemble (`unlisten_scope`) ; sert à relier de nouveau un widget quand la disposition de sa barre change |

Écarts dans les modules repris (tous signalés dans leur en-tête) : `bars.rs` crée des fenêtres
Slint et affiche / cache par le fil de l'interface ; `presence.rs` et `game.rs` réveillent le
son ; `tray.rs` réécrit en Win32 (menu complet : réglages, modifier la barre, palette, « Ne pas
déranger », recharger, démarrage avec Windows, journal, quitter) ; `state.rs`, `accent.rs`,
`resume.rs`, `foreground.rs`, `util.rs` presque identiques.

## Thème, polices, icônes

- `ui/theme.slint` : `Theme` (commun : polices, tailles, durées, accent, rayons…) et
  `Styles.islands` / `Styles.graphic` (structure `StyleTokens` : couleurs et tailles de chaque
  style de barre). Un composant reçoit `t: StyleTokens` de sa barre. Pas de courbe
  d'accélération stockable : écrire `easing: cubic-bezier(0.2, 0.8, 0.2, 1)` (ease-out).
  `color-mix(accent N %, transparent)` → `Theme.accent.with-alpha(N / 100)` ; avec une autre
  couleur → `a.mix(b, N / 100)`.
- Polices : `font-family: Theme.font-ui` (Inter 400/500/600), `Theme.font-display` (Space
  Grotesk 500/600), `Theme.font-mono` (JetBrains Mono 500/600). Le style graphique écrit en
  Space Grotesk (`t.font`).
- Icônes : copier le SVG Phosphor dans `icons/phosphor/<nomCamelCase>.svg` (même nom que dans
  `app/src/lib/icons.ts`). Fixe, dans un `.slint` : `Icon { source:
  @image-url("../../../icons/phosphor/clock.svg"); size: 14px; color: t.text; }` (chemin relatif
  au `.slint` ; demande la fonction `image-decoders` de `i-slint-core`, déjà activée). Choisie à
  l'exécution : `ui::icons::image("clock")` (cache, rendu à la taille affichée).
- Logos Simple Icons (raccourcis, notifications) : `icons/simple-icons/brands.tsv` (paquet
  `simple-icons` 16.37, CC0), une ligne par logo `slug⇥couleur⇥nom⇥tracé`, triée par identifiant ;
  `brands::brand(slug)` (recherche dichotomique dans le texte embarqué), `brands::image`,
  `readable_on_dark`. Régénérer après une mise à jour du paquet (depuis `app/`) :
  `node -e "const m=require('simple-icons');const v=Object.values(m).filter(x=>x&&x.slug&&x.path).sort((a,b)=>a.slug<b.slug?-1:1);require('fs').writeFileSync('../natif/icons/simple-icons/brands.tsv',v.map(x=>[x.slug,x.hex,x.title.replace(/\t/g,' '),x.path].join('\t')).join('\n')+'\n')"`.

## Fonctions transverses de la barre

- **Accent** (`src/accent.rs` → `EV_ACCENT`, `src/ui/accent.rs`) : `Theme` est un global par
  fenêtre en Slint ; toute fenêtre ouverte appelle `ui::accent::follow(&fenêtre)` (barres,
  panneau surgissant, essai — à faire aussi pour les fenêtres des phases E et F). Fondu de
  700 ms mené par Rust (un global Slint ne s'anime pas), immédiat si les animations sont
  réduites. Les dérivés (`Theme.accent-soft`, `mix`, `with-alpha`) suivent seuls.
- **Affichage du volume** (`src/osd.rs` → `EV_OSD`, `src/ui/osd.rs`, `ui/osd.slint`, global
  `OsdData`) : barre principale seulement, centré dans la bande ; l'îlot de Windows est masqué
  pendant ce temps et remis à la fermeture (`osd::exit`, aussi en cas de panique), en plein
  écran sur l'écran principal et quand `[features] osd` est coupé.
- **Minuteur et chronomètre** (`src/timer.rs`, logique pure `horizon_core::timer` → `EV_TIMER`) :
  une séance à la fois (minuteur, pomodoro ou chronomètre), affichée par l'horloge
  (`src/widgets/clock/`) et, si elle est demandée, par la **fenêtre flottante**
  (`src/ui/timer_float.rs`, `ui/timer-float.slint`) : sans cadre, au premier plan, jamais
  active, déplacée par un glisser mené en Rust (position du pointeur à l'appui et à chaque
  mouvement : pas de boucle modale de Windows, les minuteurs Slint continuent) ; passée sur un
  écran d'une autre échelle, elle reprend sa taille logique ; au relâchement, ramenée dans la
  zone de travail de l'écran le plus proche (`float_rect`) et sa place gardée
  (`timer-float.json`). Essai sans souris, instance en cours : `horizon.exe --minuteur-essai
  chrono|minuteur|pomodoro|pause|reprise|zero|stop|flottante|fixe|panneau`.
- **Animations de la musique, par barre** : `BarLayout::music` (`horizon_core::layout::BarMusic` :
  `backdrop`, `progress`, `pulse`, dans `layout.json`), réglées par `BarOptionsPatch` (Réglages ›
  Barres, `ui::options::music_entries` ; menu de la barre › Musique, `MenuAction::Music`).
  Préréglage du style à la création (`BarMusic::for_style`), suivi au changement de style s'il
  n'a pas été touché (`BarLayout::restyle`) ; « lumière qui tourne » refusée sur une barre
  graphique (`Pulse::fits`). Reprise une fois des réglages globaux de 0.7.2 et 0.7.3
  (`Layout::adopt_music`, marque `music-per-bar`, au chargement de `AppState::load`).
  `BarMusic::effective` coupe tout si Windows demande moins d'animations (relu toutes les 5 s
  par `options.rs`, qui émet alors `EV_OPTIONS`).
- **Pulsation** (`src/ui/pulse.rs`, `ui/pulse.slint`, global `PulseState`) : Rust pose les
  valeurs du son (basse, niveau, flash, compteur de grosses caisses), l'animation de la barre
  (`pulse::set_bar`, à chaque disposition) les dessine : `PulseLayers` dans `Island` (style A,
  sous le contenu) et sur la bande (style C) pour « edges », « breath » et « wave », `HorizonLine`
  au bas de chaque îlot (style A) ou de la bande (style C) pour « horizon » (elle part du widget
  Lecture en cours, qui pose `PulseState.origin-x` ; la barre pose `PulseState.span`). Une
  animation déclenchée par une grosse caisse passe par `Kick` (`p` de 0 à 1 à chaque
  changement de `beat`) et se termine d'elle-même ; si elle dure plus que l'écart entre deux
  coups, deux exemplaires se relaient selon la parité du coup (`WaveRun`, `Sparks`). Un dégradé
  est peint en rectangle, coins ignorés : la vague est faite de bandes unies rentrées à la
  courbe des coins, à l'échelle de l'îlot (traîne d'au plus la moitié de sa largeur, même
  vitesse partout : sur l'îlot d'une seule icône, une vague fixe de 48 px devenait un bloc en
  escalier qui rampait). Les îlots des plugins (image rendue à part, qui ne voit pas
  `PulseState`) reçoivent la pulsation de l'hôte, posée par-dessus leur image à la place de
  l'îlot de `@horizon` (`PluginWidget`, `ui/plugin-view.slint`). En pause ou dans un silence, les valeurs retombent au rythme des images de
  son (`fade_out`), pas d'un coup. Une valeur qui suit
  le son glisse d'une image de son à la suivante par un `animate` de durée `PulseState.sound`,
  jamais d'une durée fixe : relancée à chaque image, une transition ne progresse que pendant
  les tours de boucle sans image, et se fige dès que l'interface est chargée (vu sur l'exe de
  debug). `Lateness` (`widgets/media/audio.rs`) relève le retard de chaque image et annule
  alors cette durée : les valeurs sont posées telles quelles. Mieux, quand c'est possible :
  adoucir la valeur en Rust et la poser sans transition (le niveau de la respiration,
  `Meter::frame`), ce qui ne fait redessiner que 20 fois par seconde. Le son est demandé au
  hub par utilisateur (`AudioHub::subscribe(barre, "pulse" | "media", …)`). Ajouter une
  animation : une variante de `Pulse` (identifiant, libellé, explication), ce que `Meter::frame`
  lui envoie, son composant dans `ui/pulse.slint`.
- **Fond de la lecture en cours** (`BarMusic::backdrop`, `horizon_core::settings::Backdrop`,
  `src/widgets/media/ui.rs`, relié de nouveau quand la musique de la barre change : elle est
  dans la `signature` du widget) : `MediaData.backdrop`. Le spectre et les capsules sont des
  rectangles de l'interface (`Spectrum`, `Capsules`), qui reçoivent leurs valeurs et retombent
  en pause (`MediaData.live`). Le halo, l'aurore, le ruban, les points, la progression qui
  ondule (`progress = "wave"`) et la lumière qui tourne (`pulse = "comet"`) sont
  une **image de l'îlot** (`horizon_core::music::canvas` : `Canvas::paint(scène, son, couleurs)`,
  pixel par pixel, coins arrondis compris, testée sans Windows), refaite à chaque image de son
  à la taille que le widget annonce (`MediaData.island-w/h/radius`, `resized`) et posée dans
  `MediaData.canvas`. Le mouvement y est adouci par `Sound::feed` (montée vive, retombée
  lente), pas par une transition ; sans son (pause, silence de 400 ms, hub muet en mode jeu),
  `Sound::fade` fait tout retomber puis le minuteur s'arrête. Ajouter un fond en image : une
  variante de `Backdrop` rangée dans `is_painted`, sa fonction de dessin dans `Canvas`. Les
  bandes ne sont demandées au hub que pour un fond qui les dessine. Le widget retiré d'une
  barre s'arrête (`Backend::unbind` : écouteurs, abonnement, minuteurs).
- **Son d'essai** : `horizon.exe --musique-essai [secondes]` (20 sans précision) montre un
  morceau en lecture sur les barres et remplace la capture par un son fabriqué
  (`horizon_core::music::essai_frame` : une grosse caisse par demi-seconde), mode jeu compris,
  pour voir et capturer les animations sans rien jouer. Un autre morceau qui démarre l'arrête ;
  relancé, il prolonge le précédent. `--musique-essai 8 kick` (ou `bass`, `level`) ne joue
  qu'un ingrédient (`horizon_core::music::Essai`) : chaque animation se juge seule, sur ce qui
  la fait bouger (bords et ligne d'horizon : `bass` puis `kick` ; onde : `kick` ; respiration et
  lumière qui tourne : `level`), filmée par `film.ps1` (champ 4 : l'ingrédient), en mesurant
  la couleur image par image plutôt qu'à l'œil.
- **Mode nuit** (style C) : le voile couvre toute la bande, widgets et affichage du volume
  compris (au-dessus d'eux, comme `.dimmable.graphic .band::after`).

## Réglages et palette (phase F)

- **Fenêtre de réglages** (`src/settings_window.rs` + `src/ui/settings_window.rs`,
  `ui/settings-window.slint`) : une seule instance, fenêtre Windows ordinaire (`Frame::Normal` :
  cadre, barre de titre sombre quand Windows l'est, redimensionnable — 460 × 420 au moins —,
  clavier, Tab, molette), posée au centre de la zone de travail de l'écran de la barre du widget
  (sinon de celui de la souris) à son échelle (`centered_on`, 760 × 640 pour « Réglages
  d'Horizon », 560 × 620 sinon), titre de la page. `settings_window::open(app, page)` de
  n'importe quel fil (`SettingsPage::Options(section)`, `Widget(id)`, `Profiles`) ;
  `settings_open(app, "options:etat")` depuis un nom. Rouverte : ramenée au premier plan, sur la
  nouvelle page (et déplacée si l'écran voulu a changé). Fermée (croix, Alt+F4) : la page
  enregistre ce qui attend (`closing`), le retrait annulable devient définitif, la fenêtre est
  lâchée. `EV_SETTINGS_WINDOW` et `ui::settings_window::is_open()` : le mode édition d'une barre
  ne se termine pas quand elle prend le focus. Retrait annulable :
  `ui::settings_window::offer_undo(Removal { … })` (bandeau `UndoBar` en bas, 5 s ; rendu
  définitif aussi au changement de page). Un rappel d'annulation s'exécute pendant que la
  fenêtre est tenue : il ne rappelle ni `window()` ni `offer_undo` (chaque page garde sa propre
  poignée faible de la fenêtre).
  **Ajouter une page** : un élément `visible: root.page == "…"` dans `ui/settings-window.slint`,
  sa liaison appelée par `try_show`, son départ dans `leave_page` et `close`, le nom de la page
  dans `page_name`. Seule la page affichée pose `SettingsLayout.content-w` (largeur de son
  contenu qui défile : `if root.visible` dans `init` / `changed content-w`, et `changed visible`).
- **Réglages d'un widget** (`ui/widget-settings/`, `src/ui/widget_settings/`, repris de
  SettingsApp.svelte et des `Settings.svelte`) : en-tête (nom du widget, « Les modifications sont
  enregistrées automatiquement. »), puis la page du genre : Agenda (adresses iCal, nom, couleur
  gardée, « Tester » et lecture à l'ouverture, `calendar::calendar_test`), Raccourcis (liste
  glissable, cible avec « Parcourir… », icône logo / image / appli, recherche de logos
  `brands::search`, couleur par le sélecteur de Windows), Suggestions (exclusions, retrait
  différé), Météo (recherche `weather::weather_search` 350 ms après la frappe), Micro et Krisp
  (une case) ; Casque, Sessions Claude, Notifications : `GlobalLink` ; plugins : page générée
  depuis les réglages de leur manifeste (`plugin.slint`, `plugin.rs`). Chaque
  modification fusionne ses champs dans les réglages (`save`), enregistrés 300 ms après la
  dernière par `edit::edit_set_widget_settings` (une étape d'annulation par ouverture) ; les barres suivent aussitôt (`EV_LAYOUT`, liaisons refaites ;
  Agenda : une liste d'adresses nouvelle est lue tout de suite, `calendar::request`). Ouverte
  par « Réglages… » (menu d'un widget en édition), « Réglages de « X »… » (menu de la barre) et
  `Panels.settings(id)` (widget pas encore réglé cliqué : Météo, Raccourcis, Agenda).
- **Profils** (`ui/profiles/page.slint`, `src/ui/profiles.rs`, repris de ProfilesEditor.svelte) :
  renommer, activer, supprimer (différé, annulable, au moins un profil), applis associées
  (retrait annulable, ajout parmi les applis récentes du journal : « sera retirée de « X » »),
  « Nouveau profil (copie du profil actif) ». Commandes de `profiles.rs`, liste suivie par
  `EV_PROFILES`. Ouverte par « Gérer les profils… » (menu Profil, Réglages d'Horizon › Général,
  palette) et après « Nouveau profil (copie de celui-ci) » du menu.
- **Briques** (`ui/settings/`, une par fichier, couleurs `SettingsLayout.t` = `Styles.settings`,
  surfaces opaques de `.islands.settings`) :

| Fichier | Brique | Version Tauri |
| --- | --- | --- |
| `layout.slint` | global `SettingsLayout` : `content-w` (largeur du contenu, posée par la page ; les textes qui passent à la ligne en tirent leur largeur), `t` | — |
| `scroll.slint` | `SettingsScroll` : contenu qui défile (colonne en enfants, centrée, `max-content-w` 640 px au plus, marges `padding-x/y`, molette, barre fine glissable), `content-w`, `scroll-top()` | `overflow: auto` de la page |
| `text.slint` | `SettingsTitle` (titre de page et `description`), `SettingsHeading` (titre d'un groupe de cartes, `first`, `below`), `SettingsSubheading` (`h3` + `small`), `SettingsNote` (paragraphe : `color`, `above`, `below`, police) | `h2`, `h3`, `.note`, `.intro` |
| `card.slint` | global `Card` : mesures et couleurs des cartes (une carte = un réglage, comme les Paramètres de Windows 11), `text-w(nested, contrôle)` | — |
| `row.slint` | `SettingRow` : carte, libellé et `hint` à gauche, contrôle en enfants à droite ; `nested` : ligne d'une carte dépliante | `Row.svelte` |
| `check.slint` | `SettingCheck` : carte à interrupteur, effet immédiat (`checked`, `label`, `hint`, `enabled`, `nested` ; `toggled(bool)`) | `Check.svelte` |
| `switch.slint` | `Switch` : dessin de l'interrupteur (`checked`, `hovered`, `enabled`) | — |
| `expander.slint` | `SettingExpander` : carte dépliante, réglage principal (interrupteur si `switchable`) et dessous les réglages qui en dépendent (enfants `nested`) ; `expanded` | — |
| `segmented.slint` | `Segmented` (`value`, `choices: [SegmentChoice { value, label, hint }]`, `enabled` ; `selected(valeur)`), infobulle par choix | `Segmented.svelte` |
| `field.slint` | `CommitField` (texte validé à Entrée ou à la sortie, `commit-on-blur`, `placeholder` ; `committed`, `edited`), `NumberField` (borné, ↑ ↓ = ± `step`), `SelectField` (`open(x, y)` : le service ouvre un menu Windows sous le champ, `ui::menu::show_at`) | `<input>`, `<input type="number">`, `<select>` |
| `button.slint` | `SettingsButton` (filet ; `small`, `filled`, `on`, `mono`, `icon`, `tip` ; `clicked`, `key(KeyEvent)`, `grab()`), `RemoveButton` (corbeille, rouge au survol, « Retirer X » ou `reason`) | boutons des sections, `RemoveButton.svelte` |
| `global-link.slint` | `GlobalLink` (`what`, `section`, `label` ; `open(section)` → `settings_open("options:" + section)`) | `GlobalLink.svelte` |
| `banner.slint` | `SettingsBanner` : avertissement teinté `warn` (settings.toml illisible) | `.banner` |
| `input.slint` | `InputField` : champ libre, prévenu à chaque frappe (`edited`), à Entrée (`accepted`) et à Entrée ou à la sortie s'il a changé (`committed`) ; `value` le remplace sans toucher au texte tapé identique, `set-text`, `invalid` (+ `invalid-color`), `quiet` (sans fond ni filet au repos), `mono` | `<input>` des réglages des widgets et des profils |

  Aussi : `UndoBar` (`ui/undo-bar.slint`), `TextField` (`ui/panel/field.slint`, champ de
  panneau), `Slider` (`ui/panel/slider.slint`, luminosité de la nuit), `StatusDot`, `Tip`
  (infobulles, branchées par la fenêtre). Rust : `ui::model::sync_rc` (modèle d'une propriété
  Slint mis à jour ligne par ligne), `ui::dialogs` (boîte « Ouvrir » et sélecteur de couleurs de
  Windows, modaux, hors d'un rappel Slint). Mise en page qui passe à la ligne : `FlexboxLayout`
  de Slint (puces d'applis des profils).
- **« Réglages d'Horizon »** (`ui/options/`, global `OptionsData`, `src/ui/options.rs`,
  `src/options.rs`) : recherche (`horizon_core::options_search`, un clic mène à la section), sections Barres,
  Apparence, Son et média, Alertes, Minuteur, Palette, Plugins (tuiles des plugins installés, des
  modules intégrés et de la bibliothèque, `src/ui/plugin_tiles.rs`),
  Général, État (`options_search::SECTIONS`, anciens noms « fonctions » et « nuit » repris). Une option se désigne par son chemin dans
  `settings.toml` (« features.osd ») : `options::apply_edit` (règles et bornes des sections,
  testées), affichée tout de suite, enregistrée 400 ms après la dernière modification
  (`options_set` → `state::save_settings`) ; les fonctions qui ne relisent pas les réglages sont prévenues (`applied` :
  nuit, masquage, accent, affichage du volume, minuteur, « Ne pas déranger », état). Options des
  barres : `options_set_bar` (disposition, annulable par Ctrl+Z). Démarrage avec Windows :
  `tray::set_autostart`. Raccourci de la palette : saisi au
  clavier (`options::capture_hotkey`, règle de lib/hotkey.ts), pris à chaud. Page État :
  `status::status_get` (hors du fil de l'interface : le journal est relu) toutes les 15 s tant
  qu'elle est affichée, actions `status_action`.
- **Palette** (`src/palette/`, `src/ui/palette.rs`, `ui/palette.slint`) : fenêtre layered
  transparente créée au démarrage (620 × 470, sur l'écran de la souris, au cinquième haut),
  absente d'Alt+Tab mais activable ; raccourci global `RegisterHotKey` (`horizon_sys::hotkey`,
  pas de crochet clavier) ; catalogue construit hors du fil de l'interface à chaque ouverture
  (`EV_PALETTE_ENTRIES` relance la recherche), classement `horizon_core::palette`, usage
  (`palette.json`). Perte du premier plan = fermeture ; Échap et
  une action rendent le premier plan à la fenêtre d'avant. Ouverte aussi par le menu de la
  barre et l'icône de notification.
- **Essais** (instance en cours) : `horizon.exe --reglages [page]` ouvre la fenêtre des
  réglages (section des Réglages d'Horizon, `widget:<id d'exemplaire>`, `profiles`) ;
  `--palette-essai` ouvre la palette sans qu'aucune de ses actions ne s'exécute (journalisées).
- Écarts avec Tauri : liste déroulante = menu Windows (la version Tauri montre la liste de
  Chromium) ; boîte « Ouvrir » et sélecteur de couleurs = ceux de Windows (comme `dialog()` et
  `<input type=color>`) ; pas de `code` en chasse fixe au milieu d'un paragraphe ; logos trouvés
  classés par identifiant (Tauri : ordre du paquet) ; pas d'interlignage 1,5 dans les
  paragraphes ; pas d'ombre sous la palette ni sous le bandeau d'annulation (rendu logiciel).

## Mode édition, menus, profils

- **Service** `src/edit.rs` (repris de `app/src-tauri/src/edit.rs`) : `edit_move`, `edit_undo`,
  `edit_redo`, `edit_end`, `edit_bar(app, écran, on)` (envoie `EV_EDIT` à la barre),
  `edit_main_bar` (icône de notification), `edit_set_widget_settings` (page de réglages),
  `broadcast_layout` (`EV_LAYOUT` à chaque barre, hauteurs suivies). Menus décrits en données
  (`menu_bar`, `menu_widget`, `menu_add`) ; le choix revient à `on_menu(app, id)`
  (`MenuAction::parse`, testé). « Réglages d'Horizon » et la palette ouvrent leurs fenêtres ;
  « Réglages… » d'un widget et « Gérer les profils… » ouvrent la fenêtre de réglages.
- **Profils** `src/profiles.rs` : bascule, « Nouveau profil (copie de celui-ci) » (alerte
  « Profil créé »), profil automatique selon l'appli au premier plan (retour 2 s après),
  commandes de la fenêtre des profils (`profiles_state`, `_add`, `_rename`, `_delete`,
  `_set_apps`, `_recent_apps`) en fonctions testées, `EV_PROFILES` (témoin de la barre).
- Disposition modifiée : enregistrée par `AppState::persist` (écritures ordonnées). Une barre reçoit `EV_LAYOUT` et se met à jour
  sans être recréée (`ui::bar::relayout`) : emplacements ligne par ligne, widgets qui arrivent
  reliés, widgets de `widgets::LAYOUT_BOUND` (liaison qui lit les exemplaires, le style)
  reliés de nouveau avec `host::scoped` / `unlisten_scope`. Un widget de cette liste dont la
  liaison garde une liste de fenêtres doit remplacer l'entrée de la même fenêtre (agenda,
  horloge, lecture en cours).
- **Interface** `src/ui/edit.rs` : entrée (menu, icône de notification, palette),
  sortie (Échap, « Terminé », menu, perte du premier plan sauf pendant un menu), Ctrl+Z,
  Ctrl+Y / Ctrl+Maj+Z, aide sous la barre (`ui::popup::show_help`, `ui/edit-help.slint`),
  menus du clic droit (`platform::on_context`, survol des emplacements) et du « + »,
  glisser-déposer (`slot-drag` de `BarWindow` ; cible et repère par `crate::drag`, zones lues
  dans la barre : `left-zone-x`…, `*-row-x`, largeur montrée `slot-shown`), d'une barre à
  l'autre (barre sous la souris), bande ↔ tuiles (zone en colonnes : `TileBox` de la barre,
  défilement compris ; seuls les widgets à panneau). Une zone de dépôt de plus : un `ZoneBox` dans
  `zone_boxes` (`columns: true`).
- **Menus** `src/ui/menu.rs` : `menu::show(hwnd, entrées, garder_activable, choisi)` avec
  `Entry::item`, `check`, `sub`, `with_icon` (icône Phosphor dessinée en pixels), séparateurs ;
  sombres si Windows l'est (comme muda/tao). `menu::requested()` : un widget a ouvert son propre
  menu pendant le clic droit en cours. `menu::popup` : menu simple (suggestions).
- Briques : `ui/dashed.slint` (`DashedBox`, contour en pointillés que Slint n'a pas : traits de
  3 px, coins arrondis pleins), `ui/edit-help.slint` (`EditHelp`), `ui/undo-bar.slint`
  (`UndoBar`, « « X » retiré · Annuler », logique `src/undoable.rs`, pour les fenêtres de
  réglages de la phase F ; essayée dans `--essai`, bouton « Retirer (essai) »).
- Écarts avec Tauri : pas d'oscillation des widgets (cadre fixe au survol), pas d'animation
  « FLIP » au dépôt, pas de fondu au bord des zones qui défilent en mode édition, pas de lueur du
  repère (rendu logiciel).

## Briques des widgets

Une seule brique par rôle ; tout widget passe par elles.

- **Sélecteur** `ui/widget-view.slint` (`WidgetView`) : le widget du genre de l'emplacement
  (`SlotView`, emplacement nommé, pour un genre pas encore porté). Il tient le masquage
  (`slot.hidden`), l'écart avant le widget (`slot.lead`) et annonce sa largeur naturelle
  (`measured`) ; une largeur nulle (widget vide) ne prend ni place ni écart (`ui/bar.rs`, comme
  `.slot:not(:has(*))`). Il passe à chaque widget les mêmes propriétés : `t`, `slot` (`id`,
  `data` = rang de l'exemplaire parmi ceux de son genre, `clickable` = `clicksOn` avec
  `clickableOnHover`…), `dim`, `interactive` (pas « Information seule »), `clickable`,
  `compact` (barre resserrée), `wide` (≥ 2 200 px logiques).
- **Cadre** `ui/widget-frame.slint` : `WidgetFrame`, dont un widget hérite (il en reçoit les
  propriétés ci-dessus) : îlot (style A) ou groupe (style C) centré dans la bande, `empty`
  (rien à montrer), `padding-x`, `gap`, `panel` / `pin` / `tip` / `has-action` / `activated`
  (voir Panneaux), `hovered`. `BarButton` : bouton intérieur (`enabled`, `tip`, `rest`,
  `hover`, `pressed`) ; désactivé, son clic passe au cadre. `StatusDot` : pastille d'état.
  Lecture en cours garde sa forme propre (fond qui suit le son dans l'îlot) avec les mêmes
  propriétés et `PanelArea`.
- **Accroche des panneaux** `ui/panels.slint` + `src/ui/panels.rs` : global `Panels`
  (`hover(genre, entré, x, l)`, `pin(genre, x, l)`, `settings(id)`) et `PanelArea` (zone qui
  l'appelle : survol si `interactive`, épinglage au clic si `clickable` hors des boutons actifs,
  infobulle, action propre `activated`). Genre : celui du widget (« media ») ou propre à un
  exemplaire (« weather:<id> »). `panels.rs` tient le délai d'aperçu (300 ms, aussitôt si un
  panneau voisin est ouvert), l'annule à la sortie et garde la dernière ancre de chaque panneau
  (un épinglage décidé par Rust, `panels::pin(label, genre, None)`, s'ouvre sous son widget) ;
  la fenêtre est menée par `src/ui/popup.rs` (voir « Panneaux »).
- **Mesure** `ui/gauge.slint` : `Eyebrow` (`.eyebrow` d'app.css ; style C : petites capitales
  imitées), `Track` (piste 0–1, `rounded` : bout arrondi), `Gauge` (lib/Gauge.svelte, 0–100, négatif = « — »).
- **Infobulles** `ui/tooltip.slint` + `src/ui/tooltip.rs` : `Tip.show(texte)` / `Tip.hide()`
  (info-bulle Windows sous la souris après 500 ms) ; déjà branchées dans `WidgetFrame.tip`,
  `PanelArea.tip`, `BarButton.tip`, `AppButton.tip`.
- **Bulle de molette** `ui/wheel-hint.slint` + `src/ui/hint.rs` : `hint::show(window, label, x,
  icône, texte, jauge)` (volume de la zone vide, ±10 s de la lecture).
- **Îlot dynamique** : service `src/alerts.rs` (`alerts::show(app, AlertKind, Alert)`, filtré
  par « Ne pas déranger » ; `Alert::icon` / `Alert::app`, `with_action`, `with_image` ;
  `LowBattery` pour les seuils de `[alerts]`), affichage `src/ui/island.rs` (durée 3,2 s ou
  `hold_ms`, fondu de sortie, activité d'un plugin qui dure, clic = action) et `ui/flash.slint`
  (global `FlashData`, le même dans toutes les barres ; `FlashCard` + `FlashGlow` dans l'îlot de
  Lecture en cours en style îlots, `FlashLine` dans le widget Notifications en style graphique
  ou sur une barre sans Lecture en cours : `FlashData.media-host`). Émetteurs : notifications,
  Claude attend, captures, casque, minuteur, fenêtre non déplacée, mise à jour, plugins
  (`horizon.alert`, `AlertKind::Plugin` : masquée pendant « Ne pas déranger »). Essai, instance
  en cours : `horizon.exe --alerte-essai notification|claude|batterie|minuteur|fenetre|capture`.
- **Témoins de la barre principale** `ui/bar-status.slint` + `src/ui/bar_status.rs` : profil
  actif autre que le premier, « ! » des intégrations en échec (`src/status.rs`) ; largeur
  comptée dans la zone gauche (`#status`).
- **Boutons d'applis** `ui/app-button.slint` : `AppButton` (icône teintée ou non, pastille
  « ouverte », Maj+clic, clic droit), `AppTray` (rangée, plaque en style C).
- Rust : `src/ui/model.rs` (`model::sync`, modèle mis à jour ligne par ligne),
  `src/ui/time.rs` (`ago`, `since`, `until` de lib/time.ts), `src/ui/format.rs` (`app_name`,
  `clock`), `src/ui/menu.rs` (menu contextuel Windows, barre activable le temps du menu),
  `src/ui/images.rs` (images chargées à la demande : icônes d'exe, favicons ; cache),
  `src/ui/icons.rs` (icônes choisies à l'exécution), `src/web.rs` (HTTP borné, lien ouvert
  dans le navigateur), `src/brands.rs` (logos Simple Icons), `src/wsclient.rs` +
  `src/relay.rs` (relais Discord).
- Catalogue `src/widgets/mod.rs` : `Backend { id, name, manage, start, bind, exit, panel }` de chaque
  widget porté dans `BACKENDS` (ordre alphabétique, vérifié par un test) ; `manage_all` avant
  les barres, `start_all` après le cœur et avant le premier plan (le widget d'un module intégré
  désactivé est sauté ; `start_one` le démarre s'il est activé ensuite), `bind_all` à la création
  de chaque barre (avant ses emplacements), `exit_all` à l'arrêt ; `instances`, `data_index`,
  `clicks_on`.

## Panneaux (fenêtre surgissante)

Une seule fenêtre layered pour tous les panneaux (`ui/popup.slint`, `PanelWindow`), menée par
`src/ui/popup.rs` ; règles testées dans `src/popup.rs` (reprises de `app/src-tauri/src/popup.rs`) :

- survol d'un widget = aperçu après 300 ms, sans focus (la fenêtre ne s'active pas, ses boutons
  marchent quand même) ; fermeture 250 ms après la sortie, sauf si la souris entre dans le
  panneau (l'écart de 6 px se traverse) ; fondu de sortie (150 ms) annulé si elle revient sur le
  panneau ou le widget ;
- passage d'un widget à l'autre : panneau gardé (`popup::keep`), le suivant s'affiche aussitôt
  à sa taille, en fondu, sans fermeture ;
- clic = épinglé : la fenêtre devient activable (`WindowSpec.focusable`, changé à chaud par
  `platform::configure`) et prend le premier plan (`bring_to_front`), le focus va au panneau
  (`focus-panel`) ; Tab passe d'un bouton à l'autre (anneau de focus au clavier seulement),
  Échap ou la désactivation (clic ailleurs : `platform::on_active`) ferme ; un survol ne
  remplace pas un panneau épinglé, le clic d'un autre widget si ;
- taille : celle du panneau (`PanelView.natural-w/h`) + marges 14 × 16 + bord, annoncée par la
  fenêtre (`measured`) et gardée par panneau : à la réouverture, affiché aussitôt à cette
  taille ; la première fois, la fenêtre est affichée transparente le temps d'être dessinée
  (Slint ne construit le panneau d'un `if` qu'au rendu), puis apparaît à sa mesure ; un panneau
  vide n'apparaît pas ; centrée sous le widget, sous la bande du haut
  (par-dessus les tuiles d'une barre haute), bornée à l'écran de la barre, à son échelle ;
- style de la barre d'origine (`t` : îlots ou graphique), fond `popup-bg`, bord `popup-border`,
  rayon `island-radius + 3px` ; fondu d'entrée (opacité, 4 px), jamais d'animation continue ;
  panneau fermé = déchargé (`kind` vide : ses éléments et minuteurs Slint disparaissent) ;
- ce que lisent les autres fils : `popup::PopupState` (`is_open("system")`,
  `is_open_kind("weather")`), `popup::screen(app)` (écran du dernier panneau) ; `autohide`
  compte la fenêtre parmi les nôtres.

### Fenêtres qui montrent les panneaux (`src/ui/panel_hosts.rs`)

Un panneau est montré par la fenêtre surgissante et par les tuiles des barres hautes ; chaque
fenêtre a son exemplaire des globaux Slint (`<Id>PanelData`). `panel_hosts` tient la liste
(`Host` : étiquette et fenêtre, `host.global::<X>()`, `is_popup()`, `inline()`) et appelle les
crochets du `widgets::Panel` de chaque widget :

- `bind(app, host)` : une fois par fenêtre (la surgissante à sa création, une barre au premier
  affichage d'une de ses tuiles) ; abonnements aux événements une seule fois, dans la
  surgissante (`if host.is_popup()`) ;
- `opened(app, genre)` / `closed(app, genre)` : le panneau du genre commence à être montré
  quelque part / ne l'est plus nulle part (une barre cachée en plein écran ne compte pas) ;
- `shown(app, host, genre)` / `hidden(app, host, genre)` : une fenêtre commence / cesse de le
  montrer, avec son exemplaire (« weather:w1 » ; une tuile a toujours le sien) — pour ce qui
  dépend de la fenêtre (Météo, Agenda : l'exemplaire ; Fenêtres : l'écran de la barre,
  `panel_hosts::screen`).

Écrire dans le panneau : `panel_hosts::each(genre, |h| …)` (la surgissante, toujours tenue à
jour, et les barres qui en ont une tuile), ou `panel_hosts::host(étiquette)` pour une seule.

### Barre haute à tuiles

`ui/bar.slint` (zone `tiles`), `ui/tile.slint`, `src/ui/bar.rs`, rangement pur `src/tiles.rs`
(testé). Une tuile par widget de la zone `tiles` qui a un panneau ; le panneau (`PanelFill`,
généré par `ui/build.rs` à côté de `PanelView`) prend la largeur de sa colonne. Chaque tuile
annonce sa hauteur naturelle (`tile-measured`) ; Rust range (`tiles::flow` : colonnes d'au
moins 330 px, écart 10 px, de haut en bas puis colonne suivante, équilibrées, une tuile jamais
coupée ni plus haute que la zone − 10 px) et pose `x`, `y`, `width`, `height` (`TileBox`). La
zone défile à la molette quand les colonnes dépassent (jamais le volume) ; fondu du bord d'où il
reste à défiler dessiné dans chaque tuile (pas de masque dans le rendu logiciel : le contenu
s'efface dans le fond de la tuile). Souris : cliquable, survol seul (la molette fait défiler le
panneau, rien d'autre ; `clickableOnHover` reste cliquable), information seule (rien). Mode
édition (`editing`, aspect seulement : cadre pointillé, tuiles inertes, « Ajouter une tuile » →
`add-tile`). Changements de disposition : `tiles::set_height`,
`add_tile`, `remove_tile` (puis `bars::sync_heights` et `ui::bar::refresh_all`). Essais :
`HORIZON_TILES_ESSAI` (voir `tiles::essai`, ex. `main+media;main+system;main=480;edit`,
`portrait:click;portrait+weather;portrait!`).

Volet (`BarLayout::tiles_mode` : `fixed` sous la barre, `click`, `hover`) : la place réservée
(`BarLayout::reserved_height`, `bars.rs`) n'est que la bande ; la **fenêtre** de la barre grandit
à l'ouverture (`slint::Window::set_size`, `ui::bar::open_drawer`) à la hauteur de ses tuiles
(`horizon_core::layout::drawer_height`, bornée à l'écran moins `DRAWER_MARGIN`), les tuiles
bornées à cette hauteur maximale et non à la fenêtre. Après un `ABM_POSCHANGED`,
`appbar::update` remet la hauteur réservée : `ui::bar::reapply_drawer` rend la sienne au volet.
Il ne prend jamais le focus : fermeture au clic ailleurs par le crochet
`horizon_sys::outside_click` (`WH_MOUSE_LL`, posé seulement tant qu'un volet est ouvert), au
bouton, au plein écran, à la fin du mode édition (qui l'ouvre) ; au survol, quand la souris
quitte la barre et les fenêtres d'Horizon (vérifié toutes les 250 ms, seulement volet ouvert au
survol). Volet fermé, ses tuiles ne sont pas montrées (`panel_hosts` : rien ne tourne).

### Briques des panneaux (`ui/panel/`, une par fichier)

| Fichier | Brique | Version Tauri |
| --- | --- | --- |
| `base.slint` | `PanelBase` : racine d'un panneau (colonne, `spacing` 12 px), reçoit `t`, `instance`, `pinned` | `.panel` |
| `head.slint` | `PanelHead` : titre « eyebrow » à gauche, complément en enfants à droite (`center` : centré, 18 px de haut) | `PanelHead` |
| `empty.slint` | `EmptyState` (`text`, `small`) | `EmptyState` |
| `error.slint` | `ErrorLine` (icône d'alerte `warn`, texte qui passe à la ligne) | `ErrorLine` |
| `button.slint` | `PanelButton` : icône et / ou texte et enfants ; `rest` / `hover` (fonds), `fg` / `hover-fg`, `radius`, `padding-x`, `gap`, police, `alignment`, `tip`, `enabled`, clavier (Entrée, Espace), `clicked`. `LinkButton` : petit texte estompé (`.more`, `.link`) | boutons des panneaux |
| `row.slint` | `ListRow` : ligne cliquable (survol, `on` : contour et fond dans l'accent, `radius`), enfants en ligne, clavier | `li` du panneau Audio, listes |
| `check.slint` | `CheckBox` : case à cocher et son libellé, contrôlée (`checked` ; `toggled(nouvelle valeur)`), souris et clavier (Espace, Entrée) | `<input type="checkbox">` |
| `slider.slint` | `Slider` : glissière contrôlée (`value`, `minimum`, `maximum`, `step`, `tint`, `enabled` ; `moved`, `released`), souris et clavier | `<input type="range">` |
| `volume.slint` | `VolumeSlider` : muet + glissière + « 52 % », molette ±2 %, valeur locale gardée 700 ms, envois limités à un par 60 ms (`label`, `volume`, `muted`, `max` ; `set-volume`, `set-mute`) | `VolumeSlider` |
| `scroll.slint` | `PanelScroll` : colonne qui défile au-delà de `limit` (440 px), barre fine, gouttière (`gutter`) | `max-height` + `overflow-y: auto` |
| `divider.slint` | `Divider` : filet `divider` | `border-top: 1px solid var(--divider)` |
| `focus.slint` | `FocusRing` : anneau du focus clavier, pour une brique nouvelle | `:focus-visible` |
| `field.slint` | `TextField` : champ de saisie (`text`, `placeholder`, `invalid` : contour rouge ; `edited`, `accepted` = Entrée), anneau dès qu'il a le focus ; ne se tape que panneau épinglé | `<input>` (durée libre du minuteur) |
| `check-row.slint` | `CheckRow` : case à cocher et son libellé posé en enfants, toute la ligne se clique (`checked`, `toggled(bool)`), clavier | `<label><input type="checkbox">` |

Toujours utiles : `Icon` (`components.slint`), `Eyebrow`, `Track`, `Gauge` (`gauge.slint`),
`StatusDot` (`widget-frame.slint`), `Tip` (infobulles, déjà branché pour la fenêtre du
panneau). Rust : `ui::model::sync`, `ui::time`, `ui::format`, `ui::icons::image`, `ui::images`,
`crate::mixer` (volumes, sorties, micros : `view`, `watch`, `set_*`, `set_default`, `pin`).
Une brique qui servirait à plusieurs panneaux et manque : la créer dans `ui/panel/` (un
fichier), l'ajouter à ce tableau, et le signaler aux autres lots. Vues en vrai avec le panneau
Lecture en cours : `PanelBase`, `PanelButton` (souris, Tab, anneau), `Divider`, `VolumeSlider`
(affichage seulement : aucun réglage du volume pendant les essais). Vues avec le panneau Audio
(et ceux d'Éclairage, d'Essaim et de Serveurs, devenus des plugins depuis) : `PanelHead`,
`ListRow` (survol), `PanelScroll` (molette dans la gouttière et sur les lignes, barre), `Slider`
et `CheckBox` (affichage). Compilées mais
pas encore essayées : `EmptyState`, `ErrorLine`, `LinkButton`, glissement du `Slider`, molette
d'un `VolumeSlider` dans un `PanelScroll` (vérifiée par le code : le curseur accepte l'événement,
la liste ne défile pas).

Travail en parallèle (plusieurs agents, un lot de panneaux chacun) : une copie du dépôt et une
cible de compilation par agent (`CARGO_TARGET_DIR` distinct), comme en phase B ; essais en vrai
un à la fois (un fichier verrou commun, hors des copies), Horizon installé relancé après chaque
essai. Les fichiers d'un panneau sont propres à son widget : seuls une brique
nouvelle ou un correctif de brique touchent un fichier commun.

### Ajouter le panneau d'un widget

Chaque panneau ne touche que les fichiers de son widget : **aucun fichier commun à modifier**
(`ui/build.rs` trouve `panel.slint`, génère `PanelView` — `$OUT_DIR/panel-view.slint` — et
réexporte ses globaux vers Rust — `$OUT_DIR/panels.slint` ; le `Backend` est dans le module du
widget). Modèle complet : Lecture en cours (`ui/widgets/media/panel.slint`,
`src/widgets/media/panel.rs`).

1. **Interface** `ui/widgets/<id>/panel.slint`, d'après `app/src/widgets/<id>/Panel.svelte` :
   `export component <Id>Panel inherits PanelBase { width: <largeur du .panel>; spacing: <gap>;
   … }` (`<Id>` : identifiant en CamelCase, `claude_sessions` → `ClaudeSessionsPanel`, vérifié par
   `ui/build.rs`). Il reçoit `t` (style de la barre), `instance` (« w1 » pour « weather:w1 ») et
   `pinned`. Ses données dans un global **du même fichier**, `<Id>PanelData` (structures
   `<Id>Panel…`) : tout `export global|struct|enum` du fichier est réexporté vers Rust, les noms
   doivent donc être uniques dans Horizon. Rien à montrer (`{#if …}` autour de tout le panneau)
   : largeur et hauteur nulles. Textes, infobulles (`tip`) et libellés en français, identiques
   à la version Tauri. Imports relatifs (`"../../panel/button.slint"`).
2. **Rust** `src/widgets/<id>/panel.rs` (le module passe en `<id>/mod.rs` au besoin) :
   `pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed),
   ..Panel::new(bind) }` (ou `Panel::new(bind)`), et dans le `BACKEND` du widget
   `panel: Some(panel::PANEL)`. Le panneau est montré par la fenêtre surgissante et par les
   tuiles des barres hautes (voir « Fenêtres qui montrent les panneaux »).
   - `bind(app, host)` : appelé une fois par fenêtre (avant toute ouverture). Remplir
     `host.global::<<Id>PanelData>()` d'après l'état du service, brancher les rappels vers les
     fonctions du service (les `send('…')` / `call('…')` du `Panel.svelte`) ; s'abonner aux
     événements du service une seule fois (`if host.is_popup() { host::listen(EV_…,
     Some(crate::popup::LABEL), …) }`), en écrivant dans chaque fenêtre
     (`panel_hosts::each("<id>", |h| …)`).
   - `opened(app, genre)` / `closed(app, genre)` : le panneau commence à être montré quelque
     part / ne l'est plus nulle part. Y mettre ce que faisait le montage du `Panel.svelte`
     (`$effect`, `useMixer()` → `mixer::watch(app, true)` et `mixer::watch(app, false)` à la
     fermeture, relecture) et le `panel_opened` du `Backend` Tauri ; lancer et arrêter ici les
     minuteurs du panneau (`every(…)` de `now.svelte.ts` → `slint::Timer`) : rien ne tourne
     panneau fermé.
   - `shown(app, host, kind)` / `hidden(app, host, kind)` : ce qui dépend de la fenêtre ;
     `kind` complet (« weather:w3 ») : poser les données de cet exemplaire dans cette fenêtre.
   - Un panneau montré en tuile ne ferme pas la fenêtre surgissante après une action
     (`host.inline()`).
3. **Ce que lit un autre fil** : `app.state::<crate::popup::PopupState>().is_open("system")`
   (mesures rapides panneau ouvert), `is_open_kind("weather")`.
4. **Vérifier** : `scripts\verifier.ps1` (clippy et tests ;
   `every_panel_is_bound_by_its_backend` vérifie `panel.slint` ↔
   `Backend::panel`), puis en vrai (voir « Lancer ») : survol (aperçu), clic (épinglé, Tab,
   Échap), passage depuis un widget voisin, style îlots (D1) et graphique (autres écrans), 4K à
   150 % ; captures BitBlt comparées au panneau Tauri (relancer l'installée, le capturer de la
   même façon). Ne cliquer dans un panneau que ce qui ne change aucun état réel de
   l'utilisateur.

Pièges propres aux panneaux :

- Une taille lue tout de suite après avoir changé un `if` (`get_panel_width()` après
  `set_kind`) est celle d'avant : le contenu d'un `if` n'existe qu'après un rendu. Se fier aux
  `changed … => measured(…)` de la fenêtre, pas à une lecture immédiate.
- Ni découpe à coins arrondis ni ombre portée dans le rendu logiciel (`clip` ignore le rayon,
  `drop-shadow` n'est pas dessiné) : arrondir l'image en Rust (`round_corners` de
  `widgets/media/mod.rs`), se passer d'ombre.
- `parent` n'est pas accessible à la racine d'un composant : une brique reçoit sa taille de
  l'élément qui la pose (voir `FocusRing`).
- Un texte qui passe à la ligne (`wrap`) posé dans une liste (`for`) ou une vue qui défile
  (`PanelScroll`) est mesuré sur une seule ligne tant que sa largeur vient de la mise en page :
  le panneau est trop court et son en-tête et son pied sont tassés. Lui donner une largeur fixe
  (celle du panneau moins les marges ; panneau Sessions Claude).
- Un `Text` de deux lignes au plus (`line-clamp: 2`) : `wrap: word-wrap; overflow: elide;
  max-lines: 2;` (une `max-height` laisse dessiner une troisième ligne).
- Un `TouchArea` posé au-dessus d'une zone (frère placé après elle) lui prend le survol et le
  clic : la zone perd `has-hover` (le panneau ne s'ouvrait pas sur le titre de la lecture, à
  cause du `TouchArea` du titre défilant). Un élément qui doit seulement savoir que la souris
  est là reçoit le survol de la zone (`Marquee.hovered: area.has-hover`) ; la zone de survol
  du panneau contient son cadre (`ui/popup.slint`).
- Les tuiles d'une barre haute posent les mêmes `<Id>Panel` dans la barre, à la largeur de la
  colonne (`width` imposé à l'usage) : garder le panneau indépendant de la fenêtre surgissante
  (ses données dans son global, pas de `PanelWindow` dans le `.slint`) et ne pas écrire sa
  largeur de fenêtre surgissante en dur dans ses éléments (`root.width`, voir Sessions Claude).
  Un global est un par fenêtre : deux tuiles du même widget propre à l'exemplaire (deux Météo)
  dans une même barre montrent le même exemplaire.

### Pièges (Slint 1.18, rendu logiciel)

- `@image-url` d'un SVG ou d'un PNG demande la fonction `image-decoders` de `i-slint-core`.
- `transform-scale` (et toute transformation) est ignoré par le rendu logiciel : un « scale »
  se fait en multipliant les tailles (`AppButton`, badge des Notifications, commandes de
  lecture).
- Une mise en page place en haut un élément de hauteur fixe, et le `parent` d'un élément écrit
  dans un widget est le cadre entier (40 px), pas l'îlot : centrer avec
  `y: (root.t.island-h - self.height) / 2`, ou dans un `VerticalLayout { alignment: center; }`.
- Une mise en page (`HorizontalLayout`…) n'a ni `opacity` ni fond : l'envelopper dans un
  `Rectangle`.
- `TouchArea.has-hover` d'un parent reste vrai au-dessus d'un bouton enfant ; un `TouchArea`
  activé garde le clic (le cadre ne l'épingle pas), désactivé il le laisse passer.
- Mesurer après `VecModel::set_vec` donne des largeurs périmées : passer par `measured`
  (largeur naturelle annoncée par l'interface).

## Ajouter (ou porter) un widget natif

Identifiant `<id>` : minuscules, chiffres et `_`, le même que dans `layout.json` et que le dossier
de la version Tauri. Un nouveau widget n'est posé sur aucune barre : il s'ajoute par le menu
« + » du mode édition. La barre de départ ne contient que l'essentiel (lecture en cours,
raccourcis, notifications, système, horloge : `horizon_core::layout::defaults`) et plus aucune
migration n'ajoute de widget aux barres existantes (`layout/migrations.rs`).

1. **Manifeste** : `ui/widgets/<id>/widget.json` (même contenu que
   `app/src/widgets/<id>/widget.json` : `label`, `icon`, `squeeze`, `clickableOnHover`).
   `build.rs` le trouve seul et vérifie l'icône ; le widget apparaît aussitôt dans
   les barres comme emplacement nommé (`SlotView`). Un widget qui grandit avec son nombre
   d'éléments (raccourcis, sessions Claude) déclare `"grows": <largeur minimale en px>` : barre
   trop chargée, il reçoit `slot.limit`, montre ce qui tient puis `MoreChip` (« +N », panneau au
   survol), et n'est jamais masqué tant qu'une limite suffit (`src/overflow.rs`, `limits`).
2. **Interface** : `ui/widgets/<id>/widget.slint`, `export component <Id>Widget inherits
   WidgetFrame` : ses éléments sont posés dans l'îlot ; `empty`, `panel` (`"<id>"`, ou
   `"<id>:" + root.slot.id` pour un panneau propre à l'exemplaire), `tip`, `activated` d'après
   le widget de la version Tauri (`use:hoverPanel`, `title`, `onclick`). Ses données dans un
   **global** du même fichier (`export global <Id>Data`, un exemplaire par fenêtre, exporté par
   `ui/main.slint`) ; ce qui varie d'un exemplaire à l'autre : modèle indexé par
   `root.slot.data`, ou `pure callback x(id) -> …` qui reçoit `root.slot.id`. Réglages :
   `Panels.settings(root.slot.id)`. Le brancher dans `ui/widget-view.slint` (sa ligne
   `if kind == "<id>"` et son genre dans `ported`). Panneau : voir « Ajouter le panneau d'un
   widget » ; réglages : une page `ui/widget-settings/<id>.slint` (global `<Id>SettingsData`)
   et son module `src/ui/widget_settings/<id>.rs`, branchés dans `page.slint` et `mod.rs`.
3. **Service** : `src/widgets/<id>.rs` (ou `<id>/mod.rs` + `ui.rs`), repris de
   `app/src-tauri/src/widgets/<id>.rs` : `use crate::host::{AppHandle, Manager, Emitter}` au
   lieu de `tauri`, `pub const BACKEND: Backend = Backend { id, name, manage, start, bind,
   ..Backend::NONE }` inscrit dans `BACKENDS`. Les `#[tauri::command]` deviennent des
   fonctions ; ses `EV_…` gardent leurs noms.
4. **Liaison** : `bind(app, window, label, bar)`, appelée pour chaque barre qui montre le
   widget : remplir son global (valeur initiale lue dans l'état du service, comme la page
   appelait sa commande `…_state` ; un rang par exemplaire dans l'ordre de
   `widgets::instances`), `host::listen(EV_<ID>, Some(label), |e| …)` pour les changements
   (`e.payload::<Payload>()`), rappels du global vers les fonctions du service.
5. **Règles** : rien ne se redessine sans changement (une propriété posée à la même valeur ne
   coûte rien ; un modèle se met à jour ligne par ligne, `model::sync`) ; ce qui suit le son
   est posé au plus 20 fois par seconde ; aucune animation sans fin ; jamais de `PopupWindow`
   (elle serait coupée par la barre : panneau = fenêtre surgissante, voir « Panneaux »).
6. **Vérifier** : `scripts\verifier.ps1`, puis l'exe en vrai (voir « Lancer ») et des captures BitBlt des quatre écrans
   comparées à la version Tauri.

## Modules intégrés (`src/modules.rs`)

Une fonction qui demande plus que ce qu'un plugin peut faire reste dans l'exe, mais ne tourne
que si elle est activée : page Plugins des réglages, rubrique « Modules intégrés » (tuiles de
`ui::plugin_tiles::module_tiles`, actions `module_enable:<id>` / `module_disable:<id>` de
`status::status_action`). Un module (`MODULES`) porte l'identifiant de son widget et de son
`Backend` ; aujourd'hui : `claude_sessions` et `phone` (voir « Téléphone »).

- `modules::resolve(app)` au démarrage, avant les barres et les services : choix de
  l'utilisateur (`[features] claude_sessions` de settings.toml), sinon activé seulement s'il
  sert déjà (son widget est sur une barre, tous profils, ou les hooks d'Horizon sont dans le
  `settings.json` de Claude Code). Une installation existante ne change donc pas ; une neuve ne
  lance rien.
- `modules::enabled(id)` : faux seulement pour un module désactivé. `widgets::manifest` ne
  connaît alors plus son widget (ni barres, ni menu « + », ni réglages, ni panneau) et
  `start_all` saute son service. À tester aussi là où le module se montre ailleurs : lignes de
  la page État (`status.rs`), réglages de la page Alertes (`OptionsData.claude-sessions`), hooks
  reçus (`claude_sessions::on_hook`).
- `modules::set(app, id, on)` (fil de l'interface) : choix enregistré, service démarré aussitôt
  (`widgets::start_one`) ou mis au repos (`claude_sessions::clear` : sessions oubliées, hooks
  ignorés ; ses fils ne sont plus lancés au démarrage suivant), barres et page État mises à
  jour.
- Ajouter un module : une entrée dans `MODULES`, son réglage `Option<bool>` dans
  `horizon_core::settings::features` (lu par `chosen`, écrit par `set`), ce qui prouve qu'il
  sert déjà dans `in_use`.

## Téléphone (`src/widgets/phone/`)

Module intégré qui reçoit l'appli Horizon Mobile. Protocole : `docs/PHONE.md` ; un message
d'exemple par type dans `crates/horizon-core/tests/fixtures/phone/`, que l'appli teste aussi.

- **Logique pure** (`horizon_core::phone`) : `proto` (messages), `pairing` (offre à usage
  unique, 5 min, sans limite d'essais : le jeton fait 128 bits ; contenu du QR), `registry` (téléphones appairés : seule l'empreinte
  du jeton est gardée), `battery` (`Book` : batteries reçues, `BatteryView`), `firewall`
  (décision d'après les règles lues, script `netsh`).
- **Système** (`horizon_sys`) : `firewall` (lecture COM des règles, pose par `netsh` élevé),
  `local_net` (adresses IPv4 locales, nom du PC).
- **Service** : `identity.rs` (clé du PC dans le Gestionnaire d'identifiants, certificat
  auto-signé, empreinte du SPKI), `server.rs` (TLS + WebSocket, un fil par session ; avant
  authentification : sondes TCP, 10 s en tout pour le premier message, 64 Kio par trame, 8 connexions en
  attente et 4 sessions au plus, adresses privées seulement ; un fil par session, réveillé toutes
  les secondes, toutes les 50 ms tant que le téléphone regarde la lecture ou le volume, pour que
  ce qui part vers lui n'attende pas), `mod.rs` (`Core` : appairage,
  sessions, batteries ; `refresh` : serveur en marche si le module est activé et que le pare-feu
  le permet ; `publish` : `EV_PHONE` = `Vec<BatteryView>` pour les barres, `EV_PHONE_LINK` =
  `LinkView` pour la page ; `set_right`), `ui.rs` (widget de barre, global `PhoneData`),
  `remote.rs` (télécommande : `Remote` tient les abonnements `watch` de chaque téléphone, lit
  ses droits au moment voulu, exécute les commandes par le trait `Controls` — en vrai
  `AppControls`, les fonctions de `widgets::media` et de `mixer` —, envoie l'état du média
  quand il change, la pochette en JPEG une fois par morceau, le volume, le mixeur
  (`mixer.state`) et les actions (`actions.state` : NPD, minuteur, casque) ; crochets
  `phone::media_changed` (`media::on_snapshot`), `phone::mixer_changed` (`mixer::publish`) et
  `phone::actions_changed` (`timer`, `dnd`, `widgets::headset`) ; rien n'est calculé si aucun
  téléphone ne regarde ; le mixeur ne relit l'état que tant qu'un téléphone regarde le volume ou
  le mixeur). Verrouiller : `horizon_sys::power::lock_workstation`. Saisie à distance
  (`witness.rs`) : `horizon_sys::input` (`SendInput`), débit et boutons tenus par téléphone dans
  `Remote`, séances (`horizon_core::phone::input::InputSessions`), alerte `AlertKind::RemoteInput`
  (passe le NPD) avec l'action `CutInput`, témoin `BarStatusData.input` (événement
  `horizon://phone-input`) dont le clic coupe.
  Logique pure de la télécommande : `horizon_core::phone::remote` (droits, commandes bornées,
  message d'état) ; droits par téléphone : `registry` (`granted`, `set_granted`).
- **Page** (`src/ui/phone_page.rs`, `ui/phone/page.slint`, `SettingsPage::Phone`, nom `phone`) :
  pare-feu, QR de l'appli, QR d'appairage (`qr_image`, décompte seulement pendant qu'une offre
  est affichée), téléphones (retrait différé par le bandeau d'annulation ; case *Télécommande*
  = droit `media`, case *Son et actions* = droit `sound`, case *Clavier et souris* = droit
  `input`), seuil
  `alerts.phone_low`. Ouverte par l'activation du module, la palette et les réglages du widget.
- **Fichiers** : `%APPDATA%\Horizon\phones.json` ; règle de pare-feu « Horizon Mobile » (une
  seule, pour l'exe en cours, profil privé).
- **Essai sans téléphone** : `crates/faux-telephone` joue l'appli (les tests du serveur s'en
  servent). En vrai : `horizon.exe --reglages phone`, *Afficher le code* (en debug, le lien du
  QR est écrit dans le journal), puis
  `cargo run -p faux-telephone -- "<lien>" 82 --charge [--boucle <secondes>]` ; le jeton reçu
  est gardé dans `%TEMP%\faux-telephone.json` pour les lancements suivants. La règle de pare-feu
  vaut pour un exe : celui de debug demande la sienne.
- **Essai avec la vraie appli, sans téléphone ni pare-feu** : un émulateur Android joint le PC
  par `10.0.2.2` (boucle locale). `cargo test -p horizon-natif serve_an_emulated_phone_by_hand
  -- --ignored --nocapture` monte le vrai serveur et le vrai `Core`, écrit le lien d'appairage à
  ouvrir dans l'émulateur (`adb shell am start -a android.intent.action.VIEW -d '<lien>'`), puis
  ce qui arrive. Les lignes ajoutées au fichier `HORIZON_ESSAI_CMD` sont exécutées : `accent
  #a06cd5`, `retirer`, `offre`, `fin`. La batterie de l'émulateur se change par `adb emu power
  capacity 37` et `adb emu power ac off`.

## Plugins (`src/plugins/`)

Hôte des plugins : `registry.rs` (dossier surveillé toutes les 500 ms, changement pris après
300 ms de calme, accord des permissions dans `plugins.json`), `slint_host.rs` (compilation par
`slint-interpreter`, bibliothèque `@horizon` servie depuis `ui/plugin-api/horizon.slint`,
imports vérifiés par l'analyseur de Slint, rendu hors écran), `view.rs` (exemplaires dans les
barres et les panneaux, global `PluginFrames` de `ui/plugin-view.slint`, 20 images/s au plus),
`js/` (QuickJS : un runtime par plugin, API `horizon`, entrées/sorties sous permissions),
`script.rs` (scripts chargés selon la disposition), `events.rs` (événements d'Horizon),
`convert.rs` (JSON ↔ valeurs Slint), `library.rs` (bibliothèque `KingsRequin/horizon-plugins` :
index, installation, mise à jour, retrait). Logique pure : `horizon_core::plugin` (manifeste,
permissions, réglages, budget, index et paquets de la bibliothèque). Essai réel sans toucher à
sa configuration : `APPDATA` et `HORIZON_PLUGINS_DIR` vers un dossier à part, après
`horizon.exe --quit`.

- **Propriétés standard** (`view.rs`, `apply_std`) : `compact`, `hovered`, `limit`,
  `bar-height` et `instance` sont posées sur l'exemplaire d'un widget si son composant les
  déclare. La barre les annonce par `PluginFrames.context` (`PluginSurface` de
  `ui/plugin-view.slint` : barre resserrée, souris dessus, `slot.limit`) ; l'exemplaire n'est
  redessiné que si l'une d'elles a changé.
- **Plugins attendus** (`Layout::pending_plugins`, `library.rs`) : une migration qui met un
  plugin à la place d'un widget intégré (`plugins-essaim-serveurs-quota` : Essaim, Serveurs,
  Quota Claude ; même exemplaire, mêmes réglages) le note dans `pending_plugins`
  (`Layout::migrate`). Au démarrage, `start_pending` le prend dans la bibliothèque et accepte
  d'office les permissions de son entrée de l'index (`Plugins::adopt` : la fonction tournait
  déjà avec ces droits). Hors ligne : nouvel essai après 1 min, 5 min, puis toutes les 30 min,
  et à chaque lancement. Un plugin déjà installé à la main, ou dont le widget a été retiré, sort
  de la liste sans rien installer.
