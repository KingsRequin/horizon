# Releasing a version

1. Bump `version` in `Cargo.toml` (`[workspace.package]`) and commit.
2. Push a `v<version>` tag: `git tag v0.2.0 && git push origin v0.2.0`.
3. The `Release` workflow (`.github/workflows/release.yml`) checks that the tag matches the
   version, then runs two jobs side by side: clippy and the tests (`scripts\verifier.ps1`), and
   the build of the public edition with `scripts\package.ps1` (`HORIZON_RELEASE=1`, which turns
   the auto-updater on). When both succeed, it publishes the Release with:
   - `Horizon-Setup-<version>.exe` — first install (Inno Setup 6, `packaging\horizon.iss`;
     preinstalled on the GitHub runner, `winget install JRSoftware.InnoSetup` elsewhere);
   - `horizon.exe`, `horizon-hook.exe`, `SHA256SUMS` — what installed copies download.

Installed copies look for a new Release one minute after start and every 12 hours, verify the
SHA-256 of each file, and install it at the next start (or right away from the island or
*Réglages d'Horizon › État*). See `natif/src/update.rs`.

Both jobs start from the dependencies compiled by the `CI` workflow on `master`
(`.github/actions/cargo-cache`): push the version commit to `master` before, or with, the tag. A
dependency change is only cached once `CI` has finished on `master`; a tag pushed before that
still works, it just compiles everything.
