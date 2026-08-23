import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { AgentProfileStatusSummary, AgentSummary } from "../api/contracts";
import { I18nProvider } from "../i18n/I18nProvider";
import { compactAttentionSignalKeys, prioritizedAgentStatuses, visibleAgentCapacityForWidth, visibleAgentSummaries } from "./AgentStatusSlots";
import { AGENT_STATUS_COLOR } from "./agentStatusPresentation";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

afterEach(cleanup);

const agents: AgentSummary[] = [
  { agentId: "claude", displayName: "claude", aggregateStatus: "offline", environments: [{ agentId: "claude", environment: "windows", taskId: "c", status: "offline", summary: "", sourceEventId: "c", occurredAt: 1, receivedAt: 1 }], integrations: [] },
  { agentId: "workbuddy", displayName: "WorkBuddy", aggregateStatus: "running", environments: [{ agentId: "workbuddy", environment: "windows", taskId: "w", status: "running", summary: "", sourceEventId: "w", occurredAt: 10, receivedAt: 10 }], integrations: [] },
  { agentId: "hermes", displayName: "Hermes", aggregateStatus: "running", environments: [{ agentId: "hermes", environment: "windows", taskId: "h", status: "running", summary: "", sourceEventId: "h", occurredAt: 10, receivedAt: 10 }], integrations: [] },
  { agentId: "codex", displayName: "Codex", aggregateStatus: "failed", environments: [{ agentId: "codex", environment: "windows", taskId: "x", status: "failed", summary: "", sourceEventId: "x", occurredAt: 2, receivedAt: 2 }], integrations: [] },
];

const openedClaude: AgentSummary = {
  ...agents[0],
  aggregateStatus: "idle",
  environments: [{ ...agents[0].environments[0], status: "idle" }],
};

const profileSummaries: AgentProfileStatusSummary[] = [
  {
    profile: {
      id: "kimi-windows", kind: "preset", displayName: "Kimi Code", environment: "windows",
      configTarget: { kind: "preset", adapterId: "kimi" }, eventMapping: [], enabled: true,
      installationState: "installed", reasonCode: null, revision: 1, updatedAt: 2,
    },
    aggregateStatus: "running",
    observations: [{ profileId: "kimi-windows", environment: "windows", taskId: "task-1", status: "running", sourceEventId: "kimi-event-1", occurredAt: 2, receivedAt: 2 }],
  },
  {
    profile: {
      id: "qoderwork-windows", kind: "preset", displayName: "QoderWork", environment: "windows",
      configTarget: { kind: "preset", adapterId: "qoderwork" }, eventMapping: [], enabled: true,
      installationState: "notInstalled", reasonCode: null, revision: 1, updatedAt: 2,
    },
    aggregateStatus: "running",
    observations: [{ profileId: "qoderwork-windows", environment: "windows", taskId: "task-2", status: "running", sourceEventId: "qoder-event-1", occurredAt: 2, receivedAt: 2 }],
  },
];

const responsiveProfileSummaries: AgentProfileStatusSummary[] = Array.from({ length: 9 }, (_, index) => {
  const status = index < 2 ? "waiting" : index < 5 ? "running" : "idle";
  return {
    profile: {
      ...profileSummaries[0].profile,
      id: `responsive-${index}`,
      displayName: `Responsive ${index}`,
    },
    aggregateStatus: status,
    observations: [{
      profileId: `responsive-${index}`,
      environment: "windows",
      taskId: `task-${index}`,
      status,
      sourceEventId: `event-${index}`,
      occurredAt: index,
      receivedAt: index,
    }],
  };
});

