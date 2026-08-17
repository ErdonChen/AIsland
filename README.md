<p align="center">
  <img src="public/aisland-icon.svg" alt="AIsland LOGO" width="160">
</p>

<h1 align="center">AIsland</h1>

<p align="center">
  不单只是 Windows 的灵动岛，更是多 Agent 并行的航空母舰<br>
  简单直观的 Windows 多 Agent 监控工具
</p>

<p align="center">
  <strong>中文</strong> | <a href="README.en.md">English</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="Apache-2.0 license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2011%20x64-0078D4.svg" alt="Windows 11 x64">
  <a href="https://github.com/ErdonChen/AIsland/releases/tag/preview-v0.1.0.2"><img src="https://img.shields.io/badge/distribution-unsigned%20preview-orange.svg" alt="Unsigned preview available"></a>
</p>

## 简介

AIsland 是一款运行在 Windows 上的桌面悬浮工具。它把多个 AI Agent 的状态集中到屏幕顶部，让你不用来回切换窗口，也能知道谁正在工作、谁已经完成，以及 Agent 刚刚回复了什么。

窗口平时保持紧凑，需要时可以展开。除了 Agent 监控，AIsland 还集成了 Windows 通知、Markdown 每日笔记、剪贴板历史和系统监控。

https://github.com/user-attachments/assets/faeef743-5b6f-4357-9d71-ef28cfe4acdb

## 星舰视图

星舰视图把已连接的 Agent 排成一组状态位。最近发生变化的 Agent 会排在前面，打开多个 Agent 时可以向下滚动查看。

<p align="center">
  <img src="docs/images/readme-starship-compact.png" alt="AIsland 星舰紧凑视图" width="492">
</p>

<p align="center">
  <img src="docs/images/readme-agent-status.png" alt="AIsland Agent 状态主页">
</p>

状态灯的含义：

| 颜色 | 状态 |
| --- | --- |
| 黄色呼吸灯 | 工作中 |
| 绿色常亮 | 已完成 |
| 浅蓝色常亮 | 空闲 |
| 灰色常亮 | 离线 |
| 红色 | 失败、等待或超时 |

同一个 Agent 同时运行多个任务时，AIsland 会聚合这些任务的状态。任务完成后会短暂显示绿色；如果还有任务在运行，状态会回到黄色。

## 主要功能

### 查看状态和最近回复

AIsland 会显示 Agent 的工作、完成、空闲和离线状态。支持的原生源还会显示最新一条 assistant 回复。回复只取自经过验证的 assistant 字段，不会把用户输入、推理过程、工具参数或工具输出显示在岛上。

### 自动识别与一键接入

先打开需要监控的 Agent，再进入 `设置 > Agent 与接入`，点击 `一键检测并配置 Hook`。AIsland 会识别正在运行的受支持 Agent，并优先使用可用的原生源。

<p align="center">
  <img src="docs/images/readme-agent-integrations.png" alt="AIsland Agent 检测与接入页面">
</p>

一键检测只检查进程和固定配置位置。原生源始终只读；当用户明确点击自动配置时，AIsland 才会修改自己管理的 Hook 条目，并保留 Agent 原有配置。

### 手动连接 Agent

如果 Agent 不在默认列表中，但提供了可调用的 Hook，可以在 `Agent 与接入` 页面添加 Custom Hook。Windows 第一版要求填写一个已存在的 `.exe`、独立参数和事件映射。

Custom Hook 适合兼容接入，但不等同于原生支持。厂商没有提供明确事件、稳定任务 ID 或 assistant 正文字段时，部分状态和最近回复可能不可用。

### Windows 实用工具

- 通知中心：集中查看 Windows 通知，并支持来源和未读筛选。
- Markdown 每日笔记：按日期保存，支持搜索、复制、导出和打开笔记目录。
- 剪贴板历史：管理文字和图片记录，支持搜索、筛选、置顶、复制与删除。
- 系统监控：查看 CPU、内存、磁盘、网络和 GPU 指标。
- 托盘与启动：窗口可收进系统托盘，并可在设置中管理开机启动、语言、缩放和外观。

<p align="center">
  <img src="docs/images/readme-system-monitor.png" alt="AIsland 系统监控页面">
</p>

## Agent 支持情况

| Agent | 当前接入方式 | 状态灯 | 最近回复 |
| --- | --- | --- | --- |
| Codex | 原生 | 支持 | 支持 |
| Claude Desktop Cowork | 原生 | 支持 | 支持 |
| Hermes | 原生 | 支持 | 支持 |
| Cursor | 原生 | 支持 | 支持 |
| Kimi / Kimi Work | 原生 | 支持 | 支持 |
| WorkBuddy | 原生 | 支持 | 支持 |
| QoderWork / 千问办公 | 原生 | 支持 | 支持 |
| TRAE / TRAE CN / TRAE SOLO CN / TraeWork | 已识别，Hook 兼容或等待适配 | 视 Hook 而定 | 不保证 |

