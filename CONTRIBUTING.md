# Contributing to AIsland

Thank you for helping improve AIsland Community Edition. The Community Edition is developed in the open under the Apache License 2.0. Future paid editions may be maintained separately and are not part of this repository.

## Before you contribute

- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- Report vulnerabilities through the private process in [SECURITY.md](SECURITY.md), not a public issue.
- Search existing issues before opening a new bug report or feature request.
- Do not include real Agent conversations, credentials, private keys, personal paths, database files, or unredacted screenshots.

## Development environment

AIsland currently targets Windows 11 x64. Windows 10 x64 may work but is not part of the full release gate. WSL Agent integrations remain experimental.

Install Node.js 22, Rust stable with the MSVC toolchain, Microsoft C++ Build Tools, WebView2, and Git. Then run:

```powershell
npx --yes pnpm@10.15.0 install
npx --yes pnpm@10.15.0 tauri dev
```

Before opening a pull request, run:

```powershell
npx --yes pnpm@10.15.0 test
npx --yes pnpm@10.15.0 build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
npx --yes pnpm@10.15.0 tauri build --no-bundle
node scripts/check-license-policy.mjs
```

## Pull requests

Keep each pull request focused and explain the user-visible behavior, privacy impact, tests, and any remaining limitations. Add deterministic regression tests for state transitions, parsers, persistence, and performance-sensitive code. Performance tests should assert measurable work such as bytes read or parser calls instead of relying only on timing thresholds.

Native Agent adapters must preserve AIsland's privacy boundary: read third-party data without modifying it, extract only lifecycle state and an explicitly verified assistant reply field, and never surface user prompts, reasoning, tool parameters, or tool output. Hook configuration changes must be initiated explicitly by the user, remain scoped to AIsland-managed entries, and preserve unrelated configuration.

## Developer Certificate of Origin

AIsland uses the [Developer Certificate of Origin 1.1](https://developercertificate.org/). A CLA is not required. Sign every commit with:

```powershell
git commit -s -m "Describe the change"
```

The sign-off certifies that you have the right to submit the contribution under this repository's license. CI rejects pull requests containing commits without a matching `Signed-off-by` line.

## Dependency license policy

New dependencies should use a permissive license such as Apache-2.0, MIT, BSD, ISC, Zlib, Unicode, CC0, or Boost Software License 1.0. MPL-2.0 and LGPL dependencies require an explicit compatibility and distribution review before they are added.

GPL, AGPL, SSPL, Business Source License (`BUSL-*`), Commons Clause, noncommercial licenses, no-derivatives licenses, custom licenses, and packages with unknown or missing license metadata are not accepted without a documented maintainer decision. `BSL-1.0` means the permissive Boost Software License and is distinct from `BUSL-*`.

## Project decisions

Maintainers may decline changes that expand data collection, weaken local-first behavior, destabilize the Agent capability contract, or create an unsupported maintenance burden. Community security fixes and major compatibility fixes for existing Community features remain part of the free edition.
