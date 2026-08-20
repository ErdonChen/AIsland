# Windows signed releases

AIsland publishes Windows updates from `.github/workflows/release-windows.yml`.
The workflow accepts only an existing `v<semver>` tag whose version matches
`package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`.

This workflow is only for stable, Authenticode-signed releases. A narrowly
scoped unsigned installer may be published for technical testing by following
`docs/unsigned-preview-release.md` and the separate manual workflow at
`.github/workflows/release-windows-preview.yml`. It must use a `preview-v*`
tag, remain a GitHub Pre-release, and stay outside the stable updater channel.
Never weaken this signed workflow to publish an unsigned preview. Tauri updater
signatures do not replace Authenticode.

## One-time repository setup

1. Generate a password-protected Tauri updater key pair on a trusted machine:

   ```powershell
   pnpm tauri signer generate -w "$env:USERPROFILE\.tauri\aisland.key"
   ```

2. Keep the private key and its password outside the repository. Add their
   values as GitHub Actions repository secrets named
   `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Put only the generated public key in `plugins.updater.pubkey` in
   `src-tauri/tauri.conf.json`.
4. Configure the updater endpoint as:

   ```text
   https://github.com/ErdonChen/AIsland/releases/latest/download/latest.json
   ```

5. Set `bundle.createUpdaterArtifacts` to `true`. The release workflow fails
   before building when any required secret or updater setting is missing.
6. Apply to SignPath Foundation first. If it accepts AIsland, implement and
   review its trusted-build flow in a separate change while keeping
   Authenticode ahead of final updater signing. Otherwise, provision SSL.com IV
   + eSigner CKA (or another reviewed Windows-store provider) on the Windows
   runner so its code-signing certificate is visible in `Cert:\CurrentUser\My`
   without exporting the private key. See `docs/code-signing.md` and the dated
   research note linked from it.
7. Add the certificate SHA-1 thumbprint as the repository secret
   `AUTHENTICODE_CERTIFICATE_SHA1`. Add the provider's RFC 3161 URL as the
   repository variable `AUTHENTICODE_TIMESTAMP_URL`.
8. For a Windows-store provider, the stable workflow merges
   `src-tauri/tauri.signed.conf.json`, whose Tauri
   `signCommand` invokes `.github/scripts/sign-windows-artifact.ps1`. The wrapper
   uses SignTool with SHA-256, applies the RFC 3161 timestamp, and fails unless
   `Get-AuthenticodeSignature` reports `Valid` with both signer and timestamp
   certificates. Authenticode therefore finishes before Tauri creates the final
   updater signature and `latest.json`.
9. Run the workflow with `publish=false` and verify the provider, Authenticode,
   updater, and draft-asset checks. Only after this dry run passes, create the
   repository variable `AUTHENTICODE_RELEASE_ENABLED` with the exact value
   `true`.

Back up the private key securely. Losing it prevents existing installations
from accepting future updates. Never commit the private key or print it in CI.

The repository does not install or authenticate a signing provider by itself.
Purchasing, identity verification, provider enrollment, MFA, and GitHub secret
creation must be completed by the maintainer. Keep the stable release gate off
until that provider-specific setup has been reviewed and pinned.

## Publishing a version

1. Set the same semantic version in all three version files.
2. Merge and verify the release commit.
3. Create and push its version tag, for example:

   ```powershell
   git tag -a v0.1.0 -m "AIsland v0.1.0"
   git push origin v0.1.0
   ```

The workflow can also be started manually with an existing tag from the
Actions tab. It runs frontend and Rust tests, builds the NSIS installer and
signature, and creates a draft release. The release is made public and marked
latest only after the installer has a valid trusted Authenticode signature and
timestamp, and after `.exe.sig` plus signed `latest.json` have been verified.
A failed build or verification leaves no public updater metadata.
