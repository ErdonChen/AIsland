# AIsland Privacy Policy

Last updated: 2026-08-17

## 中文

AIsland Community Edition 是本地优先的 Windows 桌面应用。它不要求账号，不包含遥测、广告分析或自动崩溃上传，也不会把 Agent 对话、笔记、剪贴板、通知或系统指标发送给 AIsland 维护者。

### 本地处理的数据

- Agent 状态和经过验证的最新 assistant 正文；
- 用户主动启用的 Windows 通知历史、剪贴板历史、Markdown 笔记与系统监控数据；
- 应用设置、集成状态、提醒记录和本地诊断日志。

这些数据保存在用户自己的 Windows 设备上。AIsland 不读取或展示用户提示词、模型推理、工具参数或工具输出。原生 Agent Adapter 只读经过验证的本地会话源，未知格式不会被猜测解析。

### Hook 配置

只有在用户明确点击安装、修复或卸载 Hook 时，AIsland 才会修改受支持 Agent 的配置。修改使用预览、版本检查、备份和回滚保护。原生会话扫描本身始终只读。

### 网络访问

AIsland Community Edition 不自动上传使用数据。只有用户主动点击“检查更新”时，应用才会访问 GitHub Release updater endpoint；GitHub 可能按照其自己的政策处理普通网络请求元数据。源码安装依赖和 GitHub 页面访问也由用户主动触发。

### 本地日志

诊断日志保存在本机并受大小与轮转限制。日志不得包含用户输入、assistant 正文、推理、工具输出、剪贴板正文、通知正文或密钥。日志只有在用户主动选择并自行提供时才会离开设备。

### 数据删除

用户可以在应用中删除支持的本地记录。卸载应用不会自动删除第三方 Agent 数据。需要彻底移除 AIsland 本地数据时，请先退出应用，再删除 Windows 应用数据目录中的 AIsland 数据；执行前请自行备份需要保留的笔记或剪贴板记录。

### 未来 Pro 功能

如果未来 AIsland Pro 增加账号、同步、远程控制或云服务，将提供单独的隐私说明、明确开关和必要授权，不会静默改变 Community Edition 的本地优先承诺。

隐私问题请联系 `aisland_support@163.com`。安全漏洞请使用仓库的 GitHub Private Vulnerability Reporting，不要发送到公开 Issue。

## English

AIsland Community Edition is a local-first Windows desktop application. It requires no account and includes no telemetry, advertising analytics, or automatic crash uploads. It does not send agent conversations, notes, clipboard entries, notifications, or system metrics to the AIsland maintainer.

### Data processed locally

- Agent status and the latest assistant text from a verified assistant-only field;
- Windows notification history, clipboard history, Markdown notes, and system metrics when the related feature is enabled by the user;
- Application settings, integration state, reminders, and local diagnostic logs.

This data remains on the user's Windows device. AIsland does not read or display user prompts, model reasoning, tool parameters, or tool output. Native agent adapters read only verified local session sources and fail closed on unknown formats.

### Hook configuration

AIsland changes a supported agent configuration only after the user explicitly chooses to install, repair, or remove a Hook. Changes use preview, revision checks, backups, and rollback protection. Native session scanning remains read-only.

### Network access

AIsland Community Edition does not automatically upload usage data. It contacts the GitHub Release updater endpoint only after the user selects “Check for updates.” GitHub may process ordinary request metadata under its own policies. Dependency installation and opening GitHub pages are also user-initiated actions.

### Local logs

Diagnostic logs remain local and are size-limited and rotated. They must not contain user prompts, assistant text, reasoning, tool output, clipboard contents, notification contents, or secrets. Logs leave the device only when the user deliberately provides them.

### Deleting local data

Supported records can be deleted from the application. Uninstalling AIsland does not delete third-party agent data. To remove all AIsland local data, exit the application and delete its Windows application-data directory after backing up any notes or clipboard records you want to keep.

### Future Pro features

If AIsland Pro later adds accounts, synchronization, remote control, or cloud services, those features will have a separate privacy notice, explicit controls, and required consent. They will not silently change the local-first promise of Community Edition.

For privacy questions, contact `aisland_support@163.com`. Report security vulnerabilities through GitHub Private Vulnerability Reporting, not a public Issue.
