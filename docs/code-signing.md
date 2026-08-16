# Windows code-signing policy

## Current distribution status

AIsland currently publishes source code and build instructions only. The project does not publish an unsigned installer or portable executable for ordinary users.

A public Windows installer is allowed only after the final artifacts receive a trusted Authenticode signature from a suitable certificate provider and the release workflow verifies that signature. The Tauri updater signature is a separate integrity mechanism and does not replace Authenticode trust.

For a China mainland individual applicant, a suitable trusted certificate provider has not yet been selected. This does not block source publication, but it remains a release blocker for installers and portable executables.

## Release requirements

Before publishing a binary release:

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