原生支持只覆盖已经验证过的本地会话格式。厂商更新存储格式后，相关 Adapter 可能需要同步更新。

## 使用提示

### Hook 能力有限

部分 Agent 无法安全读取本地会话，只能通过 Hook 连接。Hook 可能只能提供基础状态，暂时无法保证最近回复、精确完成时间、多任务聚合和完成状态切换全部可用。

### WSL 仍需更多实机验证

AIsland 保留 Codex 和 WorkBuddy 的 WSL 接入路径，但目前缺少足够多的真实 WSL 环境验证。不同发行版、挂载方式和 Agent 版本可能影响状态或回复读取。

### 隐私边界

AIsland 在本机运行，并以只读方式访问受支持 Agent 的会话源。它只提取状态和最新 assistant 正文，不读取或展示用户输入、推理过程、工具参数和工具输出。未知数据库或日志格式不会被猜测解析。

## 基本操作

1. 启动 AIsland。紧凑状态岛会显示在桌面顶部。
2. 打开一个或多个需要监控的 Agent。
3. 展开 AIsland，进入 `设置 > Agent 与接入`。
4. 点击 `一键检测并配置 Hook`。原生支持的 Agent 会自动使用原生源，其余 Agent 会显示可用的 Hook 或待适配状态。
5. 回到主页查看状态阵列和最近回复。点击顶部箭头可以折叠窗口，最小化按钮会把程序收进系统托盘。

## 下载 Windows 预览版

当前版本：[`preview-v0.1.0.2`](https://github.com/ErdonChen/AIsland/releases/tag/preview-v0.1.0.2)

- [下载 Windows 11 x64 NSIS 安装包](https://github.com/ErdonChen/AIsland/releases/download/preview-v0.1.0.2/AIsland_0.1.0_x64-setup.exe)
- [查看 SHA256SUMS.txt](https://github.com/ErdonChen/AIsland/releases/download/preview-v0.1.0.2/SHA256SUMS.txt)
- SHA-256：`1f5c0f184b8e8f0e05acd144ece4e96e6333a41b33f5a41b2f679abae225ce6f`

> [!WARNING]
> 这是面向技术测试用户的未签名预览版。Windows SmartScreen 和 UAC 可能显示“未知发布者”。请只从 AIsland 官方 GitHub 仓库下载，并在运行前核对 SHA-256；不要关闭 Microsoft Defender 或 SmartScreen。

## 当前发布状态

项目当前为技术测试用户提供未签名 Windows 安装包，但只能作为明确标注的 GitHub Pre-release：发布标题和说明必须注明“未签名预览版”，附带 SHA-256，不得标记为 `Latest`，也不得发布 `latest.json` 或进入正式自动更新通道。

面向普通用户的正式版、`Latest` 和便携 EXE 仍须取得受信任的 Authenticode 签名，并通过签名发布门禁。Tauri 更新签名只负责更新完整性，不能代替 Windows 的 Authenticode 信任。具体规则见[未签名预览版发布指南](docs/unsigned-preview-release.md)和[Windows 代码签名政策](docs/code-signing.md)。

当前正式支持 Windows 11 x64。Windows 10 x64 可能兼容，但尚未纳入完整门禁；Windows ARM、macOS 和 Linux 桌面版不在首版支持范围。

## 从源码运行

需要 Windows、Node.js、Rust stable MSVC、Microsoft C++ Build Tools 和 WebView2。

```powershell
npx --yes pnpm@10.15.0 install
npx --yes pnpm@10.15.0 tauri dev
```

运行测试和生产构建：

```powershell
npx --yes pnpm@10.15.0 test
cargo test --manifest-path src-tauri/Cargo.toml
npx --yes pnpm@10.15.0 tauri build --no-bundle
```

## 开发文档

- [Agent 能力注册表与原生适配指南](docs/agent-integration-capabilities.md)
- [当前架构决策](docs/decisions.md)
- [后端状态](docs/backend-status.md)
- [前端状态](docs/frontend-status.md)
- [Community / Pro 边界](docs/open-core.md)
- [未签名预览版发布指南](docs/unsigned-preview-release.md)
- [Windows 代码签名政策](docs/code-signing.md)

## 参与、隐私与许可

- [贡献指南与 DCO](CONTRIBUTING.md)
- [隐私说明](PRIVACY.md)
- [安全政策](SECURITY.md)
- [支持范围](SUPPORT.md)
- [项目治理](GOVERNANCE.md)
- [商标与品牌](TRADEMARKS.md)
- [Apache License 2.0](LICENSE)

本仓库是 Apache-2.0 许可的 Community Edition。未来可能推出独立维护、单独许可的付费 Pro 功能；已经发布的 Community 代码不会因此改变许可，现有 Community 功能的安全修复和重大兼容性修复仍保持免费。

应用对外名称、安装器名称和内部名称统一使用 AIsland，应用标识为 `com.aisland.app`。
