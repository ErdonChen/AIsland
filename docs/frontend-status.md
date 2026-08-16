# Frontend status

## Completed

- Agent cards show a bounded latest Agent-reply preview only.
- Agent snapshot startup races recover through a retained listener and a two-second authoritative poll.
- Fixed Agents with no live desktop or terminal observation stay hidden; currently running Agents remain visible.
- Process-only Agent observations render as idle rather than falsely reporting active work; Hook status remains authoritative.
- Kimi Code, TRAE, and QoderWork desktop presence participates in the two-second authoritative profile refresh without claiming the Hook is installed.
- Status lights follow the product contract: running yellow/pulsing, completed green/steady, idle sky-blue/steady, offline gray/steady.
- Same-Agent concurrent tasks show completed green/steady for 2 seconds after an individual completion, then return to running yellow/pulsing while a sibling task remains active; all-finished tasks remain green.
- Daily Notes recovers from the startup State-registration race before enabling editing; ASCII, Chinese UTF-8, multiline Markdown, cold-start readback, autosave, and retry are verified natively.
- Expanded island can minimize to the Windows tray.
- Compact Agent slots use packaged desktop icons for Codex, Hermes, WorkBuddy, Claude, Kimi Code, and TRAE.
- Tauri release builds track the current `dist` so frontend changes are embedded in the final executable.
- Agent Settings includes one-click integration discovery. The scan is read-only; running Agents with a safe Windows preset are installed/repaired through the existing revisioned command, while unsupported/custom results remain explicit and Custom Hook drafts stay unsaved.
- Frontend 367/367, Rust 571/571, TypeScript, Vite, and the final Tauri release no-bundle build are green.
- Native acceptance: Codex and WorkBuddy process presence renders idle/sky-blue; latest activity is shown inside the matching Agent card; Daily Notes cold-start readback and autosave persist through SQLite; minimize hides the window while the process remains alive; only the main AIsland window is present and the retired standalone reminder window is never shown.

## Current

- One-click Agent integration discovery is in final native verification. Private GitHub synchronization remains paused by user request.

## Blockers

- The updater public key and HTTPS release endpoint are embedded. Signed updater publication still requires valid GitHub authentication plus the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

## Need backend

- None.
