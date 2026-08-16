import { describe, expect, it } from "vitest";
import { parseCommandError } from "./commandError";
import type {
  AgentId,
  ReminderSound,
  ReminderSourceContext,
  ServiceHealthSnapshot,
  DiagnosticEvent,
  StorageIntegrityResult,
  MediaControlInput,
  AdvanceOnboardingInput,
} from "./contracts";

type Equal<Left, Right> = (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2) ? true : false;
type Assert<Condition extends true> = Condition;
const staticContractAssertions: [
  Assert<Equal<Extract<AgentId, "claude">, "claude">>,
  Assert<Equal<Extract<ReminderSound, { kind: "localFile" }>["canonicalPath"], string>>,
  Assert<Equal<Extract<ReminderSourceContext, { kind: "agent" }>["agentId"], AgentId>>,
  Assert<Equal<keyof ServiceHealthSnapshot, "serviceId" | "state" | "messageKey" | "parameters" | "checkedAt">>,
] = [true, true, true, true];
void staticContractAssertions;
const payloadContractAssertions = [
  { id: "diag-1", serviceId: "clipboard", level: "warning", code: "locked", parameters: { count: 2 }, createdAt: 12 } satisfies DiagnosticEvent,
  { integrity: "ok", schemaVersion: 1, checkedAt: 12 } satisfies StorageIntegrityResult,
  { command: "seek", positionSeconds: 3 } satisfies MediaControlInput,
  { nextStep: "modules", locale: "zh-CN", modulePreferences: [], privacyConsent: null, expectedRevision: 2 } satisfies AdvanceOnboardingInput,
];
void payloadContractAssertions;

describe("parseCommandError", () => {
  it("preserves a valid typed envelope", () => {
    expect(parseCommandError({ code: "conflict", messageKey: "errors.conflict", details: { entityId: "item-1" }, retryable: true })).toEqual({ code: "conflict", messageKey: "errors.conflict", details: { entityId: "item-1" }, retryable: true });
  });

  it("maps an untyped rejection to the fixed io envelope", () => {
    expect(parseCommandError("database closed")).toEqual({ code: "ioFailure", messageKey: "errors.ioFailure", details: {}, retryable: false });
  });

  it("preserves numeric allowlisted removal counts", () => {
    expect(parseCommandError({ code: "invalidInput", messageKey: "settings.storage.retentionConfirmationRequired", details: { clipboardRemovalCount: 12, notificationRemovalCount: 4 }, retryable: true })).toMatchObject({ details: { clipboardRemovalCount: 12, notificationRemovalCount: 4 } });
  });

  it("rejects a sensitive or message-key-mismatched detail", () => {
    expect(parseCommandError({ code: "ioFailure", messageKey: "errors.ioFailure", details: { body: "secret", entityId: "C:\\Users\\name\\secret.txt" }, retryable: false })).toEqual({ code: "ioFailure", messageKey: "errors.ioFailure", details: {}, retryable: false });
  });

  it("copies valid details and creates an independent fallback for every call", () => {
    const input = { code: "conflict", messageKey: "errors.conflict", details: { entityId: "item-1" }, retryable: true };
    const valid = parseCommandError(input);
    input.details.entityId = "mutated";
    expect(valid.details).toEqual({ entityId: "item-1" });
    const first = parseCommandError("bad");
    const second = parseCommandError("bad");
    (first.details as Record<string, string>).reasonCode = "mutated";
    expect(second).toEqual({ code: "ioFailure", messageKey: "errors.ioFailure", details: {}, retryable: false });
  });
});
