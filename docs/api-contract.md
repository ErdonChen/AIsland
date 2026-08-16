# API contract

## Agent status snapshot

- Command: `getAgentsSnapshot()`
- Return: `AgentsSnapshot { agents, generatedAt }`
- `AgentObservation.latestReplyPreview?: string | null` is a bounded display preview of the newest completed Agent-authored reply.
- Producers may populate it only from a verified assistant-only field: `last_assistant_message` on Stop-compatible Hooks, Hermes `extra.assistant_response` on `post_llm_call`, or the equivalent allowlisted profile field. User prompts, conversation history, reasoning, tool bodies/output, generic native `message` fields, and process-presence text are not valid sources.
- Consumers must treat an absent or null preview as “no recent reply” and must not infer reply text from `taskId` or `summary`.
- `AgentProfileObservation.latestReplyPreview?: string | null` follows the same bounded/privacy contract. Profile status events without a new assistant preview retain the latest verified preview for that task.
- Legacy desktop-process presence is a fallback `idle` observation, never proof that an Agent is actively working. Explicit Hook events win for running, completed, waiting, failed, and latest-reply state.
- Windows preset process detection recognizes Kimi Code, TRAE, and QoderWork by exact executable basenames. It may surface a profile as idle/connectable without changing its truthful installation state.
- Live status colors are fixed: running yellow with pulse, completed green steady, idle sky blue steady, offline gray steady; failed/waiting/timeout remain red attention states.
- For concurrent tasks belonging to the same Agent, the steady lifecycle display priority is `running > completed > idle > offline`. A newly completed sibling temporarily overrides `running` with green `completed` for 2 seconds from receipt; after that window the aggregate returns to yellow/pulsing `running`. If no task remains running, green is retained.

## Agent integration discovery

- Command: `discoverAgentIntegrationCandidates()`.
- The command performs a bounded, read-only scan of exact running-process names and known local configuration/application markers. It never writes vendor configuration and never returns file contents.
- Results distinguish `automatic`, `readyToInstall`, `detectionPending`, and `adapterRequired`. Detecting an Agent application is not proof that it implements the Custom Hook protocol.
- The Settings one-click action automatically invokes the existing revision-checked install/repair command only for a running Windows Agent with a `readyToInstall` preset. Each candidate is attempted independently. Built-in, disabled, already-installed, unsupported, non-running, and custom candidates are never auto-installed; a custom result may only prepare an unsaved draft.

## Daily Notes startup

- `getDailyNote({ noteDate })` is authoritative for the note ID and revision before editing is enabled.
- A transient startup failure is retried with bounded exponential backoff while the editor stays disabled. The UI also exposes a local Retry action.
- Autosave uses `createNote` only after an authoritative null result; otherwise it uses `updateNote` with the loaded revision.

## Window lifecycle

- Command: `hide_island_to_tray()`
- Behavior: hides the main AIsland window only after the native tray icon exists. The process remains running and the tray icon restores the window.

## Reminder presentation

- The legacy `reminder-alert` webview remains a compatibility resource but is never shown by production reminder delivery.
- Agent reminder notifications may supply the latest activity fallback for their matching Agent card only. They never populate another Agent card.
- Non-Agent reminders remain available through Windows Toast and Notification Center and do not expand the Agent status surface.
