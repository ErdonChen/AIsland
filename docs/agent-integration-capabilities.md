# Agent 能力注册表与原生适配指南

## 第一版边界

AIsland 的能力注册表是受信任的编译期清单，不是让终端用户填写任意文件路径、JSONPath 或 SQL 的解析器。注册表负责声明“如何识别 Agent、是否存在原生 Adapter、可以可靠提供哪些能力、何时回退到 Hook”；真正的厂商格式解析必须封装在原生 Adapter 内。

这样可以同时保证：

- 第三方会话文件始终只读；
- 只提取厂商可验证的 assistant 最终正文；
- 不读取或展示用户输入、推理、工具参数和工具输出；
- 文件未变化时不读取内容、不解析 JSON、不写 AIsland 数据库；
- 厂商格式变化时安全失败并回退，而不是误报状态或泄露内容。

## 每个注册项需要声明的能力

| 字段 | 含义 |
| --- | --- |
| 稳定 ID 与显示名称 | 不随本地化或进程标题变化 |
| 进程与安装别名 | 仅用于发现和空闲/可连接兜底，不得据此误报工作中 |
| 原生 Adapter | `builtin`、`profile` 或 `none` |
| 精确状态 | 是否能区分工作中、完成、失败和空闲 |
| 回复预览 | 是否有经过验证的 assistant-only 字段 |
| 稳定任务 ID | 是否可以支持同一 Agent 多任务聚合与完成态恢复 |
| Hook 回退 | `managed`、`custom`、`pending` 或 `none` |
| 平台范围 | Windows、WSL 或两者 |

能力必须逐项声明，不能因为“检测到进程”就推断存在原生状态、回复预览或多任务能力。

## 增加一个原生 Agent

1. 在 `src-tauri/src/services/agent_integration_discovery.rs` 和 `agent_profiles.rs` 增加精确进程/安装证据，避免模糊匹配其他程序。
2. 内置 Agent 实现 `NativeAgentActivitySource::latest_activity`；Profile Agent 实现 `NativeProfileActivitySource::latest_activity`。调用方只消费统一的状态、任务 ID、时间和最近回复，不接触厂商格式。
3. 原生 Adapter 仅允许读取固定、已验证的本地源。对 JSONL 使用增量偏移、文件身份和半行缓存；对数据库使用只读连接与 assistant-only 查询。
4. 为大文件首载、文件未变化、增量追加、半行、截断/轮转、会话切换和事件去重增加确定性测试；必须断言 bytes-read 与 parser-call，不能只依赖耗时。
5. 为隐私增加负向测试，证明用户输入、推理、工具参数和工具输出不会进入解析器或回复预览。
6. 更新 README 能力矩阵并进行真实桌面验收；厂商格式或版本未经验证时保持 `pending`，不得把 Hook 或进程检测描述为完整原生接入。

## 未收录 Agent 的用户路径

- Agent 提供 Hook：在设置中使用 Custom Hook，映射其明确的生命周期事件。
- Agent 只有本地会话源：第一版不允许直接填写数据库或日志路径；提交脱敏格式说明与 Adapter 贡献，由测试锁定隐私和兼容性后加入注册表。
- Agent 没有 Hook，也没有可验证的只读本地源：只能显示进程存在/空闲，不能承诺工作状态或最近回复。

Custom Hook 适合兼容接入，但不保证原生 Adapter 的完整能力。尤其是厂商不提供 assistant-only 正文字段或稳定任务 ID 时，AIsland 必须保持回复为空或降低状态精度。
