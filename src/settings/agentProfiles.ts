import type {
  AgentConfigTarget,
  AgentProfileEnvironment,
  AgentProfilePresetId,
  SaveAgentIntegrationProfileInput,
} from "../api/contracts";

export type {
  AgentConfigTarget,
  AgentEventMapping,
  AgentIntegrationProfile,
  AgentProfileEnvironment,
  AgentProfileInstallationState,
  AgentProfileKind,
  AgentProfilePresetId,
  SaveAgentIntegrationProfileInput,
} from "../api/contracts";

export interface AgentProfilePreset {
  id: AgentProfilePresetId;
  displayName: string;
  descriptionKey: "agentProfiles.kimi.description" | "agentProfiles.trae.description" | "agentProfiles.qoderwork.description" | "agentProfiles.cursor.description";
}

export const AGENT_PROFILE_PRESETS: readonly AgentProfilePreset[] = [
  { id: "kimi", displayName: "Kimi Code", descriptionKey: "agentProfiles.kimi.description" },
  { id: "trae", displayName: "TRAE", descriptionKey: "agentProfiles.trae.description" },
  { id: "qoderwork", displayName: "QoderWork", descriptionKey: "agentProfiles.qoderwork.description" },
  { id: "cursor", displayName: "Cursor", descriptionKey: "agentProfiles.cursor.description" },
];

export const CUSTOM_HOOK_LIMITS = {
  maxPathLength: 4096,
  maxArguments: 32,
  maxArgumentLength: 1024,
  minTimeoutSeconds: 1,
  maxTimeoutSeconds: 600,
  maxDisplayNameLength: 64,
  minEventMappings: 1,
  maxEventMappings: 32,
  maxNativeEventLength: 64,
} as const;

export function presetProfileId(adapterId: AgentProfilePresetId, environment: AgentProfileEnvironment) {
  return `${adapterId}-${environment}`;
}

export function createCustomHookDraft(environment: AgentProfileEnvironment): SaveAgentIntegrationProfileInput {
  return {
    id: null,
    kind: "custom",
    displayName: "Custom Hook",
    environment,
    configTarget: {
      kind: "customHook",
      executable: "",
      argv: [],
      workingDirectory: null,
      timeoutSeconds: 30,
    },
    eventMapping: [],
    enabled: true,
    expectedRevision: null,
  };
}

export type CustomHookValidationErrors = Partial<Record<"executable" | "argv" | "workingDirectory" | "timeoutSeconds", string>>;
export type CustomHookProfileValidationErrors = Partial<Record<"displayName" | "eventMapping", string>>;

function hasControlCharacters(value: string) {
  return /[\u0000-\u001F\u007F]/.test(value);
}

function asciiCaseFold(value: string) {
  return value.replace(/[A-Z]/g, (character) => String.fromCharCode(character.charCodeAt(0) + 32));
}

function hasAbsolutePath(environment: AgentProfileEnvironment, value: string) {
  return environment === "windows"
    ? /^(?:[a-zA-Z]:[\\/]|\\\\)/.test(value)
    : value.startsWith("/");
}

export function validateCustomHookTarget(
  environment: AgentProfileEnvironment,
  target: Extract<AgentConfigTarget, { kind: "customHook" }>,
): CustomHookValidationErrors {
  const errors: CustomHookValidationErrors = {};
  if (hasControlCharacters(target.executable)) errors.executable = "controlCharactersNotAllowed";
  else if (!hasAbsolutePath(environment, target.executable)) errors.executable = "absolutePathRequired";
  else if (target.executable.length > CUSTOM_HOOK_LIMITS.maxPathLength) errors.executable = "pathTooLong";

  if (target.argv.length > CUSTOM_HOOK_LIMITS.maxArguments) errors.argv = "tooManyArguments";
  else if (target.argv.some((argument) => hasControlCharacters(argument))) errors.argv = "controlCharactersNotAllowed";
  else if (target.argv.some((argument) => argument.length > CUSTOM_HOOK_LIMITS.maxArgumentLength)) errors.argv = "argumentTooLong";
  if (target.workingDirectory !== null) {
    if (hasControlCharacters(target.workingDirectory)) errors.workingDirectory = "controlCharactersNotAllowed";
    else if (!hasAbsolutePath(environment, target.workingDirectory)) errors.workingDirectory = "absolutePathRequired";
    else if (target.workingDirectory.length > CUSTOM_HOOK_LIMITS.maxPathLength) errors.workingDirectory = "pathTooLong";
  }
  if (!Number.isInteger(target.timeoutSeconds) || target.timeoutSeconds < CUSTOM_HOOK_LIMITS.minTimeoutSeconds || target.timeoutSeconds > CUSTOM_HOOK_LIMITS.maxTimeoutSeconds) {
    errors.timeoutSeconds = "timeoutOutOfRange";
  }
  return errors;
}

export function validateCustomHookProfile(input: Pick<SaveAgentIntegrationProfileInput, "displayName" | "eventMapping">): CustomHookProfileValidationErrors {
  const errors: CustomHookProfileValidationErrors = {};
  if (input.displayName !== input.displayName.trim()) errors.displayName = "displayNameMustBeTrimmed";
  else if (input.displayName.length === 0) errors.displayName = "displayNameRequired";
  else if (input.displayName.length > CUSTOM_HOOK_LIMITS.maxDisplayNameLength) errors.displayName = "displayNameTooLong";

  if (input.eventMapping.length < CUSTOM_HOOK_LIMITS.minEventMappings) {
    errors.eventMapping = "eventMappingRequired";
    return errors;
  }
  if (input.eventMapping.length > CUSTOM_HOOK_LIMITS.maxEventMappings) {
    errors.eventMapping = "tooManyEventMappings";
    return errors;
  }

  const nativeEvents = new Set<string>();
  for (const mapping of input.eventMapping) {
    const nativeEvent = mapping.nativeEvent;
    if (nativeEvent !== nativeEvent.trim()) {
      errors.eventMapping = "nativeEventMustBeTrimmed";
      return errors;
    }
    if (nativeEvent.length === 0) {
      errors.eventMapping = "nativeEventRequired";
      return errors;
    }
    if (hasControlCharacters(nativeEvent)) {
      errors.eventMapping = "nativeEventControlCharactersNotAllowed";
      return errors;
    }
    if (nativeEvent.length > CUSTOM_HOOK_LIMITS.maxNativeEventLength) {
      errors.eventMapping = "nativeEventTooLong";
      return errors;
    }
    const matchKey = asciiCaseFold(nativeEvent);
    if (nativeEvents.has(matchKey)) {
      errors.eventMapping = "duplicateNativeEvent";
      return errors;
    }
    nativeEvents.add(matchKey);
  }
  return errors;
}
