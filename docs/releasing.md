# Releasing Repola

Release tags use the exact application version: `v0.1.0`. The release workflow builds native desktop bundles and a matching `repola-agent` for macOS Apple Silicon, macOS Intel, Linux x64, and Windows x64, plus an agent-only Linux ARM64 artifact for common remote servers and development boards. Every agent asset has a SHA-256 checksum, a Minisign signature from Repola's pinned updater key, and a keyless Sigstore bundle tied to this repository's GitHub Actions identity.

The GitHub release is intentionally created as a draft. A maintainer must verify every platform artifact and publish it explicitly.

## Required release trust

Desktop code-signing and updater identities are security boundaries, not defaults to fabricate in source control.

- Configure Apple certificate/notarization secrets before publishing macOS artifacts.
- Configure Windows Authenticode signing in the release environment before publishing Windows artifacts.
- Generate a Tauri updater key with `pnpm tauri signer generate -w ~/.tauri/repola.key` on a protected maintainer machine.
- Store the private updater key and password only as `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` release secrets.
- Store the exact one-line contents of the generated `.pub` file as the `REPOLA_SIGNING_PUBLIC_KEY` GitHub Actions variable. Tauri encodes the complete Minisign public-key document as that base64 line; do not decode or reformat it. Release builds compile the same value into both the updater configuration and the remote-agent verifier.
- Configure `REPOLA_WINDOWS_SIGN_COMMAND` as the protected Actions variable containing the approved Tauri Authenticode signing command for the chosen certificate provider.

For Apple notarization, configure either `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`, or configure `APPLE_API_KEY`, `APPLE_API_ISSUER`, and the downloaded `.p8` contents as `APPLE_API_PRIVATE_KEY`. The workflow writes the `.p8` to a mode-`0600` runner-temporary file, exposes only its path to Tauri, and removes it in an `always()` cleanup step. Never commit the private key or store a local filesystem path in Actions.

`scripts/prepare-release.mjs` fails a release before compilation when the repository, tag, package/Cargo/Tauri versions, updater trust, Apple signing/notarization inputs, or Windows signing command are incomplete. It enables Tauri v2 updater artifact generation only in that validated release configuration. Desktop publication is intentionally serialized because each platform safely merges its signed entry into the same `latest.json` release asset. Local development builds intentionally have no update endpoint or public key; they cannot advertise, install, or bootstrap from unsigned release material.

## Agent verification policy

Remote bootstrap downloads only the exact target and desktop version from `austin-smith/repola`. It bounds every response, verifies the named SHA-256 checksum, decodes Tauri's signed-artifact envelope, rejects legacy Minisign algorithms, verifies the artifact signature against the public key compiled into the signed desktop, uploads through system OpenSSH, atomically replaces the per-user agent, and repeats the exact-version handshake before sending the original operation. Sigstore bundles remain published as independent provenance evidence for maintainers and external verification.

The installed desktop checks the signed `latest.json` channel automatically and from Settings. Installation exposes download progress and relaunches only after Tauri verifies and installs the signed native update.