test("adapts Agent capacity across narrow, medium, and wide capsules without adding fields", async () => {
  expect(visibleAgentCapacityForWidth(248)).toBe(4);
  expect(visibleAgentCapacityForWidth(420)).toBe(9);
  expect(visibleAgentCapacityForWidth(720)).toBeGreaterThanOrEqual(9);

  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const view = render(
    <I18nProvider>
      <AgentStatusSlots agents={[]} profileSummaries={responsiveProfileSummaries} compactWidth={248} onOpenAgent={vi.fn()} />
    </I18nProvider>,
  );

  expect(screen.getAllByTestId("agent-gui-logo")).toHaveLength(4);
  expect(screen.getByText("+5")).toBeInTheDocument();
  expect(screen.queryByText(/^▶/)).not.toBeInTheDocument();
  expect(screen.queryByText(/^!/)).not.toBeInTheDocument();
  expect(screen.getByText("工作中")).toBeInTheDocument();
  expect(screen.queryByText("Responsive 0")).not.toBeInTheDocument();

  view.rerender(
    <I18nProvider>
      <AgentStatusSlots agents={[]} profileSummaries={responsiveProfileSummaries} compactWidth={420} onOpenAgent={vi.fn()} />
    </I18nProvider>,
  );
  expect(screen.getAllByTestId("agent-gui-logo")).toHaveLength(9);
  expect(screen.queryByText(/^\+\d+$/)).not.toBeInTheDocument();

  view.rerender(
    <I18nProvider>
      <AgentStatusSlots agents={[]} profileSummaries={responsiveProfileSummaries} compactWidth={720} onOpenAgent={vi.fn()} />
    </I18nProvider>,
  );
  expect(screen.getAllByTestId("agent-gui-logo")).toHaveLength(9);
  expect(screen.queryByText(/^\+\d+$/)).not.toBeInTheDocument();
});

test("compact window shows every Agent as a GUI logo with one working-or-idle summary", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  render(<I18nProvider><AgentStatusSlots agents={[openedClaude, ...agents.slice(1)]} onOpenAgent={vi.fn()} /></I18nProvider>);

  expect(screen.getAllByTestId("agent-gui-logo")).toHaveLength(4);
  expect(screen.getByText("工作中")).toBeInTheDocument();
  expect(screen.queryByText("WorkBuddy")).not.toBeInTheDocument();
});

test("uses yellow blinking for work, green steady for completion, sky blue steady for idle, and gray for offline", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const running: AgentSummary = {
    ...agents[0],
    aggregateStatus: "running",
    environments: [{ ...agents[0].environments[0], status: "running" }],
  };
  const completed: AgentSummary = {
    ...agents[0],
    aggregateStatus: "completed",
    environments: [{ ...agents[0].environments[0], status: "completed" }],
  };
  const idle: AgentSummary = {
    ...agents[0],
    aggregateStatus: "idle",
    environments: [{ ...agents[0].environments[0], status: "idle" }],
  };
  const { rerender } = render(<I18nProvider><AgentStatusSlots agents={[running]} onOpenAgent={vi.fn()} /></I18nProvider>);

  expect(document.querySelector(".agent-compact-state .status-dot")).toHaveStyle({ background: "#EF9F27" });
  expect(document.querySelector(".agent-compact-state .status-dot")).toHaveClass("status-dot--pulse");

  rerender(<I18nProvider><AgentStatusSlots agents={[completed]} onOpenAgent={vi.fn()} /></I18nProvider>);
  expect(document.querySelector(".agent-compact-state .status-dot")).toHaveStyle({ background: "#639922" });
  expect(document.querySelector(".agent-compact-state .status-dot")).not.toHaveClass("status-dot--pulse");

  rerender(<I18nProvider><AgentStatusSlots agents={[idle]} onOpenAgent={vi.fn()} /></I18nProvider>);
  expect(document.querySelector(".agent-compact-state .status-dot")).toHaveStyle({ background: "#72BCFF" });
  expect(document.querySelector(".agent-compact-state .status-dot")).not.toHaveClass("status-dot--pulse");
  expect(AGENT_STATUS_COLOR.offline).toBe("#888780");
});

