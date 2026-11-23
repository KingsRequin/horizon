# Horizon : consignes du projet

Barre d'outils modulaire en haut de chaque écran (Windows 10 et 11, un ou plusieurs écrans).
Rust + Slint 1.18 (rendu logiciel, fenêtres « layered »), un seul processus. Ce que fait l'appli
et comment l'installer : `README.md` (`README.en.md` en anglais). Mode d'emploi du code
(plateforme, hôte, thème, briques, panneaux, réglages, ajouter un widget, pièges de Slint) :
`natif/README.md`. Consignes propres à un poste (dossier de compilation, droits d'installer) :
`CLAUDE.local.md`, non suivi par git.

## Vérification (avant de dire « terminé »)

```
.\scripts\verifier.ps1          clippy (-D warnings) puis les tests de tout le workspace
.\scripts\verifier.ps1 -Exe     et l'exe de debug, pour le contrôle en vrai
```

En développement l'interface est **interprétée** (fonction cargo `live`, par défaut : le script,
une commande `cargo` sans option et rust-analyzer l'utilisent tous) : les `.slint` sont chargés
à l'exécution au lieu d'être traduits en Rust, ce qui était l'essentiel du temps de compilation.
À savoir :

- l'exe de debug relit les `.slint` du dépôt et les recharge quand ils changent (un changement de
  `.slint` se voit sans recompiler) ; il met quelques secondes à démarrer et occupe plus de
  1 Go : c'est normal dans ce mode ;
- l'exe publié reste la référence. Il est compilé par GitHub avec l'interface compilée
  (`--no-default-features`), que la CI vérifie à chaque poussée (ici : `.\scripts\verifier.ps1
  -Compile`) ;
- une compilation release demande donc `--no-default-features` (`scripts\package.ps1` et
  `scripts\install.ps1` le font) : sans lui, `natif/ui/build.rs` l'arrête avec ce message.

Un changement visible se vérifie aussi en vrai : `horizon.exe --quit` sur l'installée, lancer
`<dossier de compilation>\debug\horizon.exe`, provoquer la fonction (arguments
d'essai : `natif/README.md`, « Lancer »), captures BitBlt (CAPTUREBLT, processus
PER_MONITOR_AWARE_V2) des écrans concernés, puis `--quit` et relancer l'installée. Contrôler le
journal `%APPDATA%\Horizon\horizon.log` : pas de vérification « à l'aveugle ». La compilation
release dure 3 minutes après un changement de Rust, 10 après un changement de `.slint` (le code
que Slint génère) et plus de 20 depuis zéro : la lancer alors en processus détaché.

## Organisation

```
natif/                      l'appli (paquet horizon-natif, exe horizon.exe)
  build.rs                  catalogues (widgets, icônes), manifeste, icône
  horizon.exe.manifest      identité de paquet (notifications), DPI par écran
  ui/                       interface Slint : thème, briques, barres, panneaux, réglages… C'est
                            un paquet à part (horizon-ui : Cargo.toml, lib.rs, build.rs), pour
                            que le code que Slint génère ne soit recompilé que si un .slint change
    widgets/<id>/           UN DOSSIER PAR WIDGET (widget.json, widget.slint, panel.slint)
    widget-settings/        pages de réglages des widgets
    plugin-api/             bibliothèque Slint `@horizon` offerte aux plugins
  src/
    platform/               plateforme Slint maison (fenêtres Win32 layered, saisie, IME)
    host/                   AppHandle, Manager, Emitter, événements (API à la Tauri)
    *.rs                    cœur : barres, panneau, mode édition, profils, palette, modules
                            intégrés, minuteur, NPD, mixeur, relais Discord, réglages, page État…
    plugins/                hôte des plugins (dossiers chargés à chaud, scripts, bibliothèque)
    ui/                     liaison des fenêtres Slint
    widgets/<id>/           UN MODULE PAR WIDGET qui a un service propre (BACKENDS)
crates/
  horizon-core/             logique pure, testable partout (disposition, réglages, calculs)
  horizon-sys/              accès à Windows (API Win32/WinRT, audio, HID, fenêtres…) ; capture et
                            analyse du son = bibliothèque partagée audio-commun (dépôt à part,
                            avec Prisme et Essaim), à ne pas recopier ici
  horizon-hook/             exe appelé par les hooks de Claude Code
packaging/                  paquet « sparse » (identité de paquet) et ses icônes ; installateur
                            (horizon.iss, Inno Setup 6)
scripts/                    installation, désinstallation, paquet d'une Release, hooks Claude
.github/workflows/          vérification (ci.yml) et publication d'une Release (release.yml)
```

