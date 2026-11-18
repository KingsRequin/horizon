# Horizon

*[Français](README.fr.md)*

A modular toolbar at the top of every screen, for Windows 10 and 11 (one or several monitors,
mixed scaling). Written in Rust with [Slint](https://slint.dev) (software rendering, layered
windows): one process, ~50 MB of memory, ~1.5 % of one core at rest, no WebView2.

> The interface is in French only for now. Contributions (translations included) are welcome.

## Features

- **One bar per screen** that reserves its space (maximized windows start below it), in
  "islands" or "graphic" style, clickable, hover-only or information-only. It hides when a
  window goes fullscreen and comes back when the mouse rests at the top edge.
- **Modular**: right-click a bar → *Modifier la barre*, drag widgets around, *+* to add one,
  right-click a widget for its settings, to remove it or to send it to another screen; Ctrl+Z
  undoes. **Tall bars** show widget panels as tiles below the strip. **Profiles** switch the
  layout automatically while a given app is in the foreground.
- **Widgets**: now playing (cover, controls, scroll to seek, the bar glows with the bass),
  shortcuts with any icon (brand logos, images, app icons) and an "open" dot, Windows
  notifications with a dynamic island, CPU / GPU / memory, microphone and audio panel (default
  devices, per-app volume), clock with timer and pomodoro, Claude Code sessions (hooks: waiting,
  permission requests, token usage, bring the terminal to the front), Claude quota, suggestions
  from usage (apps, Vivaldi history), headset battery (30+ wireless headsets read over HID, see
  `README.fr.md`), weather (Open-Meteo), calendar (iCal), servers from `~/.ssh/config`, windows
  per screen, and [Essaim](https://github.com/KingsRequin/essaim) (music-reactive cursor).
- **Command palette** (Ctrl+Alt+P), **Do not disturb**, volume / media on-screen display in
  place of Windows', accent color taken from the album cover or the wallpaper, game mode.
- **Automatic updates** from GitHub Releases (verified with SHA-256, installed at the next start).

## Install

1. Download `Horizon-<version>.zip` from [Releases](https://github.com/KingsRequin/horizon/releases)
   and extract it.
2. Double-click `Installer.cmd`. Horizon goes to `%LOCALAPPDATA%\Programs\Horizon`, starts with
   Windows and appears in the Start menu. No administrator rights needed. The exe is not signed:
   on "Windows protected your PC", choose "More info" → "Run anyway".
3. To read other apps' notifications, turn on Windows developer mode (Settings → System → For
   developers) and run `Installer.cmd` again (package identity without signing, see
   `docs/notifications.md`). Everything else works without it.

Claude Code users: the installer adds Horizon's hooks to `%USERPROFILE%\.claude\settings.json`
(a backup is kept next to it); run `Installer.cmd -NoClaudeHooks` to skip them. Uninstall with
`Desinstaller.cmd` (`-RemoveConfig -RemoveClaudeHooks` removes everything).

Updates: Horizon checks GitHub one minute after start and every 12 hours, downloads and verifies
the new version, then installs it at the next start; the island and *Réglages d'Horizon › État*
offer to install it right away. Turn it off in *Réglages d'Horizon › Fonctions*.

## Build from source

Rust 1.90 or newer, on Windows.

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts\install.ps1            # build in release, install and start
```

Sound capture and analysis come from [audio-commun](https://github.com/KingsRequin/audio-commun),
shared with Essaim. To work on both at once, clone it next to this repository and point to it in
`.cargo/config.toml` (not tracked):

```toml
[patch."https://github.com/KingsRequin/audio-commun"]
audio-commun = { path = "../audio-commun" }
```

Code layout and how to add a widget: `natif/README.md` (one folder per widget:
`natif/ui/widgets/<id>/` for the interface, `natif/src/widgets/<id>/` for its service).

### Personal edition

A few widgets talk to the author's own, unpublished apps (Prisme RGB engine, SharkApp phone
agent, a BetterDiscord voice relay). They sit behind the Cargo feature `personal`, which neither
the Releases nor a default build include (`scripts\install.ps1 -Personal` builds it).

### Releasing

Bump `version` in `Cargo.toml` (`[workspace.package]`), commit, then push a `v<version>` tag. The
`Release` workflow builds the public edition with `scripts\package.ps1` and publishes the zip,
`horizon.exe`, `horizon-hook.exe` and `SHA256SUMS`, which installed copies pick up.

## License

[MIT](LICENSE). Bundled icons, fonts and adapted headset protocols: see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