test("hides fixed Agents that have no live desktop or terminal source", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const unopened: AgentSummary = agents[0];

  render(<I18nProvider><AgentStatusSlots agents={[unopened, agents[1]]} onOpenAgent={vi.fn()} /></I18nProvider>);

  expect(visibleAgentSummaries([unopened, agents[1]])).toEqual([agents[1]]);
  expect(document.querySelector('[data-agent-id="claude"]')).toBeNull();
  expect(document.querySelector('[data-agent-id="workbuddy"]')).toBeInTheDocument();
});

test("keeps priority order while making every fixed Agent logo directly available", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const onOpenAgent = vi.fn();
  const user = userEvent.setup();
  render(<I18nProvider><AgentStatusSlots agents={[openedClaude, ...agents.slice(1)]} onOpenAgent={onOpenAgent} /></I18nProvider>);

  expect(screen.getAllByRole("button").filter((button) => button.hasAttribute("data-agent-id")).map((button) => button.getAttribute("data-agent-id")))
    .toEqual(["codex", "hermes", "workbuddy", "claude"]);
  await user.click(screen.getByRole("button", { name: /^claude/ }));
  expect(onOpenAgent).toHaveBeenCalledWith("claude");
});

test("binds compact attention identity to the observation that actually supplies the aggregate status", () => {
  const failedAgent: AgentSummary = {
    agentId: "codex",
    displayName: "Codex",
    aggregateStatus: "failed",
    integrations: [],
    environments: [
      { agentId: "codex", environment: "windows", taskId: "failed", status: "failed", summary: "", sourceEventId: "failed-10", occurredAt: 10, receivedAt: 10 },
      { agentId: "codex", environment: "wsl", taskId: "idle", status: "idle", summary: "", sourceEventId: "idle-20", occurredAt: 20, receivedAt: 20 },
    ],
  };
  const waitingProfile: AgentProfileStatusSummary = {
    profile: {
      ...profileSummaries[0].profile,
      id: "waiting-profile",
      displayName: "Waiting profile",
    },
    aggregateStatus: "waiting",
    observations: [
      { profileId: "waiting-profile", environment: "windows", taskId: "waiting", status: "waiting", sourceEventId: "waiting-30", occurredAt: 30, receivedAt: 30 },
      { profileId: "waiting-profile", environment: "wsl", taskId: "idle", status: "idle", sourceEventId: "idle-40", occurredAt: 40, receivedAt: 40 },
    ],
  };

  expect(compactAttentionSignalKeys([failedAgent], [waitingProfile])).toEqual([
    "profile:waiting-profile:waiting:30",
    "agent:codex:failed:10",
  ]);

  const unrelatedUpdates = {
    ...failedAgent,
    environments: failedAgent.environments.map((observation) => observation.status === "idle"
      ? { ...observation, sourceEventId: "idle-21", occurredAt: 21, receivedAt: 21 }
      : observation),
  };
  const unrelatedProfileUpdates = {
    ...waitingProfile,
    observations: waitingProfile.observations.map((observation) => observation.status === "idle"
      ? { ...observation, sourceEventId: "idle-41", occurredAt: 41, receivedAt: 41 }
      : observation),
  };

  expect(compactAttentionSignalKeys([unrelatedUpdates], [unrelatedProfileUpdates])).toEqual([
    "profile:waiting-profile:waiting:30",
    "agent:codex:failed:10",
  ]);
});

