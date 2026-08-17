import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import type { AgentProfileStatusSummary, AgentSummary } from "../api/contracts";
import { I18nProvider } from "../i18n/I18nProvider";
import "../App.css";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

const agents: AgentSummary[] = [
  {
    agentId: "claude", displayName: "claude", aggregateStatus: "running",
    environments: [
      { agentId: "claude", environment: "windows", taskId: "claude-win", status: "running", summary: "Windows work", sourceEventId: "cw", occurredAt: 40, receivedAt: 40 },
      { agentId: "claude", environment: "wsl", taskId: "claude-wsl", status: "completed", summary: "WSL work", sourceEventId: "cl", occurredAt: 30, receivedAt: 30 },
    ], integrations: [],
  },
  {
    agentId: "workbuddy", displayName: "WorkBuddy", aggregateStatus: "running",
    environments: [{ agentId: "workbuddy", environment: "windows", taskId: "wb", status: "running", summary: "Desktop work", sourceEventId: "wb", occurredAt: 40, receivedAt: 40 }], integrations: [],
  },
  {
    agentId: "hermes", displayName: "Hermes", aggregateStatus: "running",
    environments: [
      { agentId: "hermes", environment: "windows", taskId: "hermes-win", status: "running", summary: "Windows work", sourceEventId: "hw", occurredAt: 40, receivedAt: 40 },
      { agentId: "hermes", environment: "wsl", taskId: "hermes-wsl", status: "idle", summary: "WSL work", sourceEventId: "hl", occurredAt: 20, receivedAt: 20 },
    ], integrations: [],
  },
  {
    agentId: "codex", displayName: "Codex", aggregateStatus: "running",
    environments: [
      { agentId: "codex", environment: "windows", taskId: "codex-win", status: "running", summary: "Windows work", sourceEventId: "xw", occurredAt: 40, receivedAt: 40 },
      { agentId: "codex", environment: "wsl", taskId: "codex-wsl", status: "failed", summary: "WSL work", sourceEventId: "xl", occurredAt: 10, receivedAt: 10 },
    ], integrations: [],
  },
];

afterEach(cleanup);

test("renders the four fixed agents in stable card order with authoritative environment records", async () => {
  // Removing fixed ordering, aggregate status, or an observed source badge must fail this UI contract.
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  render(<I18nProvider><AgentsPage agents={agents.slice().reverse()} /></I18nProvider>);

  expect(screen.getAllByRole("article").map((card) => card.getAttribute("data-agent-id")))
    .toEqual(["codex", "hermes", "workbuddy", "claude"]);
  expect(screen.getByRole("article", { name: /Codex.*运行中.*2.*Windows.*WSL/ })).toBeInTheDocument();
  expect(screen.getByRole("article", { name: /WorkBuddy.*运行中.*1.*Windows/ })).toBeInTheDocument();
  expect(screen.queryByTestId("agent-environment-workbuddy-wsl")).not.toBeInTheDocument();
  for (const agentId of ["codex", "hermes", "claude"]) {
    expect(screen.getByTestId(`agent-environment-${agentId}-windows`)).toBeInTheDocument();
    expect(screen.getByTestId(`agent-environment-${agentId}-wsl`)).toBeInTheDocument();
  }
});

test("shows native and Profile Agents in a scrollable list with the latest status update first", async () => {
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const profileSummaries: AgentProfileStatusSummary[] = [{
    profile: {
      id: "trae-windows", kind: "preset", displayName: "TRAE", environment: "windows",
      configTarget: { kind: "preset", adapterId: "trae" }, eventMapping: [], enabled: true,
      installationState: "installed", reasonCode: null, revision: 1, updatedAt: 80,
    },
    aggregateStatus: "running",
    observations: [{ profileId: "trae-windows", environment: "windows", taskId: "trae-task", status: "running", latestReplyPreview: "Profile assistant reply", sourceEventId: "trae-event", occurredAt: 80, receivedAt: 80 }],
  }];
  const codex = { ...agents[3], environments: agents[3].environments.map((observation) => ({ ...observation, receivedAt: 100 })) };
  const workbuddy = { ...agents[1], environments: agents[1].environments.map((observation) => ({ ...observation, receivedAt: 90 })) };

  const { rerender } = render(<I18nProvider><AgentsPage agents={[codex, workbuddy]} profileSummaries={profileSummaries} /></I18nProvider>);

  expect(screen.getAllByRole("article").map((card) => card.getAttribute("data-status-source")))
    .toEqual(["agent:codex", "agent:workbuddy", "profile:trae-windows"]);
  expect(screen.getByRole("article", { name: /TRAE.*运行中/ })).toHaveTextContent("Profile assistant reply");

  const updatedProfiles = [{
    ...profileSummaries[0],
    observations: [{ ...profileSummaries[0].observations[0], occurredAt: 120, receivedAt: 120 }],
  }];
  rerender(<I18nProvider><AgentsPage agents={[codex, workbuddy]} profileSummaries={updatedProfiles} /></I18nProvider>);

  expect(screen.getAllByRole("article").map((card) => card.getAttribute("data-status-source")))
    .toEqual(["profile:trae-windows", "agent:codex", "agent:workbuddy"]);
  const list = document.querySelector(".agents-grid") as HTMLElement;
  expect(getComputedStyle(list).overflowY).toBe("auto");
  expect(getComputedStyle(list).gridTemplateColumns).toBe("minmax(0, 1fr)");
});

test("keeps the native claude name lowercase when UI language changes", async () => {
  // Capitalizing or translating the fixed native name must fail this contract.
  localStorage.setItem("aisland.ui.language", "en-US");
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  render(<I18nProvider><AgentsPage agents={agents} /></I18nProvider>);

  expect(await screen.findByRole("article", { name: /claude.*Running/ })).toBeInTheDocument();
  expect(screen.queryByText("Claude")).not.toBeInTheDocument();
});

