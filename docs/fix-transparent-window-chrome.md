# 透明无边框窗口"原生 chrome 复活"问题排查与修复记录

> 关联修复：`preview-v0.1.0.4`
> 涉及文件：`src-tauri/src/lib.rs`
> 环境：Windows 11 / Tauri 2.11.5（tao 0.35.3）/ window-vibrancy 0.6

## 问题现象

AIsland 主悬浮窗（`decorations: false` + `transparent: true` + 亚克力玻璃效果）在以下操作后，会意外弹出 Windows 原生窗口框：

1. **点击桌面**（悬浮窗失焦）后，悬浮窗顶部出现细边框（失焦态系统 chrome）；
2. 失焦状态下**再点击悬浮窗顶部空白栏**，展开后的悬浮窗出现**完整原生标题栏**（最小化/最大化/关闭按钮 + 边框）；
3. 修改 **Windows 显示缩放（DPI）** 后同样可复现。

## 根因分析

### 触发源：`DwmExtendFrameIntoClientArea`

应用的玻璃透明效果通过 `window_vibrancy::clear_acrylic` 实现，其底层调用 Windows DWM 的
`DwmExtendFrameIntoClientArea`。该 API 将窗口 frame 扩展到客户区，会让 DWM 持续认为
"这个窗口存在可绘制的非客户区 frame"。

### 复活时机

`tao` 的 `set_decorations(false)` 只在**设置时**清除 `WS_OVERLAPPEDWINDOW` 样式组
（`WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX`）并发
`SWP_FRAMECHANGED`。但 DWM 会在以下运行时事件重新绘制非客户区：

- 窗口**激活 / 失焦**（`WM_NCACTIVATE` 默认处理重绘标题栏）
- **尺寸变化**（`WM_NCPAINT` 重绘边框）
- **DPI 缩放变化**（系统重算非客户区）

于是出现"样式被清了，但系统一有机会又把 chrome 画回来"的复活现象。
此前的修复（#23）只覆盖了"修改玻璃透明度"这一条路径，焦点与缩放路径是漏网之鱼。

### 排除的错误方向

排查中确认与以下因素**无关**：

- 前端点击事件冒泡未阻止（纯系统层绘制问题，与 JS 事件无关）
- 顶部空白栏被误识别为可拖动区域（`data-tauri-drag-region` 工作正常）
- 缩放后的坐标计算偏差（几何计算无误，是样式复活导致命中区域整体被系统接管）

## 失败方案：事件钩子 + 时序补救（治标不治本）

**做法**：在 `WindowEvent::Focused` / `Resized` / `ScaleFactorChanged` 中重新调用
`set_decorations(false)`，试图在 chrome 复活后立即清掉。

**结果**：部分有效（完整标题栏不再出现），但**失焦瞬间系统画出的细边框仍残留**；
展开窗口后完整原生框偶发复现。

**失败原因**：这是"事后补救"思路——DWM 绘制 chrome 与我们清除样式之间存在**绘制竞争**，
`set_decorations` 的样式清除无法保证在 DWM 绘制之前完成，治标窗口期无法消除。

## 成功方案：subclass 拦截系统非客户区消息（根治）

**做法**：用 `SetWindowSubclass` 给主窗口安装窗口过程子类，直接拦截三个系统消息，
从源头掐断 chrome 的绘制路径：

| 拦截消息 | 处理 | 作用 |
|---|---|---|
| `WM_NCCALCSIZE`（wParam=TRUE） | 返回 0 | 客户区占满整个窗口区域，系统层面不存在非客户区 |
| `WM_NCACTIVATE` | 返回 TRUE | 激活/失焦时不再走默认的标题栏重绘 |
| `WM_NCPAINT` | 返回 0 | 吞掉非客户区绘制请求 |

其余消息一律转发 `DefSubclassProc`，不影响 tao/WebView2 的正常消息处理。

同时保留事件钩子中的 `set_decorations(false)` 再强制作为**表层保险**，形成双层防护：

```text
用户点击桌面/缩放
      │
      ▼
DWM 尝试重绘非客户区 ──► subclass 拦截 WM_NC*，直接吞掉（根治）
      │
      ▼（兜底）
事件钩子 reassert set_decorations(false)（保险）
```

**关键实现点**：

- 子类 ID 固定（`0x41534C44`），`SetWindowSubclass` 重复调用幂等，不会叠加；
- 安装时机在 `show_borderless_window`（窗口显示前必经路径），进程生命周期内一次生效；
- `WM_NCCALCSIZE` 返回 0 与 tao 无边框窗口的原有行为一致，无副作用。

## 验证结果

- 点击桌面失焦 → 无细边框 ✅
- 失焦后点击悬浮窗顶部空白栏展开 → 无完整原生框 ✅
- Windows 显示缩放变化后重复上述操作 → 无 chrome 复活 ✅
- `cargo check` / `cargo build` 通过，无新增警告
- `cargo test` 基线并发 flaky 失败（27~28 个，单独执行均通过）与本次改动无关，无回归

## 经验总结

1. **Windows 无边框 + 透明 + vibrancy 组合**下，`set_decorations(false)` 只是"设置时"的样式声明，
   无法对抗 DWM 在运行时的非客户区绘制。涉及 `DwmExtendFrameIntoClientArea` 的项目，
   chrome 复活问题应默认按"需要拦截 NC 消息"来设计。
2. **绘制竞争类问题不能靠时序补救**：事后清理永远存在治标窗口期，应从消息源头拦截。
3. **修改 glass 材质（vibrancy 系 API）后必须重新强制无边框样式**（#23 已覆盖），
   但还要覆盖焦点、尺寸、DPI 三类系统事件路径（本次补齐）。
4. 测试基线存在并发 flaky（临时文件/共享资源争抢），验证修复时应对照未修改基线，
   避免把环境噪音误判为回归。