test("qualifies completed, failed, waiting, and timeout observations for compact attention", () => {
  const qualifiedAgents: AgentSummary[] = [
    { agentId: "codex", displayName: "Codex", aggregateStatus: "completed", integrations: [], environments: [{ agentId: "codex", environment: "windows", taskId: "done", status: "completed", summary: "", sourceEventId: "done-1", occurredAt: 1, receivedAt: 1 }] },
    { agentId: "hermes", displayName: "Hermes", aggregateStatus: "failed", integrations: [], environments: [{ agentId: "hermes", environment: "windows", taskId: "failed", status: "failed", summary: "", sourceEventId: "failed-2", occurredAt: 2, receivedAt: 2 }] },
    { agentId: "workbuddy", displayName: "WorkBuddy", aggregateStatus: "waiting", integrations: [], environments: [{ agentId: "workbuddy", environment: "windows", taskId: "waiting", status: "waiting", summary: "", sourceEventId: "waiting-3", occurredAt: 3, receivedAt: 3 }] },
    { agentId: "claude", displayName: "claude", aggregateStatus: "timeout", integrations: [], environments: [{ agentId: "claude", environment: "windows", taskId: "timeout", status: "timeout", summary: "", sourceEventId: "timeout-4", occurredAt: 4, receivedAt: 4 }] },
  ];

  expect(new Set(compactAttentionSignalKeys(qualifiedAgents, []))).toEqual(new Set([
    "agent:codex:completed:1",
    "agent:hermes:failed:2",
    "agent:workbuddy:waiting:3",
    "agent:claude:timeout:4",
  ]));
});

test("omits a zero overflow control and keeps slot interactions out of the drag region", async () => {
  // Adding +0 or allowing a slot pointer event to bubble into dragging must fail this contract.
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const startDrag = vi.fn();
  render(<div onPointerDown={startDrag}><I18nProvider><AgentStatusSlots agents={[openedClaude, agents[1]]} onOpenAgent={vi.fn()} /></I18nProvider></div>);

  expect(screen.getAllByRole("button").filter((button) => button.hasAttribute("data-agent-id"))).toHaveLength(2);
  expect(screen.queryByText("+0")).not.toBeInTheDocument();
  fireEvent.pointerDown(screen.getByRole("button", { name: /^WorkBuddy/ }));
  expect(startDrag).not.toHaveBeenCalled();
});

test("opens a logo from the keyboard without exposing hidden Agent names in the compact bar", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const onOpenAgent = vi.fn();
  const user = userEvent.setup();
  render(<I18nProvider><AgentStatusSlots agents={agents} onOpenAgent={onOpenAgent} /></I18nProvider>);

  const codex = screen.getByRole("button", { name: /^Codex/ });
  codex.focus();
  await user.keyboard("{Enter}");
  expect(onOpenAgent).toHaveBeenCalledWith("codex");
  expect(screen.queryByText("Codex")).not.toBeInTheDocument();
});

test("shows running preset apps before Hook installation without claiming they are installed", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const onOpenProfile = vi.fn();
  const user = userEvent.setup();
  render(<I18nProvider><AgentStatusSlots agents={[]} profileSummaries={profileSummaries} onOpenAgent={vi.fn()} onOpenProfile={onOpenProfile} /></I18nProvider>);

  expect(screen.getAllByTestId("agent-gui-logo")).toHaveLength(2);
  expect(document.querySelector(".agent-gui-logo--profile-kimi")).toBeInTheDocument();
  expect(document.querySelector('[data-profile-id="qoderwork-windows"]')).toBeInTheDocument();

  await user.click(document.querySelector('[data-profile-id="kimi-windows"]') as HTMLButtonElement);
  expect(onOpenProfile).toHaveBeenCalledWith("kimi-windows");
});

test("uses packaged desktop icons for installed Agent applications", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  const { rerender } = render(<I18nProvider><AgentStatusSlots agents={[openedClaude, ...agents.slice(1)]} onOpenAgent={vi.fn()} /></I18nProvider>);

  expect(document.querySelector('[data-agent-id="codex"] img.agent-gui-logo__image')).toHaveAttribute("src", expect.stringContaining("codex"));
  expect(document.querySelector('[data-agent-id="hermes"] img.agent-gui-logo__image')).toHaveAttribute("src", expect.stringContaining("hermes"));
  expect(document.querySelector('[data-agent-id="workbuddy"] img.agent-gui-logo__image')).toHaveAttribute("src", expect.stringContaining("workbuddy"));
  expect(document.querySelector('[data-agent-id="claude"] img.agent-gui-logo__image')).toHaveAttribute("src", expect.stringContaining("claude"));

  rerender(<I18nProvider><AgentStatusSlots agents={[]} profileSummaries={[
    profileSummaries[0],
    {
      profile: {
        id: "trae-windows", kind: "preset", displayName: "TRAE", environment: "windows",
        configTarget: { kind: "preset", adapterId: "trae" }, eventMapping: [], enabled: true,
        installationState: "installed", reasonCode: null, revision: 1, updatedAt: 12,
      },
      aggregateStatus: "idle", observations: [],
    },
  ]} onOpenAgent={vi.fn()} /></I18nProvider>);

  expect(document.querySelector('[data-profile-id="kimi-windows"] img.agent-gui-logo__image')).toHaveAttribute("src", expect.stringContaining("kimi"));
  expect(document.querySelector('[data-profile-id="trae-windows"] img.agent-gui-logo__image')).toHaveAttribute("src", expect.stringContaining("trae"));
});

