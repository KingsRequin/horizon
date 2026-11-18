# Releasing a version

1. Bump `version` in `Cargo.toml` (`[workspace.package]`) and commit.
2. Push a `v<version>` tag: `git tag v0.2.0 && git push origin v0.2.0`.
3. The `Release` workflow (`.github/workflows/release.yml`) checks that the tag matches the
   version, runs the tests, builds the public edition with `scripts\package.ps1`
   (`HORIZON_RELEASE=1`, which turns the auto-updater on) and publishes the Release with:
   - `Horizon-<version>.zip` — first install (`Installer.cmd`);
   - `horizon.exe`, `horizon-hook.exe`, `SHA256SUMS` — what installed copies download.

Installed copies look for a new Release one minute after start and every 12 hours, verify the
SHA-256 of each file, and install it at the next start (or right away from the island or
*Réglages d'Horizon › État*). See `natif/src/update.rs`.
