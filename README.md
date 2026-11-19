<div align="center">

<img src="packaging/icons/Square150x150Logo.png" width="96" alt="Logo d'Horizon">

# Horizon

**Une barre d'outils rapide et modulaire en haut de chaque écran, pour Windows 10 et 11.**

Musique, raccourcis, notifications, système, sessions Claude Code, météo, agenda…
dans une appli native qui utilise ~50 Mo de mémoire et se fait oublier.

[![Version](https://img.shields.io/github/v/release/KingsRequin/horizon?style=flat-square&label=version)](https://github.com/KingsRequin/horizon/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/KingsRequin/horizon/ci.yml?branch=master&style=flat-square&label=CI)](https://github.com/KingsRequin/horizon/actions/workflows/ci.yml)
[![Licence MIT](https://img.shields.io/badge/licence-MIT-blue?style=flat-square)](LICENSE)
![Windows 10 | 11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?style=flat-square&logo=windows)
![Rust](https://img.shields.io/badge/Rust-Slint-orange?style=flat-square&logo=rust)

[**Télécharger**](https://github.com/KingsRequin/horizon/releases/latest) ·
[Fonctions](#fonctions) ·
[Installation](#installation) ·
[Questions](#questions-fréquentes) ·
[English](README.en.md)

<img src="docs/images/hero.png" alt="Une barre haute d'Horizon : raccourcis, micro, système et horloge dans la bande ; musique, Claude Code, système et météo en tuiles" width="100%">

</div>

## Pourquoi Horizon ?

- **Natif et léger.** Rust + Slint, rendu logiciel, un seul processus : ~50 Mo de mémoire et
  ~1,5 % d'un cœur au repos pour quatre barres. Ni Electron, ni WebView.
- **Une barre par écran.** Elle réserve sa place (les fenêtres agrandies commencent dessous) et
  se cache quand une fenêtre passe en plein écran.
- **À ta façon.** Glisse les widgets, choisis un style par écran, agrandis une barre pour
  afficher des panneaux en tuiles, change de disposition selon l'appli au premier plan.
- **Se met à jour seule.** Les nouvelles versions viennent des Releases GitHub, vérifiées
  (SHA-256) et installées au démarrage suivant.

## Fonctions

**Elle suit la musique.** Pendant la lecture, les îlots s'éclairent avec les basses et flashent à
chaque grosse caisse, aux couleurs de la pochette (le style graphique montre un spectre). Au plus
20 mises à jour par seconde, et seulement quand quelque chose change.

<img src="docs/images/music.gif" alt="Barres qui pulsent avec la musique" width="100%">

<table>
<tr>
<td width="33%" valign="top"><img src="docs/images/media.png" alt="Panneau de lecture"></td>
<td width="33%" valign="top"><img src="docs/images/system.png" alt="Panneau système"></td>
<td width="33%" valign="top"><img src="docs/images/weather.png" alt="Panneau météo"></td>
</tr>
<tr>
<td valign="top"><b>Lecture en cours</b> : pochette, commandes, avance à la molette, volume par
appli. La barre s'illumine avec les basses, aux couleurs de la pochette.</td>
<td valign="top"><b>Système</b> : processeur, carte graphique et mémoire en courbes ou en jauges,
et les applis qui en consomment le plus.</td>
<td valign="top"><b>Météo</b> : maintenant, heure par heure et les jours suivants (Open-Meteo,
sans compte).</td>
</tr>
</table>

Survole un widget pour voir son panneau ; agrandis une barre et les panneaux deviennent des
**tuiles** (image du haut).

**Deux styles, au choix par écran** : « îlots » (en haut) ou « graphique » (en bas).

<img src="docs/images/bars.png" alt="Styles îlots et graphique" width="100%">

**Mode édition** : clic droit sur une barre → *Modifier la barre*. Glisse les widgets d'une zone
ou d'un écran à l'autre, <kbd>+</kbd> pour en ajouter, <kbd>Ctrl</kbd>+<kbd>Z</kbd> pour annuler.

<img src="docs/images/edit.png" alt="Mode édition" width="100%">

**Palette de commandes** (<kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>P</kbd>) : recherche floue dans tes
raccourcis, les applis du menu Démarrer, les sorties audio, le minuteur, les profils…

**Widgets**

| | |
|---|---|
| 🎵 **Lecture en cours** | Toute appli reliée aux commandes média de Windows (Spotify, navigateurs, YouTube Music…) |
| 🚀 **Raccourcis** | N'importe quelle icône (logos de marques, images, icônes d'applis), pastille quand l'appli est ouverte |
| 🔔 **Notifications** | Notifications de Windows avec îlot dynamique, Ne pas déranger, appli coupée |
| 📊 **Système** | Processeur, carte graphique, mémoire, en courbes ou en jauges |
| 🎙️ **Micro et audio** | Périphériques par défaut, favoris, volume par appli |
| 🤖 **Claude Code** | Sessions (travaille, attend, demande d'autorisation), consommation, terminal ramené au premier plan ; quota de l'abonnement |
| 🎧 **Batterie du casque** | Plus de 30 casques sans fil lus directement (Logitech, SteelSeries, HyperX, Razer, Corsair, Astro, Audeze, JBL) |
| 🌤️ **Météo**, 📅 **Agenda** | Open-Meteo ; toute adresse iCal (Google, Outlook…) |
| 🖥️ **Serveurs** | Hôtes de `~/.ssh/config`, état et latence, SSH en un clic |
| 🪟 **Fenêtres** | Fenêtres ouvertes par écran, en ramener une sur l'écran courant |
| ✨ **Suggestions** | Applis et sites que tu utilises à cette heure-ci |
| 🌀 **[Essaim](https://github.com/KingsRequin/essaim)** | Pause et réglages du curseur animé qui suit la musique |

**Et aussi :** minuteur et pomodoro, affichage du volume et du média à la place de celui de
Windows, couleur d'accent tirée du fond d'écran, mode jeu, profils de disposition par appli,
page « État » qui dit ce qui marche et ce qui ne marche pas.

Toutes les fonctions en détail : [docs/guide.md](docs/guide.md).

## Installation

1. Télécharge `Horizon-<version>.zip` depuis la [dernière version](https://github.com/KingsRequin/horizon/releases/latest) et extrais-le.
2. Double-clique sur **`Installer.cmd`**. Horizon est copié dans `%LOCALAPPDATA%\Programs\Horizon`,
   démarre avec Windows et apparaît dans le menu Démarrer. Pas besoin de droits administrateur.
3. *(Facultatif)* Pour afficher les notifications des autres applis, active le **mode
   développeur** de Windows (Paramètres → Système → Espace développeurs), puis relance
   `Installer.cmd`.

Ensuite : clic droit sur une barre → **Modifier la barre**.

Désinstaller : `Desinstaller.cmd` (avec `-RemoveConfig` pour effacer aussi tes réglages).

## Questions fréquentes

<details>
<summary><b>Windows affiche « Windows a protégé votre ordinateur ».</b></summary>

Horizon n'est pas signé (un certificat coûte cher pour un projet gratuit). Clique sur
*Informations complémentaires* → *Exécuter quand même*. Le code est ici, et chaque version est
compilée par GitHub Actions à partir de ce dépôt.
</details>

<details>
<summary><b>Pourquoi le mode développeur pour les notifications ?</b></summary>

Windows ne laisse lire les notifications qu'aux applis qui ont une *identité de paquet*.
Horizon en enregistre une sans signature, ce que Windows n'accepte qu'en mode développeur.
Détails : [docs/notifications.md](docs/notifications.md).
</details>

<details>
<summary><b>Est-ce que ça modifie ma configuration de Claude Code ?</b></summary>

Si Claude Code est installé, l'installation ajoute les hooks d'Horizon dans
`%USERPROFILE%\.claude\settings.json` (une copie de sauvegarde est gardée). Ils ne font que
signaler les événements à Horizon et ne répondent jamais aux demandes d'autorisation.
`Installer.cmd -NoClaudeHooks` pour s'en passer.
</details>

<details>
<summary><b>Où sont mes réglages ?</b></summary>

Dans `%APPDATA%\Horizon` (`layout.json`, `settings.toml`, journal). La plupart des options sont
dans le menu du clic droit sur une barre → *Réglages d'Horizon*.
</details>

## Feuille de route

- **Plugins** : des widgets dans leur propre dossier (interface Slint + JavaScript), chargés
  sans redémarrer (préversion disponible, voir le [guide](docs/guide.md#plugins-préversion)) ;
  à venir : une bibliothèque sur GitHub et un guide pour les écrire avec une IA.
- **Discord vocal** par le RPC local officiel (qui parle, micro coupé, volume par personne).
- **Traductions.**

## Fait avec Claude Code

Horizon a été entièrement écrit avec [Claude Code](https://claude.com/claude-code), l'agent de
programmation d'Anthropic. [KingsRequin](https://github.com/KingsRequin) l'a conçu et a supervisé
tout le travail : chaque fonction a été décidée, relue et testée sur une vraie installation à
quatre écrans avant d'être gardée.

## Contribuer

Bugs et idées : [issues](https://github.com/KingsRequin/horizon/issues). Compiler et organisation
du code : [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

[MIT](LICENSE). Icônes, polices et protocoles de casques repris : voir
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
