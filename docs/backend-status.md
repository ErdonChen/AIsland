# Backend Status

## Completed

- Local-first Rust/Tauri backend, bounded console/file logging, SQLite migrations and lifecycle-safe shutdown.
- Daily Notes, Clipboard, Monitor, Windows notification history, reminders and typed command/event bridges.
- Todo and media-control runtime surfaces retired while retaining historical data compatibility.
- Kimi Code and QoderWork preset integration, Windows Custom Hook, dynamic profile snapshots and durable event spool. TRAE remains fail-closed until a verified configuration target is detected.
- Bounded read-only Agent integration discovery reports exact process/config/application evidence without returning vendor file contents or treating an arbitrary executable as a compatible Hook adapter. Settings may explicitly pass a running safe-preset result into the existing revisioned, rollback-capable install/repair command.
- Built-in and Profile Agent snapshots share the concurrent-task display rule: a received completion overrides running for 2 seconds, then running resumes while any sibling task remains active; all-finished tasks remain completed.
- Connected built-in and safe-preset Hooks actively attempt assistant-only reply extraction. Hermes uses `extra.assistant_response` plus stable `task_id/turn_id`; Kimi/Qoder-style Stop Hooks use `last_assistant_message`. Previews are bounded and persisted without user prompts, history, reasoning, or tool output.
- Built-in WSL Hooks resolve the Windows app-data status directory through `wslpath` and write into the same directory watched by AIsland; they no longer emit dynamic state into an unwatched WSL-private directory.
- Windows autostart and signed-updater service bridges. The updater public key and fixed GitHub Releases HTTPS endpoint are embedded; signing secrets are stored only in the private GitHub repository.
- Current automated gates: Rust 593/593 and frontend 374/374 passed; final formatting/diff and rebuilt Tauri release checks are pending the current desktop acceptance.

## Current

- Signed private release preparation, real Windows end-to-end acceptance and public distribution setup.
- Local branch `feat/window-shell` contains unpublished commits that still need review, push and merge.

## Blockers

- The repository and release must remain private/draft until the user explicitly chooses a publication time. The updater endpoint will not serve anonymous clients while the GitHub Release remains private.
- An explicit open-source license has not been selected or added.
- Microsoft Store and WinGet packaging/submission assets are not implemented.
- Final real-device checks remain for tray interaction, autostart, Windows notifications, preset/custom hooks and signed update installation.

## Need frontend

- No API contract change is currently required.
- Frontend participation is limited to final real-window acceptance and any defects found there.
