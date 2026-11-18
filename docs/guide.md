# Horizon — guide détaillé

*Présentation et installation : [README](../README.md). Développement : [CONTRIBUTING.md](../CONTRIBUTING.md).*

Ce guide décrit en détail chaque fonction, les fichiers de configuration et les casques reconnus.

## Ce que fait Horizon

- Une barre par écran, qui réserve sa place (les fenêtres maximisées commencent dessous) ;
  style « îlots » (écran principal) ou « graphique » (les autres), au choix par écran.
- **Modulaire** : clic droit sur une barre → « Modifier la barre » ; glisser les widgets, « + »
  pour en ajouter, clic droit sur un widget pour ses réglages, le retirer ou l'envoyer sur un
  autre écran ; Ctrl+Z annule, Échap termine. Tout est enregistré automatiquement.
- **Barre haute** (Réglages d'Horizon › Barres › Hauteur, ou clic droit › Hauteur : 240 à
  640 px, ou une autre entre 40 et 900) : la bande du haut reste la même, et dessous des
  **tuiles** montrent le panneau d'un widget (musique, audio, météo, système, sessions
  Claude…) en rangées. Elles s'ajoutent (« Ajouter une tuile », seulement les widgets qui ont
  un panneau), se retirent et se glissent entre la bande et les tuiles en mode édition. Revenue
  à la hauteur normale, la barre garde ses tuiles pour plus tard. Le panneau surgissant d'un
  widget de la bande s'ouvre sous la bande, par-dessus les tuiles.
- **Souris** : chaque barre est « Cliquable », « Survol seul » (panneaux au survol, clics sans
  effet : le défaut de l'écran du haut) ou « Information seule » (Réglages d'Horizon › Barres).
  Survol d'un widget = aperçu dans un panneau (passage d'un widget voisin à l'autre sans
  fermeture) ; clic = action du widget (ses boutons), sinon le panneau épinglé (il garde le
  focus : Tab, Échap ou un clic ailleurs le ferme) ; les actions destructrices (éteindre
  l'éclairage, quitter un salon) sont dans les panneaux ; un widget vide ou pas réglé ouvre ses
  réglages. Barre trop chargée pour son écran : titre du média et dernière notification
  raccourcis, puis les widgets qui grandissent avec leur nombre d'éléments (raccourcis,
  sessions Claude) sont limités : ils montrent ce qui tient et une pastille « +N », dont le
  survol ouvre un panneau avec tous les éléments. Aucun widget n'est masqué tant que ça suffit ;
  en tout dernier recours, des widgets secondaires le sont (suggestions, météo, éclairage…).
  Barre large (écran du haut) : serveurs avec leur nom, lieu de l'événement, prochaine remise à
  zéro du quota, ancienneté de la notification, message des sessions Claude.
- Widgets : lecture en cours (pochette, commandes, molette ±10 s avec bulle « +20 s · 2:43 »),
  raccourcis à icône libre (logo,
  image ou icône de l'appli sans flèche), notifications (et îlot dynamique), processeur / GPU /
  mémoire (courbes ou jauges, détail au survol), micro, horloge, sessions Claude Code,
  quota Claude, suggestions selon l'usage (applis utilisées, historique de Vivaldi), batterie du
  casque, météo (Open-Meteo, ville dans les réglages), agenda (adresses iCal secrètes, lien de
  visio), serveurs (`~/.ssh/config`), fenêtres ouvertes par écran, et
  [Essaim](https://github.com/KingsRequin/essaim) (curseur animé qui suit la musique : pause et
  réglages en direct ; le widget n'apparaît que quand Essaim tourne).
- **Raccourcis** : pastille sous l'icône quand l'appli a une fenêtre ouverte (relevé léger des
  fenêtres toutes les 2 s et à chaque changement de premier plan ; `.lnk`, exe et
  `shell:AppsFolder`, pas les URL ni les `.url`). Clic = l'appli ouverte revient au premier
  plan, sinon elle est lancée ; Maj+clic = nouvelle instance.
- **Ne pas déranger** (bouton du panneau des notifications, ou clic droit sur une barre → « Ne
  pas déranger » ; gardé dans `%APPDATA%\Horizon\dnd.json`) : l'îlot ne montre plus les
  notifications (elles restent dans le panneau) ; lune à la place de la cloche. « Réglages… » du
  widget Notifications : « Claude attend » et alertes d'état (batterie faible, serveur hors
  ligne) restent affichés (par défaut oui).
- **Notifications** : clic sur une notification du panneau = son appli (elle devient lue) ; au
  survol, « Écarter » (retirée aussi du centre de notifications de Windows) et « Couper »
  l'appli (plus d'îlot ni de compte dans Horizon ; liste « Coupées » en bas du panneau,
  `%APPDATA%\Horizon\notifs.json`). Sur une barre sans clic, la dernière notification de moins
  de 10 min s'affiche en texte, puis la cloche.
