# Unsigned Windows preview releases

This runbook is the only approved path for publishing an unsigned AIsland installer. It is intended for technical testers while trusted Authenticode signing is unavailable. It must not be used for a stable or `Latest` release.

## Non-negotiable boundaries

- Build from a clean checkout of the public repository on Windows.
- Use a tag named `preview-v<app-version>.<iteration>`, such as `preview-v0.1.0.1`. Do not use a tag beginning with `v`; `v*` is reserved for the signed release workflow.
- Publish the GitHub release as a Pre-release and never as `Latest`.
- Put `Unsigned Preview` in the release title and lead the release notes with the warning template below.
- Upload only the NSIS setup executable and `SHA256SUMS.txt`.
- Do not upload `latest.json`, `.exe.sig`, updater signatures, portable executables, or MSI packages.
- Do not set `AUTHENTICODE_RELEASE_ENABLED=true` and do not edit or bypass `.github/workflows/release-windows.yml`.
- Do not tell users to disable Microsoft Defender or SmartScreen.

## Windows operator checklist

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

## Prompt for the Windows PC agent

Copy the prompt below to the agent running on the Windows PC:

```text
你正在 Windows PC 上为 https://github.com/ErdonChen/AIsland 制作一个“未签名技术预览版”。
开始前完整阅读并严格遵守仓库中的 docs/unsigned-preview-release.md、
docs/code-signing.md 和 .github/RELEASING.md。

目标：从干净的公开源码提交构建唯一一个 NSIS 安装包，运行前端与 Rust 测试，确认
Get-AuthenticodeSignature 的状态恰好为 NotSigned，生成 SHA256SUMS.txt，然后在 GitHub
创建 Pre-release 并上传这两个文件。

硬性限制：
1. 使用 preview-v<应用版本>.<序号> 标签，标签不得以 v 开头。
2. Release 标题必须包含 Unsigned Preview，必须勾选 Pre-release，不得设为 Latest。
3. 只上传 NSIS setup.exe 和 SHA256SUMS.txt。
4. 不得上传 latest.json、.exe.sig、任何 updater 元数据、便携 EXE 或 MSI。
5. 不得设置 AUTHENTICODE_RELEASE_ENABLED=true，不得修改或绕过
   .github/workflows/release-windows.yml。
6. 不得声称安装包已获 Microsoft 信任，不得让用户关闭 Defender 或 SmartScreen。
7. 构建、测试、签名状态、文件数量、哈希或公开下载后的复验有任何一项不符合，立即停止，
   不要发布，并向我报告具体错误。

发布前先向我展示：源提交 SHA、测试结果、安装包完整路径、文件大小、Authenticode 状态、
SHA-256、准备使用的标签和完整 Release notes。得到我明确确认后才能创建并公开 Release。
发布后重新从公开 Release 下载资产并复验 SHA-256，最后把 Release 链接和复验结果交给我。
```
