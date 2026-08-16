import { expect, test } from "vitest";
import { AGENT_STATUS_COLOR } from "./agentStatusPresentation";

test("uses the product status colors consistently", () => {
  expect(AGENT_STATUS_COLOR.running).toBe("#EF9F27");
  expect(AGENT_STATUS_COLOR.completed).toBe("#639922");
  expect(AGENT_STATUS_COLOR.idle).toBe("#72BCFF");
  expect(AGENT_STATUS_COLOR.waiting).toBe("#E24B4A");
  expect(AGENT_STATUS_COLOR.failed).toBe("#E24B4A");
  expect(AGENT_STATUS_COLOR.timeout).toBe("#E24B4A");
  expect(AGENT_STATUS_COLOR.offline).toBe("#888780");
});
