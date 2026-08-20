# Windows code-signing policy

## Current distribution status

AIsland may publish an unsigned Windows installer for technical evaluation under the narrow preview exception below. An unsigned preview is not a stable release and is not intended for ordinary users.

A stable Windows installer, a release marked `Latest`, or a portable executable is allowed only after the final artifacts receive a trusted Authenticode signature from a suitable certificate provider and the signed release workflow verifies that signature. The Tauri updater signature is a separate integrity mechanism and does not replace Authenticode trust.

For a China mainland individual applicant, a trusted provider has not yet been provisioned. This does not block the unsigned preview exception, but it remains a release blocker for stable installers, `Latest` releases, and portable executables. The current provider evaluation is recorded in [the 2026-08-20 research note](research/authenticode-mainland-china-2026-08-20.md).

## Public code-signing policy

AIsland is applying to SignPath Foundation. If the application is approved and the integration is enabled, stable releases will use **free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)**. Current preview releases remain explicitly unsigned until that integration passes the stable release gate.

- **Project owner and release approver:** ErdonChen.
- **Source authors and reviewers:** changes enter the release branch through GitHub pull requests and the `Community quality gate`; the release approver must review the final tagged source and passing checks.
- **Trusted build:** stable artifacts are built from an existing `v<semver>` tag by `.github/workflows/release-windows.yml`. The workflow starts with an explicit release gate and keeps the GitHub release as a draft until signature and updater verification pass.
- **Signing key custody:** private signing keys must remain in the selected provider's HSM or cloud signing service. They must never be exported to, committed to, or printed by this repository.
- **Signing approval:** the project owner approves stable signing requests. If the selected provider requires a separate human approver, that role must be assigned before the integration is enabled.
- **Privacy:** AIsland's data-access and network boundaries are documented in [PRIVACY.md](../PRIVACY.md). Signing-provider credentials are used only by the stable release workflow.

As of 2026-08-20, the preferred application order is:

1. Apply to [SignPath Foundation](https://signpath.org/) for its free open-source signing service. Approval is not guaranteed, each release requires human approval, and the certificate publisher is `SignPath Foundation` rather than the maintainer.
2. If SignPath is unavailable, use [SSL.com IV Code Signing with eSigner](https://www.ssl.com/products/software-integrity/signing-service/). SSL.com's official documentation lists China as an accepted country and documents GitHub Actions plus headless eSigner CKA signing, but purchase and identity verification must be completed by the maintainer.
3. Do not use Microsoft Artifact Signing Public Trust for this account: its current individual-developer eligibility is limited to the United States and Canada. Do not purchase Certum until it is back in stock and Certum confirms China-mainland individual issuance and unattended CI in writing.

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
2. Load the approved cloud-backed code-signing certificate into `Cert:\CurrentUser\My` on the Windows signing runner without exporting its private key.
3. Keep signing credentials outside the repository and restrict CI access.
4. Configure `AUTHENTICODE_CERTIFICATE_SHA1` as a repository secret and `AUTHENTICODE_TIMESTAMP_URL` as a repository variable.
5. For SSL.com eSigner CKA or another reviewed Windows-certificate-store provider, build with `src-tauri/tauri.signed.conf.json`. Its Tauri `signCommand` calls `.github/scripts/sign-windows-artifact.ps1` for every executable before updater signatures are generated. SignPath requires its own trusted-build integration and must preserve the same ordering.
6. Sign every distributed `.exe` and installer with SHA-256 and an RFC 3161 timestamp.
7. Verify `Get-AuthenticodeSignature` reports `Valid` and includes a timestamp certificate.
8. Generate and verify the separate Tauri updater signatures and `latest.json` from the final Authenticode-signed installer bytes.
9. Keep the GitHub release as a draft until every check passes.

Example local verification:

```powershell
$signature = Get-AuthenticodeSignature -LiteralPath .\AIsland-Setup.exe
$signature | Format-List Status,StatusMessage,SignerCertificate,TimeStamperCertificate
```

The release workflow additionally requires the repository variable `AUTHENTICODE_RELEASE_ENABLED=true`. This is a deliberate release switch, not proof of a valid signature; the artifact check remains mandatory. Do not enable it until a reviewed provider-provisioning step has loaded the certificate on the Windows runner and a non-publishing dry run has passed.

The checked-in signing wrapper is provider-neutral only at the Windows certificate-store and SignTool boundary. It is ready for SSL.com eSigner CKA or an equivalent provider that exposes a cloud-backed certificate through `Cert:\CurrentUser\My`; it does not download a provider client, authenticate to a provider, or create credentials. If SignPath accepts AIsland, use a separate reviewed integration because SignPath signs through its trusted-build service rather than the local certificate store. In either case, pin and review the provider's official component and ensure Authenticode finishes before updater signatures are generated.
