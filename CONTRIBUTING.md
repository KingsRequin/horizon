# Contributing to Horizon

Thanks for your interest! Issues and pull requests are welcome, in English or French.

## Build from source

Requirements: Windows 10 or 11, [Rust](https://rustup.rs) (the version is pinned in
`rust-toolchain.toml` and installed automatically by `rustup`), and the MSVC build tools.

```
scripts\verifier.ps1           # clippy and tests, with the Slint UI interpreted (fast; -Compile for the compiled UI)
cargo build -p horizon-natif --release
scripts\install.ps1            # build in release, install to %LOCALAPPDATA%\Programs\Horizon and start
scripts\uninstall.ps1          # remove it (settings are kept unless -RemoveConfig)
```

To try a build without installing it: `horizon.exe --quit` on the installed copy, run
`target\debug\horizon.exe`, then start the installed one again. Test arguments
(`--reglages`, `--palette-essai`, `--alerte-essai`…) are described in `natif/README.md`.

## Code layout

- `natif/` — the app (Rust + Slint). One folder per widget: `natif/ui/widgets/<id>/` for the
  interface, `natif/src/widgets/<id>/` for its service.
- `crates/horizon-core/` — pure logic, tested everywhere (layout, settings, parsers).
- `crates/horizon-sys/` — Windows access (Win32/WinRT, audio, HID…).
- `crates/horizon-hook/` — the small exe called by Claude Code hooks.

The full guide (platform, theme, panels, settings, adding a widget, Slint pitfalls) is in
`natif/README.md` (French).

## audio-commun

Sound capture and analysis come from [audio-commun](https://github.com/KingsRequin/audio-commun),
shared with [Essaim](https://github.com/KingsRequin/essaim). To change both at once, clone it next
to this repository and point to it in `.cargo/config.toml` (not tracked):

```toml
[patch."https://github.com/KingsRequin/audio-commun"]
audio-commun = { path = "../audio-commun" }
```

## Rules of thumb

- Performance first: no endless animation; what follows the sound is updated at most 20 times
  per second and only when the value changes; nothing runs while a bar is hidden.
- Check visible changes in the real app (screenshots), not only with tests.
- Releasing a version: `docs/RELEASING.md`.