test("shows only currently observed dynamic Profiles, including connectable preset apps", () => {
  const statuses = prioritizedAgentStatuses([], [
    {
      profile: {
        id: "custom-1", kind: "custom", displayName: "Build hook", environment: "windows",
        configTarget: { kind: "customHook", executable: "C:\\Tools\\hook.exe", argv: [], workingDirectory: null, timeoutSeconds: 30 },
        eventMapping: [], enabled: true, installationState: "installed", reasonCode: null, revision: 1, updatedAt: 3,
      },
      aggregateStatus: "running",
      observations: [{ profileId: "custom-1", environment: "windows", taskId: "task-1", status: "running", sourceEventId: "custom-1", occurredAt: 3, receivedAt: 3 }],
    },
    {
      profile: {
        id: "qoderwork-windows", kind: "preset", displayName: "QoderWork", environment: "windows",
        configTarget: { kind: "preset", adapterId: "qoderwork" }, eventMapping: [], enabled: true,
        installationState: "installed", reasonCode: null, revision: 1, updatedAt: 2,
      },
      aggregateStatus: "offline",
      observations: [],
    },
    {
      profile: {
        id: "trae-windows", kind: "preset", displayName: "TRAE", environment: "windows",
        configTarget: { kind: "preset", adapterId: "trae" }, eventMapping: [], enabled: true,
        installationState: "unsupported", reasonCode: "traeGlobalTargetMissing", revision: 1, updatedAt: 4,
      },
      aggregateStatus: "running",
      observations: [],
    },
  ]);

  expect(statuses).toEqual(["running", "running"]);
});

test("keeps a running Custom Hook pulsing and hides an offline preset", async () => {
  const componentPath = "./AgentStatusSlots";
  const { default: AgentStatusSlots } = await import(componentPath);
  render(<I18nProvider><AgentStatusSlots agents={[]} onOpenAgent={vi.fn()} profileSummaries={[
    {
      profile: {
        id: "custom-1", kind: "custom", displayName: "Build hook", environment: "windows",
        configTarget: { kind: "customHook", executable: "C:\\Tools\\hook.exe", argv: [], workingDirectory: null, timeoutSeconds: 30 },
        eventMapping: [], enabled: true, installationState: "installed", reasonCode: null, revision: 1, updatedAt: 2,
      },
      aggregateStatus: "running", observations: [{ profileId: "custom-1", environment: "windows", taskId: "task-1", status: "running", sourceEventId: "custom-1", occurredAt: 2, receivedAt: 2 }],
    },
    {
      profile: {
        id: "qoderwork-windows", kind: "preset", displayName: "QoderWork", environment: "windows",
        configTarget: { kind: "preset", adapterId: "qoderwork" }, eventMapping: [], enabled: true,
        installationState: "installed", reasonCode: null, revision: 1, updatedAt: 1,
      },
      aggregateStatus: "offline", observations: [],
    },
  ]} /></I18nProvider>);

  const custom = document.querySelector('[data-profile-id="custom-1"]');
  const offline = document.querySelector('[data-profile-id="qoderwork-windows"]');
  expect(custom?.querySelector(".status-dot")).toHaveClass("status-dot--pulse");
  expect(offline).toBeNull();
});