test("renders the shell-ranked snapshot order and exposes the selected agent task context", async () => {
  // Re-sorting a supplied snapshot or treating selection as decoration must fail this contract.
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const shellRanked = [agents[0], agents[1], agents[3], agents[2]];
  render(<I18nProvider><AgentsPage agents={shellRanked} selectedAgentId="codex" /></I18nProvider>);

  expect(screen.getAllByRole("article").map((card) => card.getAttribute("data-agent-id")))
    .toEqual(["claude", "workbuddy", "codex", "hermes"]);
  expect(screen.getByRole("region", { name: "Codex" })).toHaveTextContent("codex-win");
  expect(screen.getByRole("region", { name: "Codex" })).toHaveTextContent("codex-wsl");
});

test("marks only the exact selected environment, task, and trigger-status row", async () => {
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const sameTaskDifferentStatus: AgentSummary = {
    ...agents[3],
    environments: [
      { ...agents[3].environments[0], taskId: "task:colon:id", status: "failed", sourceEventId: "failed" },
      { ...agents[3].environments[0], taskId: "task:colon:id", status: "completed", sourceEventId: "completed" },
    ],
  };
  render(<I18nProvider><AgentsPage agents={[sameTaskDifferentStatus]} selectedAgentId="codex" selectedContext={{ environment: "windows", taskId: "task:colon:id", triggerStatus: "failed" }} /></I18nProvider>);

  expect(screen.getAllByText("task:colon:id")[0].closest(".agent-detail__row")).toHaveAttribute("aria-current", "true");
  expect(screen.getAllByText("task:colon:id")[1].closest(".agent-detail__row")).not.toHaveAttribute("aria-current");
});

test("renders the translated no-data task fallback for a selected offline agent", async () => {
  // A zero-observation snapshot is legitimate state, not permission to render an empty detail shell.
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const offlineAgent: AgentSummary = { ...agents[1], aggregateStatus: "offline", environments: [] };
  render(<I18nProvider><AgentsPage agents={[offlineAgent]} selectedAgentId="workbuddy" /></I18nProvider>);

  expect(screen.getByRole("region", { name: "WorkBuddy" })).toHaveTextContent("暂无任务状态");
});

test("bounds a multi-observation selected detail region and enables scrolling", async () => {
  // Unbounded observations must not push the selected detail outside the fixed shell.
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const observations = Array.from({ length: 8 }, (_, index) => ({
    ...agents[3].environments[0],
    taskId: `task-${index}`,
    sourceEventId: `source-${index}`,
  }));
  const busyAgent: AgentSummary = { ...agents[3], environments: observations };
  render(<I18nProvider><AgentsPage agents={[busyAgent]} selectedAgentId="codex" /></I18nProvider>);

  const detail = screen.getByRole("region", { name: "Codex" });
  expect(detail).toHaveClass("agent-detail");
  expect(detail.querySelectorAll(".agent-detail__row")).toHaveLength(8);
  expect(getComputedStyle(detail).maxHeight).toBe("96px");
  expect(getComputedStyle(detail).overflowY).toBe("auto");
});

test("shows only the latest Agent reply preview and never presents process presence as conversation", async () => {
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const agentWithReply = {
    ...agents[3],
    environments: [{
      ...agents[3].environments[0],
      taskId: "process-presence",
      summary: "",
      latestReplyPreview: "已完成修复，全部定向测试均已通过。",
    }],
  } as AgentSummary;
  render(<I18nProvider><AgentsPage agents={[agentWithReply]} /></I18nProvider>);

  const card = screen.getByRole("article", { name: /Codex.*运行中/ });
  expect(card).toHaveTextContent("最近回复");
  expect(card).toHaveTextContent("已完成修复，全部定向测试均已通过。");
  expect(card).not.toHaveTextContent("process-presence");
  const replyText = card.querySelector(".agent-card__reply-text");
  expect(replyText).toHaveClass("agent-card__reply-text");
  expect(card).toHaveStyle({ overflow: "hidden" });
  expect((replyText as HTMLElement).style.height).toBe("2.9em");
});

test("uses a neutral reply fallback when the Agent has not exposed a safe reply preview", async () => {
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const processOnly = {
    ...agents[1],
    environments: [{
      ...agents[1].environments[0],
      taskId: "process-presence",
      summary: "",
    }],
  };
  render(<I18nProvider><AgentsPage agents={[processOnly]} /></I18nProvider>);

  const card = screen.getByRole("article", { name: /WorkBuddy.*运行中/ });
  expect(card).toHaveTextContent("暂无最近回复");
  expect(card).not.toHaveTextContent("process-presence");
});

test("shows the latest task activity when no Agent reply preview is available", async () => {
  const componentPath = "./AgentsPage";
  const { default: AgentsPage } = await import(componentPath);
  const completedWithoutReply = {
    ...agents[3],
    aggregateStatus: "completed",
    environments: [{
      ...agents[3].environments[0],
      taskId: "qa-status-lights",
      status: "completed",
      summary: "原生状态灯验收",
      latestReplyPreview: null,
    }],
  } as AgentSummary;
  render(<I18nProvider><AgentsPage agents={[completedWithoutReply]} /></I18nProvider>);

  const card = screen.getByRole("article", { name: /Codex.*已完成/ });
  expect(card).toHaveTextContent("最新动态");
  expect(card).toHaveTextContent("原生状态灯验收 · 已完成");
  expect(card).not.toHaveTextContent("暂无最近回复");
});
