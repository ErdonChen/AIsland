# Windows code-signing policy

## Current distribution status

AIsland may publish an unsigned Windows installer for technical evaluation under the narrow preview exception below. An unsigned preview is not a stable release and is not intended for ordinary users.

A stable Windows installer, a release marked `Latest`, or a portable executable is allowed only after the final artifacts receive a trusted Authenticode signature from a suitable certificate provider and the signed release workflow verifies that signature. The Tauri updater signature is a separate integrity mechanism and does not replace Authenticode trust.

For a China mainland individual applicant, a suitable trusted certificate provider has not yet been selected. This does not block the unsigned preview exception, but it remains a release blocker for stable installers, `Latest` releases, and portable executables.

## Unsigned preview exception

An unsigned preview release must satisfy every requirement below:

1. Use a tag in the `preview-v<app-version>.<iteration>` namespace, for example `preview-v0.1.0.1`. Preview tags must not start with `v`, because `v*` is reserved for the signed release workflow.
2. Mark the GitHub release as a Pre-release. Never mark it as `Latest`.
3. Include `Unsigned Preview` in the release title and an upfront warning in the release notes that Windows SmartScreen and UAC may report an unknown publisher.
4. Attach only the intended NSIS installer and a `SHA256SUMS.txt` file generated from that exact installer.
5. Do not upload `latest.json`, updater `.sig` files, portable executables, or any other artifact that could place the preview on the stable updater channel.
6. Run the frontend and Rust test suites from a clean checkout before building, and record the source commit in the release notes.
7. Confirm `Get-AuthenticodeSignature` reports `NotSigned`. If any other unexpected status appears, stop and investigate instead of publishing.
8. Do not set `AUTHENTICODE_RELEASE_ENABLED=true`, weaken `.github/workflows/release-windows.yml`, or describe the preview as Microsoft trusted.

The complete operator procedure and release-note template are in [unsigned-preview-release.md](unsigned-preview-release.md).

## Stable release requirements

Before publishing a stable binary release:

1. Obtain and securely provision a trusted Authenticode certificate or managed signing service.
2. Keep signing credentials outside the repository and restrict CI access.
3. Sign every distributed `.exe` and installer with an RFC 3161 timestamp.
4. Verify `Get-AuthenticodeSignature` reports `Valid` and includes a timestamp certificate.
5. Generate and verify the separate Tauri updater signatures and `latest.json`.
6. Keep the GitHub release as a draft until every check passes.

Example local verification:

```powershell
$signature = Get-AuthenticodeSignature -LiteralPath .\AIsland-Setup.exe
$signature | Format-List Status,StatusMessage,SignerCertificate,TimeStamperCertificate
```

The release workflow additionally requires the repository variable `AUTHENTICODE_RELEASE_ENABLED=true`. This is a deliberate release switch, not proof of a valid signature; the artifact check remains mandatory.
