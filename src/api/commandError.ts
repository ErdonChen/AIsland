import { type AppErrorCode, type CommandError, validateMessageParameters } from "./contracts";
const fallback = (): CommandError => ({ code: "ioFailure", messageKey: "errors.ioFailure", details: {}, retryable: false });
const codes = new Set<AppErrorCode>(["invalidInput", "notFound", "conflict", "storageUnavailable", "databaseFailure", "ioFailure", "permissionDenied", "sourceUnavailable", "platformUnsupported", "integrationUnsupported", "integrationNotInstalled", "integrationConfigInvalid", "notificationUnavailable"]);
export function parseCommandError(value: unknown): CommandError {
  if (!value || typeof value !== "object" || Array.isArray(value)) return fallback();
  const candidate = value as Record<string, unknown>;
  if (typeof candidate.code !== "string" || !codes.has(candidate.code as AppErrorCode) || typeof candidate.messageKey !== "string" || typeof candidate.retryable !== "boolean" || !validateMessageParameters("commandError", candidate.messageKey, candidate.details)) return fallback();
  return { code: candidate.code as AppErrorCode, messageKey: candidate.messageKey, details: { ...candidate.details }, retryable: candidate.retryable };
}
