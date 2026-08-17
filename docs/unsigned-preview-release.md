# Unsigned Windows preview releases

This runbook is the only approved path for publishing an unsigned AIsland installer. It is intended for technical testers while trusted Authenticode signing is unavailable. It must not be used for a stable or `Latest` release.

## Non-negotiable boundaries

- Build from a clean checkout of the public repository on the GitHub-hosted Windows runner.
- Use a tag named `preview-v<app-version>.<iteration>`, such as `preview-v0.1.0.1`. Do not use a tag beginning with `v`; `v*` is reserved for the signed release workflow.
- Publish the GitHub release as a Pre-release and never as `Latest`.
- Put `Unsigned Preview` in the release title and lead the release notes with the warning template below.
- Upload only the NSIS setup executable and `SHA256SUMS.txt`.
- Do not upload `latest.json`, `.exe.sig`, updater signatures, portable executables, or MSI packages.
- Do not set `AUTHENTICODE_RELEASE_ENABLED=true` and do not edit or bypass `.github/workflows/release-windows.yml`.
- Do not tell users to disable Microsoft Defender or SmartScreen.

## GitHub Actions release from macOS

The approved default path is the manual workflow at `.github/workflows/release-windows-preview.yml`. The workflow must first be merged into the default branch. Your Mac only triggers and monitors the run; GitHub's `windows-2022` runner performs the tests, NSIS build, Authenticode status check, checksum generation, upload, and download verification.

From the repository on macOS, authenticate `gh`, update `main`, and start the first preview:

```bash
gh auth status
git switch main
git pull --ff-only origin main
gh workflow run release-windows-preview.yml \
  --repo ErdonChen/AIsland \
  --ref main \
  -f tag=preview-v0.1.0.1 \
  -f publish=true
```

Find and watch the run:

```bash
gh run list --repo ErdonChen/AIsland --workflow release-windows-preview.yml --limit 1
gh run watch --repo ErdonChen/AIsland
```

Use a new positive iteration for every attempt that reaches release creation, for example `preview-v0.1.0.2`. The tag's application version must match `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`.

Set `publish=false` when you want the workflow to leave a verified draft for manual review. Set `publish=true` only when the same run should publish the verified GitHub Pre-release. In both cases, the workflow creates exactly two uploaded assets: the NSIS setup executable and `SHA256SUMS.txt`.

## Local Windows fallback checklist

Use this only if GitHub Actions is unavailable.

1. Install the Windows prerequisites listed in the README: Node.js, Rust stable MSVC, Microsoft C++ Build Tools, and WebView2.
2. Check out the exact public source commit that will be released. Confirm `git status --short` is empty and record `git rev-parse HEAD`.
3. Install dependencies and run both test suites:

   ```powershell
   npx --yes pnpm@10.15.0 install --frozen-lockfile
   npx --yes pnpm@10.15.0 test
   cargo test --manifest-path src-tauri/Cargo.toml --locked
   ```

4. Build only the NSIS installer. If the configured Tauri updater key is not available, override `bundle.createUpdaterArtifacts` to `false` for this build; do not generate or upload updater metadata for an unsigned preview.
5. Locate the single installer under `src-tauri\target\release\bundle\nsis`. Stop if the build produced zero or multiple setup executables.
6. Confirm the installer is unsigned:

   ```powershell
   $installer = @(Get-ChildItem -LiteralPath .\src-tauri\target\release\bundle\nsis -Filter *.exe -File)
   if ($installer.Count -ne 1) { throw "Expected exactly one NSIS installer." }
   $signature = Get-AuthenticodeSignature -LiteralPath $installer[0].FullName
   $signature | Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate
   if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
     throw "Unexpected Authenticode status: $($signature.Status)"
   }
   ```

