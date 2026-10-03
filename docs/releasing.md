# Releasing Repola

Stable tags (`vX.Y.Z`) build a draft for review. Nightlies (`X.Y.Z-nightly.<run-number>`) publish daily at 09:17 UTC, or through **Release → Run workflow → main → Publish nightly**. Unchanged nightlies are skipped. Both channels require successful Linux, macOS, and Windows CI at the exact source commit on `main`.

Desktop releases support macOS Apple Silicon and Intel (DMG), Linux x64 (AppImage and Debian package), and Windows x64 (NSIS), with matching agents for each target plus Linux ARM64.

## Build test installers

Run **Build installers** on the desired branch and download the installers from the run's **Artifacts** section. These builds do not publish releases or enable automatic updates. You can also build any branch through **Release** with **Build installers** selected:

```bash
gh workflow run release.yml --ref <branch> -f operation='Build installers'
```

## Setup

Enable GitHub Pages with **GitHub Actions** as its source; the site is dedicated to update feeds. Create `stable-release` and `nightly-release` environments restricted to `main`, with required reviewers for stable. Protect `main` and stable tags.

Configure these repository secrets and variables:

| Configuration | Names |
| --- | --- |
| Updater | Variable `REPOLA_SIGNING_PUBLIC_KEY`; secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Apple signing | Secrets `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (Developer ID Application) |
| Apple notarization | Secrets `APPLE_ID`, `APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID`; or `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY` (`.p8` contents) |

Generate the updater key with `pnpm tauri signer generate -w <private-key-path>`. Use the exact one-line `.pub` contents as the public-key variable and the private-key contents as its secret. Set a nonempty password and keep an offline backup.

## Publish stable

1. Set the version in `package.json`, `src-tauri/tauri.conf.json`, `[workspace.package]` in `src-tauri/Cargo.toml`, and both workspace packages in `src-tauri/Cargo.lock`. Merge and wait for green CI.
2. Push the matching `vX.Y.Z` tag at that commit. Wait for the Release workflow to create the draft.
3. Review the notes and smoke-test installation, updates, and SSH agent bootstrap on supported platforms.
4. Run **Publish release** from `main`, with the draft tag and channel `stable`. Approve the environment when prompted.
5. Verify the update feed and an installed client's update check. Bump `main` to the next intended stable version before another nightly.

Feeds are `https://austin-smith.github.io/Repola/updates/stable.json` and `https://austin-smith.github.io/Repola/updates/nightly.json`.

Both channels share application identity and settings. Switch by installing the desired channel; returning to an older version requires a deliberate reinstall and a settings backup.

## Recovery

Rerun failed build jobs to resume a draft. If publication succeeded but feed deployment failed, rerun **Publish release** with the same tag and channel. Keep published tags and assets unchanged and available: older desktops require their exact agent version. Fix a faulty published release with a higher version.
