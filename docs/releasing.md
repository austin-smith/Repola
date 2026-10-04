# Releasing Repola

Stable tags (`vX.Y.Z`) create drafts. Nightlies (`X.Y.Z-nightly.<run-number>`) publish daily at 09:17 UTC or through **Release → Run workflow → main → Publish nightly**. Unchanged nightlies are skipped. Both channels require passing three-platform CI at the source commit on `main`.

Desktop releases support macOS arm64 (DMG), Linux x64 (AppImage and Debian package), and Windows x64 (NSIS), with matching agents for each target plus Linux arm64.

## Build installers

Run **Release → Run workflow → <branch> → Build installers**. Download the installers from the run's **Artifacts** section. Builds use the release signing credentials and verify macOS/Windows signing and updater signatures. They do not publish releases or enable updates.

## Setup

Use **GitHub Actions** as the GitHub Pages source for update feeds. Restrict `stable-release` and `nightly-release` environments to `main`, and require reviewers for stable. Protect `main` and stable tags.

Configure these repository secrets and variables:

| Configuration | Names |
| --- | --- |
| Updater | Variable `REPOLA_SIGNING_PUBLIC_KEY`; secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Apple signing | Secrets `APPLE_CERT_P12_BASE64` (base64 `.p12` including its private key), `APPLE_CERT_PASSWORD` (export password) |
| Apple notarization | Secrets `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`, `APPLE_API_PRIVATE_KEY_BASE64` (base64 `.p8` contents) |
| Windows signing | Variables `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`, `AZURE_TRUSTED_SIGNING_ENDPOINT`, `AZURE_TRUSTED_SIGNING_ACCOUNT_NAME`, `AZURE_TRUSTED_SIGNING_CERTIFICATE_PROFILE_NAME` |

Generate the updater key with `pnpm tauri signer generate -w <private-key-path>` and a nonempty password. Store the exact one-line `.pub` contents in the public-key variable and the private-key contents in its secret.

## Publish stable

1. Set the version in `package.json`, `src-tauri/tauri.conf.json`, `[workspace.package]` in `src-tauri/Cargo.toml`, and both workspace packages in `src-tauri/Cargo.lock`. Merge to `main`.
2. Run `pnpm release` from any checkout with an authenticated GitHub CLI (`gh`). It reads the version from GitHub's current `main`, checks all version files, waits for that commit's CI, and creates the matching annotated `vX.Y.Z` tag to build a draft. It refuses existing tags and stops if `main` advances. Use `pnpm release --dry-run` to check the target without creating a tag. Wait for the draft build to finish.
3. Review the notes and smoke-test installation, updates, and SSH agent bootstrap on supported platforms.
4. Run **Publish release** from `main` with the draft tag and channel `stable`, then approve the environment.
5. Check the feed and an installed client's update. Bump `main` to the next stable version before another nightly.

Feeds are `https://austin-smith.github.io/Repola/updates/stable.json` and `https://austin-smith.github.io/Repola/updates/nightly.json`.

Both channels share application identity and settings. Switch by installing the desired channel. Back up settings before installing an older version.

## Recovery

Rerun failed build jobs to resume a draft. If feed deployment fails after publication, rerun **Publish release** with the same tag and channel. Preserve published tags and assets for older desktops' matching agents. Fix faulty releases with a higher version.