7. Generate the checksum beside the installer:

   ```powershell
   $hash = Get-FileHash -LiteralPath $installer[0].FullName -Algorithm SHA256
   $checksumPath = Join-Path $installer[0].DirectoryName "SHA256SUMS.txt"
   "{0}  {1}" -f $hash.Hash.ToLowerInvariant(), $installer[0].Name |
     Set-Content -LiteralPath $checksumPath -Encoding ascii
   Get-Content -LiteralPath $checksumPath
   ```

8. Create a GitHub Release using a new `preview-v*` tag, enable the Pre-release checkbox, and leave `Set as the latest release` disabled. Upload only the installer and `SHA256SUMS.txt`.
9. After publishing, download both assets from the public Release page onto a clean Windows account or Windows Sandbox, recalculate SHA-256, and confirm it matches before sharing the link.

## Release notes template

```markdown
> [!WARNING]
> This is an unsigned technical preview for testing only. Windows SmartScreen
> and UAC may show `Unknown publisher`. Download it only from the official
> AIsland GitHub repository and verify the SHA-256 checksum before running it.
> Do not disable Microsoft Defender or SmartScreen.

## Build provenance

- Source repository: https://github.com/ErdonChen/AIsland
- Source commit: `<full commit SHA>`
- Platform: Windows 11 x64
- Package: NSIS installer
- Authenticode status: `NotSigned`
- SHA-256: see `SHA256SUMS.txt`

## Scope

- Intended for technical evaluation, not ordinary users or production use.
- This Pre-release is not served through AIsland's stable updater channel.
- A stable release will require a trusted Authenticode signature and the signed-release gate.

## Known installation behavior

Windows may display a SmartScreen or UAC warning because the installer has no
trusted publisher signature. Verify that the download URL is under
`github.com/ErdonChen/AIsland/releases/` and that the checksum matches before
deciding whether to run it.
```

## Prompt for the macOS release agent

Copy the prompt below to an agent running on the Mac after the workflow is merged into `main`:

```text
你正在 macOS 上通过 GitHub Actions 为 https://github.com/ErdonChen/AIsland
发布一个“未签名技术预览版”。
开始前完整阅读并严格遵守仓库中的 docs/unsigned-preview-release.md、
docs/code-signing.md、.github/RELEASING.md 和
.github/workflows/release-windows-preview.yml。

目标：在确认工作流已经合并到 main 后，从 Mac 手动触发
release-windows-preview.yml。由 GitHub 的 windows-2022 runner 从 main 的确定提交构建
唯一一个 NSIS 安装包，运行前端与 Rust 测试，确认 Get-AuthenticodeSignature 的状态
恰好为 NotSigned，生成 SHA256SUMS.txt，然后创建 GitHub Pre-release 并上传这两个文件。

硬性限制：
1. 使用 preview-v<应用版本>.<序号> 标签，标签不得以 v 开头。
2. Release 标题必须包含 Unsigned Preview，必须勾选 Pre-release，不得设为 Latest。
3. 只上传 NSIS setup.exe 和 SHA256SUMS.txt。
4. 不得上传 latest.json、.exe.sig、任何 updater 元数据、便携 EXE 或 MSI。
5. 不得设置 AUTHENTICODE_RELEASE_ENABLED=true，不得修改或绕过
   .github/workflows/release-windows.yml。
6. 不得声称安装包已获 Microsoft 信任，不得让用户关闭 Defender 或 SmartScreen。
7. 使用 gh workflow run 从 main 触发，并监控到整个工作流结束。不得在本机伪造构建结果。
8. 构建、测试、签名状态、文件数量、哈希或公开下载后的复验有任何一项不符合，立即停止，
   不要发布，并向我报告具体错误。

触发前先向我展示：main 的源提交 SHA、准备使用的 preview-v 标签和 publish 输入值。
得到我明确确认后才能以 publish=true 触发。完成后把 Actions run 链接、Release 链接、
测试结果、Authenticode 状态、资产清单和公开下载复验的 SHA-256 结果交给我。
```