- **Micro** : le bouton coupe le micro par défaut de Windows (« Micro Windows (CABLE Output) —
  ouvert »).
- **Minuteur** (clic sur l'horloge) : préréglages (5, 15, 25, 50 min par défaut), durée libre
  (« 12 », « 1 h 30 » puis Entrée) ou pomodoro en boucle (durées dans Réglages d'Horizon ›
  Minuteur) ; compte à rebours à côté de l'horloge, pause, « +5 min », arrêt. Fin ou changement de phase : alerte dans l'îlot
  (même en NPD). Option « Ne pas déranger pendant la séance » (pas pendant les pauses du
  pomodoro). L'état est tenu par Rust (partagé par les barres).
- **Serveurs (homelab)** : hôtes lus dans `~/.ssh/config` (blocs `Host` sans joker ; seuls
  `HostName`, `Port` et `User` sont lus, jamais les clés), testés toutes les 60 s (et à la sortie
  de veille, à l'ouverture du panneau) par une connexion TCP (port de ssh, 22 par défaut) ou une
  requête HTTP(S). Pastille verte, orange au-delà de 300 ms, rouge hors ligne, et nombre d'hôtes
  hors ligne. Panneau (barre cliquable) : latence, dernier changement, « Terminal SSH » (Windows
  Terminal, sinon `ssh` dans une console) et interface web. Chute et retour annoncés dans l'îlot
  (hors ligne après deux échecs de suite). Réglages du widget : renommer, masquer, test et port,
  adresse web, serveurs ajoutés à la main. Ajouté une fois à la fin de la zone gauche de l'écran
  du haut (« Survol seul » : panneau au survol ; terminal SSH depuis une barre cliquable ou la
  palette).
- **Palette de commandes** : Ctrl+Alt+P (`[palette]` `hotkey` dans `settings.toml`, par
  exemple `"Win+Shift+P"` ; vide = désactivée ; relu au démarrage, raccourci déjà pris signalé
  dans le journal). Fenêtre au centre de l'écran de la souris : raccourcis des barres,
  suggestions, applis du menu Démarrer, profils, sorties audio et micros, micro, sortie audio
  suivante, sessions Claude (Entrée = terminal, Tab = oublier), « Ne pas déranger », minuteur
  (préréglages, pomodoro, pause, +5 min, arrêt), lecture / pause / suivant / précédent,
  recharger les barres, mode édition, réglages, quitter, terminal SSH des serveurs. Recherche
  floue (lettres dans l'ordre, débuts de mot, usage : `palette.json`). ↑↓, Entrée, Tab (action
  secondaire : interface web d'un serveur, emplacement d'une appli), Échap ou clic ailleurs ferme.
- **Sessions Claude Code** (hooks, voir Installation) : au survol du widget, panneau des
  sessions (projet, état travaille / attend / terminé et depuis quand, message d'attente ou
  début de la dernière réponse, branche git, fichiers modifiés, commits non poussés ; git relu à
  chaque fin de réponse et à l'ouverture du panneau). Clic sur une session (panneau ou pastille)
  → la fenêtre de son terminal revient au premier plan (le hook envoie la chaîne de ses
  processus parents et la fenêtre de sa console ; Windows Terminal, console classique, VS Code).
  Une session qui attend garde sa pastille « ● deskbar attend · 2 min » tant qu'elle attend ;
  les terminées ne sont que dans le panneau (sauf barre large). « × » dans le panneau oublie
  une session morte.
  **Autorisations** (hook `PermissionRequest`) : dès que Claude Code demande d'utiliser un
  outil, la pastille devient « deskbar demande Bash · 1 min » (bouclier orange), le panneau
  montre l'outil et la commande (ou le fichier, l'adresse), l'îlot l'annonce ; clic = terminal.
  Horizon observe seulement : son hook ne répond rien, la décision reste au terminal (ou à un
  autre hook lancé à côté). Page « État » : « Autorisations Claude » (dernière demande reçue).
  **Consommation** : le panneau montre pour chaque session ses jetons du jour (sous-agents
  compris) et le remplissage du contexte, puis le total du jour par projet. Lu dans les
  transcriptions `~/.claude/projects/…/<session>.jsonl` (format non documenté : approximatif ;
  jetons « neufs » = entrée + écriture du cache + sortie, relectures du cache à part ; fenêtre de
  contexte supposée de 200 k, ou 1 M dès qu'une session la dépasse). Diagnostic :
  `cargo run -p horizon-core --example claude_usage -- <fichier.jsonl>`.
  **Captures → Claude** (`[features] capture_to_claude`, actif par défaut) : une nouvelle image
  dans Images › Captures d'écran est proposée 15 s dans l'îlot (miniature, « Envoyer à
  deskbar ») pour la session qui attend, sinon la dernière terminée. Clic : son terminal revient
  au premier plan et le chemin y est collé (presse-papiers + Ctrl+V) ; jamais Entrée.
- **Bascule audio du casque** (Réglages d'Horizon › Fonctions ; désactivée tant qu'aucune sortie
  de repli n'est choisie) : casque éteint (deux lectures d'affilée, relu toutes les 5 s), la
  sortie par défaut passe sur la sortie de repli ; rallumé, elle revient à celle d'avant, sauf si
  une autre sortie a été choisie entre-temps. 15 s au moins entre deux bascules. L'îlot le dit.
- **Mode jeu** (`[features] game_mode`, actif par défaut) : appli plein écran sur l'écran
  principal → capture du son et spectre coupés, mesures système toutes les 6 s (sauf panneau
  Système ouvert), import Vivaldi retardé ; ligne « Mode jeu » dans la page « État ». Mesure par
  poste : `cargo run --release -p horizon-sys --example game_mode_cost`.
- **Alerte « Claude attend »** (réglages du widget) : quand une session attend ou a terminé,
  l'îlot de l'écran principal montre le projet et le message (clic = terminal au premier plan).
  Une seule alerte par attente.
- **Profils** : clic droit sur une barre → Profil. Chaque profil garde sa disposition ; une
  appli associée (fenêtre « Gérer les profils… ») active son profil tant qu'elle est au premier
  plan, le profil choisi revient 2 s après.
- Le fond suit la musique (halo ou spectre) dans la couleur de la pochette.
- **Couleur du fond d'écran** (`[features] wallpaper_accent`, `windows_accent`) : hors lecture,
  l'accent vient du fond de l'écran principal, relu chaque minute (`PrintWindow` du bureau avec
  `PW_RENDERFULLCONTENT` : Wallpaper Engine et ses listes de lecture compris, ~25 ms ; rien en
  mode jeu ni en absence). Il va aux barres et à l'accent de Windows ; l'accent d'origine de
  Windows est gardé dans
  `%APPDATA%\Horizon\accent-windows.json` et remis quand la fonction est coupée. Un fond animé ne
  change l'accent que si la teinte bouge vraiment (`accent::same_accent`).
- **Volume et média** (`[features] osd`, `osd_track`) : à la place de l'affichage de Windows,
  un îlot au centre de la barre principale montre le volume (changé ailleurs qu'Horizon :
  touches, autre appli), la touche de lecture pressée (crochet du shell `HSHELL_APPCOMMAND`, pas
  de crochet clavier) et chaque nouveau morceau. L'îlot XAML de Windows est poussé hors écran
  tant que la barre principale est visible ; il revient en plein écran, si la fonction est
  coupée et à la fermeture (pas après un plantage : redémarrer l'Explorateur le remet).
- La barre d'un écran se cache quand une fenêtre y est en plein écran ; elle revient si la
  souris reste 150 ms dans les 12 px du haut de l'écran (traverser vers l'écran du dessus ne
  la fait pas apparaître).
- Molette sur une zone vide d'une barre : volume général. L'écran du haut s'assombrit la nuit.
- **Panneau Audio** (survol du widget micro) : sortie et micro par défaut (clic = devient
  le défaut pour la console, le multimédia et les communications), épingle = favori en tête
  (les autres périphériques se replient derrière « N autres » ; favoris dans
  `%APPDATA%\Horizon\audio.json`), volume général et muet, volume et muet de chaque appli qui a
  une session audio (point vert = joue). Le panneau média montre le volume de l'appli qui joue
  quand sa lecture a pu être reliée à sa session audio (AUMID du raccourci du menu Démarrer, nom
  d'exe ; sinon pas de curseur). Molette sur un curseur : ±2 %. L'état n'est relu (toutes les
  400 ms) que pendant qu'un de ces panneaux est ouvert.
- Batterie faible du casque : annoncée une fois dans l'îlot dynamique quand le niveau passe
  sous le seuil (15 % par défaut, « Réglages… » du widget, ou aucune alerte) ; réarmée après une
  remontée de 5 points ou une charge.
- Sortie de veille : météo, agenda, quota et casque sont relus une fois le réseau revenu. La
  météo d'une ville n'est chargée qu'une fois pour toutes les barres et le panneau.
- **Mises à jour** (`[features] auto_update`, Réglages d'Horizon › Fonctions) : une version
  installée depuis les Releases cherche la dernière version sur GitHub une minute après le
  démarrage puis toutes les 12 h, télécharge `horizon.exe` et `horizon-hook.exe`, vérifie leurs
  empreintes (`SHA256SUMS` de la Release) et les installe au démarrage suivant. L'îlot l'annonce
  (clic : installer et redémarrer tout de suite) ; page « État », ligne « Mises à jour » :
  rechercher maintenant, ou installer. Si la nouvelle version ne peut pas être lancée,
  l'ancienne est remise. Une compilation depuis les sources ne se met pas à jour seule.

## Installation

Voir le [README](../README.md#installation) ; depuis les sources : [CONTRIBUTING.md](../CONTRIBUTING.md).

## Casques reconnus

Batterie lue directement (HID, sans le logiciel du fabricant), protocoles repris de Halo Battery
(MIT) : un module par famille dans `crates/horizon-core/src/headset/` (Logitech PRO X 2
Centurion et HID++ : G533, G535, G633, G635, G733, G933, G935, G PRO, G PRO X ; Astro A50 Gen 5 ;
SteelSeries Arctis 1, 7, 7+, 9, Pro, Nova 3, 5, 7, Pro, Elite, GameBuds ; HyperX Cloud II, II Core,
III, III S, Alpha 2 ; Razer BlackShark V2 Pro, Barracuda Pro ; Corsair Void v2, Virtuoso, HS80
Max ; Audeze Maxwell ; JBL Quantum 910). À défaut, casques et écouteurs Bluetooth dont Windows
connaît la batterie. Sonde : `cargo run -p horizon-sys --example headset_probe`.

## Configuration

`%APPDATA%\Horizon\` :
- `layout.json` : disposition (modifiée par le mode édition ; un fichier illisible est mis de
  côté en `layout.json.bak` et la disposition par défaut est reprise) ;
- `dnd.json` : « Ne pas déranger » activé à la main, option du minuteur ;
- `settings.toml` : `[night]` `start`, `end` (« 22:00 »), `opacity` (0–1) ; `[autohide]`
  `reveal_px` (hauteur de la zone de réapparition, px logiques, 200 au plus), `reveal_ms`
  (délai, 5000 au plus) ; `[palette]` `hotkey` (« Ctrl+Alt+P » par défaut ; Ctrl+Alt+Espace est souvent pris par la Command Palette de PowerToys). Créé seulement s'il n'existe pas ; jamais réécrit ensuite. Illisible,
  il est copié en `settings.toml.bak`, laissé en place, et les valeurs par défaut servent ;
- `palette.json` : nombre d'utilisations des entrées de la palette ;
- `audio.json` : sorties et micros favoris du panneau Audio ;
- `notifs.json` : applis dont les notifications sont coupées dans Horizon ;
- `usage.db` : base locale des suggestions (premier plan des applis, visites importées de
  Vivaldi) ;
- `tmp\` : copies temporaires de l'historique et des icônes de Vivaldi (lues sans le verrouiller) ;
- `horizon.log` : journal.

Ports locaux : récepteur des hooks `127.0.0.1:28660` ; client d'Essaim `ws://127.0.0.1:28670`.