Règle de partage : ce qui ne sert qu'à un widget va dans son dossier (interface) et son module
(Rust) ; ce qui sert à plusieurs (minuteur, mixeur, NPD, relais Discord) reste au cœur.
La logique testable sans Windows va dans `horizon-core`, l'accès au système dans `horizon-sys` ;
le module du widget ne fait que relier les deux à l'interface.

## Ajouter un widget

Suivre `natif/README.md`, « Ajouter (ou porter) un widget natif » : manifeste
`ui/widgets/<id>/widget.json`, `widget.slint` (global `<Id>Data`), branchement dans
`ui/widget-view.slint`, service `src/widgets/<id>.rs` (`Backend`, inscrit dans `BACKENDS`),
liaison `bind`, panneau et page de réglages au besoin. Il s'ajoute par le menu « + » : Horizon ne
pose aucun widget d'office, ni sur une barre neuve (`layout/defaults.rs` : le socle seulement), ni
sur une barre existante (plus de migration d'ajout). Une constante `WidgetKind::<ID>` seulement si
la logique pure doit le nommer.

Avant d'écrire un widget natif, se demander s'il peut être un plugin (`docs/PLUGINS.md`, dépôt
`KingsRequin/horizon-plugins`) : c'est la règle pour tout ce qui ne sert pas à tout le monde. Ce
qui demande plus que l'API des plugins et ne sert pas à tous est un module intégré
(`natif/src/modules.rs`, comme Sessions Claude Code) : dans l'exe, éteint tant qu'il n'est pas
activé dans la page Plugins.

## Pièges connus

- Une seule édition. Ce qui ne sert qu'à une appli de l'auteur ou à un usage particulier (Prisme,
  SharkApp, Essaim, Serveurs, Quota Claude) est un plugin de la bibliothèque
  (`KingsRequin/horizon-plugins`), pas du code d'Horizon : `docs/PLUGINS.md`. Le relais
  BetterDiscord (KrispBridge, port 28642) reste un repli de Discord vocal et donne Krisp.
- Un widget intégré qui devient un plugin : migration qui change son genre en `plugin:<id>` à la
  même place (`layout/migrations.rs`) ; le plugin est alors noté dans `Layout::pending_plugins`
  et installé au démarrage (`natif/src/plugins/library.rs`). Le plugin doit être dans la
  bibliothèque AVANT la Release d'Horizon ; la vérification du dépôt des plugins, elle, ne passe
  qu'après (elle prend le dernier `horizon.exe` publié) : la relancer ensuite.
- API des plugins : tout ajout monte `API_MAX` (`crates/horizon-core/src/plugin/mod.rs`) et se
  documente « niveau N » dans `docs/PLUGINS.md` ; un plugin qui s'en sert déclare ce niveau.
- Mise à jour automatique (`natif/src/update.rs`, logique pure dans
  `crates/horizon-core/src/update.rs`) : seulement dans une compilation de `scripts/package.ps1`
  (`HORIZON_RELEASE`). Une Release doit joindre `horizon.exe`, `horizon-hook.exe` et
  `SHA256SUMS` (workflow `.github/workflows/release.yml`, déclenché par une étiquette `v*`).
- Nouveau casque : un module par famille dans `crates/horizon-core/src/headset/` (sa `FAMILY`,
  inscrite dans `FAMILIES`), testé sur faux appareil comme `centurion.rs`.
- Aucune animation sans fin ; ce qui suit le son est posé au plus 20 fois par seconde et
  seulement si la valeur change ; rien ne tourne quand c'est inutile (barre cachée, plein écran,
  session verrouillée).
- Jamais de `PopupWindow` Slint (coupée par la barre) : panneau = fenêtre surgissante.
- Rendu logiciel : ni ombre, ni masque, ni transformation (rotation, échelle animée) ; une mise
  en page dont la taille dépend de la largeur de la fenêtre fait une boucle de liaison (poser à la
  main). Autres pièges de Slint : `natif/README.md`.
- Toute nouvelle fenêtre appelle `ui::accent::follow(&fenêtre)` (accent, animations réduites).
- Jamais de courbe à rebond (`cubic-bezier(…, 1.2)`) sur une opacité : au-delà de 1, le rendu
  logiciel de Slint déborde en teintant une icône (panique en debug, alpha faux en release).
- Scripts Python en heredoc : `\a`, `\U`… corrompent les chemins Windows, et certains caractères
  (« … ») arrivent mal ; préférer l'outil d'édition. Écrire les fichiers en LF.
