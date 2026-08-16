# Current Architecture Decisions

Last updated: 2026-08-17. This file records only decisions that are still active. Historical plans are not part of the public product scope.

## Product surface

- Production pages are Home, Daily Notes, Clipboard, Monitor, Notifications, and Settings.
- Todo and media-control runtime surfaces are retired. Their historical migrations, stored data, event names, and reminder-source parsing remain for backward compatibility; they must not be reintroduced into navigation, command handlers, or workers.
- The compact window is 248 x 46 logical pixels. Hover may expand it, double-click locks the expanded state, and loss of window focus may collapse an unlocked window. Native geometry and web content must share the same top-center anchor, scale, reduced-motion decision, and rounded shape.
- `set_island_mode` resolves only after native geometry is applied and committed, and rejects on native failure or supersession. React renders each requested visual mode immediately, dispatches newer mode intents without waiting for older native animations, ignores stale results, and rolls a failed latest intent back to the latest mode actually confirmed by Rust.

## Agent integrations

- V1 Agent capability registry is a trusted compile-time list, not an end-user JSONL/SQLite/JSONPath/SQL parser. Each entry declares exact discovery evidence, native adapter kind, precise-status support, assistant-only preview support, stable-task-ID support, Hook fallback, and platform scope. Unknown vendor formats must use Custom Hook or a reviewed native-adapter contribution; AIsland never guesses how to parse arbitrary local stores.
- Presets are Kimi Code (`kimi`), TRAE (`trae`), and QoderWork (`qoderwork`). A preset profile identity is `${adapterId}-${environment}` so Windows and WSL revisions remain independent.
- The backend may retain Codex, Hermes, WorkBuddy, and claude as compatibility rows, but Home and compact status surfaces show an Agent only while its desktop GUI, terminal process, or active Hook is currently observed. Offline and never-opened rows remain available to history/settings but are hidden from the live island.
- Desktop process presence is only an `idle`/connectable fallback. It must never claim that work is active or overwrite an explicit Hook completion/idle/failure state. Kimi Code, TRAE, and QoderWork use exact Windows process-name detection for this fallback while keeping installation state truthful.
- Windows Kimi Code and QoderWork use owned bridge configuration plus the durable profile-event spool. TRAE remains detection-pending until a verified vendor target is available. Unsupported states must fail closed and must not be presented as installed.
- Custom Hook is a separate profile kind. V1 accepts a canonical existing Windows `.exe`, separate argv entries, and strict ready/NDJSON event mapping. Custom WSL remains unsupported.
- One-click discovery is capability-aware: built-in tracking, supported presets, pending vendor compatibility, and custom-adapter-required are separate states. The scan itself stays read-only; the same explicit button action may install/repair every running safe preset through the existing revisioned rollback protocol. Installed-only, disabled, unsupported, built-in, or custom candidates are not auto-installed, and an application executable is never auto-filled as a Custom Hook.
- `agentProfileStateChanged { profileId, sourceEventId, occurredAt }` is only an invalidation hint. UI state is rebuilt from `getAgentProfilesSnapshot`.
- Built-in WSL Hooks write their locked `*-wsl.json` files into the same Windows app-data `agent-status` directory watched by AIsland. Startup resolves that Windows directory through `wslpath`; it must not use a WSL-private status directory or assume the default `/mnt/c` mount layout.
- Installed Hook profiles contribute detailed status while non-offline. A detected preset desktop app may appear as idle/connectable before Hook installation, but `notInstalled`/`unsupported` remains visible in Settings and is never relabeled as installed. Offline profiles stay hidden from the live island.
- Every installed integration actively attempts reply preview extraction only through a vendor-verified assistant-only field. Hermes `post_llm_call` uses `extra.assistant_response`; Stop-compatible built-in/profile Hooks use `last_assistant_message`. Missing capability remains empty and must never fall back to user messages, conversation history, reasoning, generic messages, or tool output.
- Status presentation is locked: running is yellow and pulsing; completed is green and steady; idle is sky blue and steady; offline is gray and steady; failed, waiting, and timeout are red attention states.
- Same-Agent task aggregation uses steady priority `running > completed > idle > offline`. A sibling completion overrides the aggregate with green `completed` for 2 seconds from receipt, then the island returns to yellow/pulsing while another task remains running; an all-finished aggregate remains green.

## Daily Notes

- Daily Notes must load the authoritative daily ID/revision before enabling the editor. Startup State-registration failures retry with bounded exponential backoff and a local Retry action; edits must not fall through to a duplicate create.
- Note bodies are UTF-8 Markdown. ASCII, Chinese text, and multiline content must round-trip through SQLite without transformation.

## Monitor and notifications

- Monitor and notification subscriptions register the listener before the first authoritative command load. A user Retry must actually retry listener registration; the UI must not report listener recovery while only polling remains.
- `monitorMetricsChanged { sampledAt }` and `notificationHistoryChanged { newestReceivedAt, origin }` are invalidation hints, never content-bearing events.
- Notification content is reloaded through `listNotificationHistory`. The compact popup reacts only to Windows-origin hints when both notification product switches are enabled. Burst reloads are single-flight with a trailing dirty reload.
- The standalone reminder-alert window is retired and must never be shown. Agent-related status notifications are projected into the corresponding Agent card; unrelated reminders stay in Windows Toast and Notification Center surfaces.

## Persistence and lifecycle

- SQLite is authoritative for product data. Browser storage is limited to lightweight UI preferences.
- Workers have one AppServices owner, register before starting, stop accepting work during shutdown, join before the final checkpoint, and fence stale generations before side effects.
- Preset configuration rollback is a crash-recoverable, identity-and-hash-guarded journal protocol. Unknown target or sidecar state must fail closed rather than overwrite or delete third-party data.

## Release and distribution

- GitHub Release updater artifacts are signed. The runtime and workflow remain fail closed while the real updater endpoint, public key, and signing secrets are absent.
- Signing private keys exist only in GitHub Secrets. Do not commit placeholder keys or publish `latest.json` without its matching installer and signature.
- AIsland Community Edition is licensed under Apache-2.0, copyright 2026 Erdon Chen. Community contributions use the same license and require DCO 1.1 sign-off; no CLA is required.
- Existing Community features remain available in Community. Future Pro functionality is developed as separate proprietary modules and does not relicense community contributions.
- Source and build instructions may be published before binaries. No installer or portable executable may be published until it has a valid trusted Authenticode signature, verified publisher identity, and timestamp in addition to the Tauri updater signature.
- The AIsland name and logo identify official releases. Modified distributions must use another name and visual identity as described in `TRADEMARKS.md`.
