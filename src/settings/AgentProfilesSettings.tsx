import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Bot, Check, CircleAlert, Pencil, Plus, Save, ScanSearch, Trash2, Wrench } from "lucide-react";
import {
  deleteAgentIntegrationProfile,
  discoverAgentIntegrationCandidates,
  getAgentsSnapshot,
  installAgentIntegration,
  installAgentIntegrationProfile,
  listAgentIntegrationProfiles,
  repairAgentIntegration,
  repairAgentIntegrationProfile,
  saveAgentIntegrationProfile,
  uninstallAgentIntegrationProfile,
} from "../api/commands";
import { parseCommandError } from "../api/commandError";
import type {
  AgentEventMapping,
  AgentId,
  AgentIntegrationDiscoveryCandidate,
  AgentIntegrationDiscoveryResult,
  AgentIntegrationProfile,
  AgentProfileEnvironment,
  AgentProfileInstallationState,
  AgentProfilePresetId,
  AgentStatus,
  CommandError,
  SaveAgentIntegrationProfileInput,
} from "../api/contracts";
import { useI18n } from "../i18n/I18nProvider";
import {
  AGENT_PROFILE_PRESETS,
  CUSTOM_HOOK_LIMITS,
  createCustomHookDraft,
  presetProfileId,
  validateCustomHookProfile,
  validateCustomHookTarget,
} from "./agentProfiles";

const STATUS_OPTIONS: readonly AgentStatus[] = ["idle", "running", "completed", "failed", "waiting", "timeout", "offline"];
type PresetProfile = AgentIntegrationProfile & { configTarget: Extract<AgentIntegrationProfile["configTarget"], { kind: "preset" }> };
type CustomHookDraft = SaveAgentIntegrationProfileInput & { configTarget: Extract<SaveAgentIntegrationProfileInput["configTarget"], { kind: "customHook" }> };

function isPresetProfile(profile: AgentIntegrationProfile): profile is PresetProfile {
  return profile.kind === "preset" && profile.configTarget.kind === "preset";
}

function isCustomHookDraft(draft: SaveAgentIntegrationProfileInput | null): draft is CustomHookDraft {
  return draft !== null && draft.configTarget.kind === "customHook";
}

function builtInAgentId(id: string): AgentId | null {
  return id === "codex" || id === "hermes" || id === "workbuddy" || id === "claude" ? id : null;
}

function profileToSaveInput(profile: AgentIntegrationProfile): SaveAgentIntegrationProfileInput {
  return {
    id: profile.id,
    kind: profile.kind,
    displayName: profile.displayName,
    environment: profile.environment,
    configTarget: profile.configTarget,
    eventMapping: profile.eventMapping,
    enabled: profile.enabled,
    expectedRevision: profile.revision,
  };
}

function stateKey(state: AgentProfileInstallationState) {
  return state === "notInstalled"
    ? "agents.integration.notInstalled"
    : state === "installed"
      ? "agents.integration.installed"
      : state === "needsRepair"
        ? "agents.integration.needsRepair"
        : "agents.integration.unsupported";
}

type DraftPatch = Partial<SaveAgentIntegrationProfileInput>;

interface AgentProfilesSettingsProps {
  focusProfileId?: string | null;
}

export default function AgentProfilesSettings({ focusProfileId = null }: AgentProfilesSettingsProps) {
  const { t } = useI18n();
  const [profiles, setProfiles] = useState<AgentIntegrationProfile[]>([]);
  const [discovery, setDiscovery] = useState<AgentIntegrationDiscoveryResult | null>(null);
  const [discoveryPending, setDiscoveryPending] = useState(false);
  const [presetEnvironment, setPresetEnvironment] = useState<AgentProfileEnvironment>("windows");
  const [draft, setDraft] = useState<SaveAgentIntegrationProfileInput | null>(null);
  const [loading, setLoading] = useState(true);
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [error, setError] = useState<CommandError | null>(null);
  const [validationFailed, setValidationFailed] = useState(false);
  const [requestedProfileId, setRequestedProfileId] = useState<string | null>(null);
  const profileCardRefs = useRef(new Map<string, HTMLElement>());
  const discoveryAttemptRef = useRef(0);
  const discoveryInFlightRef = useRef(false);
  const profileMutationPending = pendingId !== null || discoveryPending;

  const refresh = useCallback(async () => {
    const loaded = await listAgentIntegrationProfiles();
    setProfiles(loaded);
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    void refresh()
      .catch((cause) => {
        if (active) setError(parseCommandError(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [refresh]);

  useEffect(() => () => {
    discoveryAttemptRef.current += 1;
    discoveryInFlightRef.current = false;
  }, []);

  useEffect(() => {
    if (focusProfileId === null) return;
    const profile = profiles.find((candidate) => candidate.id === focusProfileId);
    const environment = profile?.environment ?? /-(windows|wsl)$/.exec(focusProfileId)?.[1] as AgentProfileEnvironment | undefined;
    if (environment !== undefined) setPresetEnvironment((current) => current === environment ? current : environment);
  }, [focusProfileId, profiles]);

  useEffect(() => {
    if (focusProfileId === null) return;
    const card = profileCardRefs.current.get(focusProfileId);
    if (!card) return;
    if (typeof card.scrollIntoView === "function") card.scrollIntoView({ block: "nearest" });
    card.focus({ preventScroll: true });
  }, [focusProfileId, presetEnvironment, profiles]);

  useEffect(() => {
    if (requestedProfileId === null) return;
    const card = profileCardRefs.current.get(requestedProfileId);
    if (!card) return;
    if (typeof card.scrollIntoView === "function") card.scrollIntoView({ block: "nearest" });
    card.focus({ preventScroll: true });
    setRequestedProfileId(null);
  }, [requestedProfileId, presetEnvironment, profiles]);

  const presetProfiles = useMemo(() => new Map(
    profiles
      .filter(isPresetProfile)
      .map((profile) => [presetProfileId(profile.configTarget.adapterId, profile.environment), profile]),
  ), [profiles]);
  const customProfiles = useMemo(() => profiles.filter((profile) => profile.kind === "custom"), [profiles]);

  const putProfile = useCallback((next: AgentIntegrationProfile) => {
    setProfiles((current) => [...current.filter((profile) => profile.id !== next.id), next]);
  }, []);

  const setFailure = (cause: unknown) => {
    const commandError = parseCommandError(cause);
    setError(commandError);
    if (commandError.code === "conflict") void refresh().catch(() => undefined);
  };

  const recoverLifecycleMutationFailure = async (cause: unknown) => {
    setError(parseCommandError(cause));
    await refresh().catch(() => undefined);
  };

  const retryPresetProfiles = async () => {
    if (pendingId !== null || loading || discoveryInFlightRef.current) return;
    setLoading(true);
    setError(null);
    try {
      await refresh();
    } catch (cause) {
      setFailure(cause);
    } finally {
      setLoading(false);
    }
  };

  const connectRunningCandidates = async (result: AgentIntegrationDiscoveryResult) => {
    let nextProfiles = profiles;
    let firstFailure: unknown = null;
    const runningBuiltIns = result.candidates.filter((candidate) => (
      candidate.integrationKind === "builtIn"
      && candidate.environment === "windows"
      && candidate.evidence.includes("runningProcess")
    ));
    if (runningBuiltIns.length > 0) {
      try {
        const snapshot = await getAgentsSnapshot();
        for (const candidate of runningBuiltIns) {
          const agentId = builtInAgentId(candidate.id);
          if (agentId === null) continue;
          const integration = snapshot.agents
            .find((agent) => agent.agentId === agentId)
            ?.integrations.find((item) => item.environment === "windows");
          if (integration === undefined || integration.state === "unsupported") continue;
          try {
            if (integration.state === "installed" || integration.state === "needsRepair") {
              await repairAgentIntegration({ agentId, environment: "windows" });
            } else {
              await installAgentIntegration({ agentId, environment: "windows" });
            }
          } catch (cause) {
            firstFailure ??= cause;
          }
        }
      } catch (cause) {
        firstFailure ??= cause;
      }
    }
    for (const candidate of result.candidates) {
      if (
        candidate.integrationKind !== "preset"
        || candidate.presetId === null
        || candidate.environment !== "windows"
        || candidate.state !== "readyToInstall"
        || !candidate.evidence.includes("runningProcess")
      ) continue;
      const profileId = presetProfileId(candidate.presetId, candidate.environment);
      const existing = nextProfiles.find((profile) => profile.id === profileId);
      if (
        existing === undefined
        || !existing.enabled
        || existing.installationState === "unsupported"
      ) continue;
      setPendingId(existing.id);
      try {
        const connected = existing.installationState === "installed" || existing.installationState === "needsRepair"
          ? await repairAgentIntegrationProfile({ id: existing.id, expectedRevision: existing.revision, confirmRepair: true })
          : await installAgentIntegrationProfile({ id: existing.id, expectedRevision: existing.revision, confirmInstallation: true });
        nextProfiles = [...nextProfiles.filter((profile) => profile.id !== connected.id), connected];
      } catch (cause) {
        firstFailure ??= cause;
      }
    }
    setProfiles(nextProfiles);
    if (firstFailure !== null) throw firstFailure;
  };

  const discoverCandidates = async () => {
    if (discoveryInFlightRef.current || pendingId !== null) return;
    discoveryInFlightRef.current = true;
    const attempt = ++discoveryAttemptRef.current;
    setDiscoveryPending(true);
    setError(null);
    try {
      const result = await discoverAgentIntegrationCandidates();
      if (discoveryAttemptRef.current === attempt) {
        setDiscovery(result);
        await connectRunningCandidates(result);
      }
    } catch (cause) {
      if (discoveryAttemptRef.current === attempt) {
        setError(parseCommandError(cause));
        await refresh().catch(() => undefined);
      }
    } finally {
      if (discoveryAttemptRef.current === attempt) {
        discoveryInFlightRef.current = false;
        setDiscoveryPending(false);
        setPendingId(null);
      }
    }
  };

  const useDiscoveryCandidate = (candidate: AgentIntegrationDiscoveryCandidate) => {
    if (candidate.integrationKind === "preset" && candidate.presetId !== null) {
      setDraft(null);
      setPresetEnvironment(candidate.environment);
      setRequestedProfileId(presetProfileId(candidate.presetId, candidate.environment));
      return;
    }
    if (candidate.integrationKind === "custom" && candidate.environment === "windows") {
      setValidationFailed(false);
      setDraft({ ...createCustomHookDraft("windows"), displayName: candidate.displayName });
    }
  };

  const discoveryStateKey = (candidate: AgentIntegrationDiscoveryCandidate) => {
    if (
      candidate.integrationKind === "preset"
      && candidate.presetId !== null
      && presetProfiles.get(presetProfileId(candidate.presetId, candidate.environment))?.installationState === "installed"
    ) return "agentProfiles.discovery.state.hookConfigured";
    if (candidate.state === "automatic") return "agentProfiles.discovery.state.automatic";
    if (candidate.state === "readyToInstall") return "agentProfiles.discovery.state.readyToInstall";
    if (candidate.state === "detectionPending") return "agentProfiles.discovery.state.detectionPending";
    return "agentProfiles.discovery.state.adapterRequired";
  };

  const discoveryActionKey = (candidate: AgentIntegrationDiscoveryCandidate) => candidate.integrationKind === "preset"
    ? "agentProfiles.discovery.configurePreset"
    : "agentProfiles.discovery.configureCustom";

  const profileReason = (reasonCode: string | null) => {
    if (reasonCode === "profileWslNotSupported") return t("agentProfiles.reason.wslUnsupported");
    if (reasonCode === "traeHooksVersionOrConfigUnavailable") return t("agentProfiles.reason.traeTargetMissing");
    return t("agentProfiles.reason.unavailable");
  };

  const validationMessage = (reason: string) => {
    if (reason === "absolutePathRequired") return t("agentProfiles.validation.absolutePathRequired");
    if (reason === "controlCharactersNotAllowed") return t("agentProfiles.validation.controlCharactersNotAllowed");
    if (reason === "pathTooLong") return t("agentProfiles.validation.pathTooLong");
    if (reason === "tooManyArguments") return t("agentProfiles.validation.tooManyArguments");
    if (reason === "argumentTooLong") return t("agentProfiles.validation.argumentTooLong");
    if (reason === "timeoutOutOfRange") return t("agentProfiles.validation.timeoutOutOfRange");
    if (reason === "displayNameRequired") return t("agentProfiles.validation.displayNameRequired");
    if (reason === "displayNameMustBeTrimmed") return t("agentProfiles.validation.displayNameMustBeTrimmed");
    if (reason === "displayNameTooLong") return t("agentProfiles.validation.displayNameTooLong");
    if (reason === "eventMappingRequired") return t("agentProfiles.validation.eventMappingRequired");
    if (reason === "tooManyEventMappings") return t("agentProfiles.validation.tooManyEventMappings");
    if (reason === "nativeEventRequired") return t("agentProfiles.validation.nativeEventRequired");
    if (reason === "nativeEventMustBeTrimmed") return t("agentProfiles.validation.nativeEventMustBeTrimmed");
    if (reason === "nativeEventControlCharactersNotAllowed") return t("agentProfiles.validation.nativeEventControlCharactersNotAllowed");
    if (reason === "nativeEventTooLong") return t("agentProfiles.validation.nativeEventTooLong");
    return t("agentProfiles.validation.duplicateNativeEvent");
  };

  const runPreset = async (adapterId: AgentProfilePresetId) => {
    if (pendingId || discoveryInFlightRef.current) return;
    const existing = presetProfiles.get(presetProfileId(adapterId, presetEnvironment));
    if (existing === undefined || existing.installationState === "unsupported") return;
    setPendingId(adapterId);
    setError(null);
    try {
      const next = existing.installationState === "installed"
        ? await uninstallAgentIntegrationProfile({ id: existing.id, expectedRevision: existing.revision, confirmOwnedRemoval: true })
        : existing.installationState === "needsRepair"
          ? await repairAgentIntegrationProfile({ id: existing.id, expectedRevision: existing.revision, confirmRepair: true })
          : await installAgentIntegrationProfile({ id: existing.id, expectedRevision: existing.revision, confirmInstallation: true });
      putProfile(next);
    } catch (cause) {
      await recoverLifecycleMutationFailure(cause);
    } finally {
      setPendingId(null);
    }
  };

  const runCustom = async (profile: AgentIntegrationProfile) => {
    if (pendingId || discoveryInFlightRef.current || profile.installationState === "unsupported") return;
    setPendingId(profile.id);
    setError(null);
    try {
      const next = profile.installationState === "installed"
        ? await uninstallAgentIntegrationProfile({ id: profile.id, expectedRevision: profile.revision, confirmOwnedRemoval: true })
        : profile.installationState === "needsRepair"
          ? await repairAgentIntegrationProfile({ id: profile.id, expectedRevision: profile.revision, confirmRepair: true })
          : await installAgentIntegrationProfile({ id: profile.id, expectedRevision: profile.revision, confirmInstallation: true });
      putProfile(next);
    } catch (cause) {
      await recoverLifecycleMutationFailure(cause);
    } finally {
      setPendingId(null);
    }
  };

  const updateDraft = (patch: DraftPatch) => {
    setDraft((current) => current ? { ...current, ...patch } : current);
  };

  const updateMapping = (index: number, patch: Partial<AgentEventMapping>) => {
    setDraft((current) => {
      if (!current) return current;
      const eventMapping = current.eventMapping.map((mapping, candidateIndex) => candidateIndex === index ? { ...mapping, ...patch } : mapping);
      return { ...current, eventMapping };
    });
  };

  const updateArgument = (index: number, value: string) => {
    setDraft((current) => {
      if (!isCustomHookDraft(current)) return current;
      const argv = current.configTarget.argv.map((argument, candidateIndex) => candidateIndex === index ? value : argument);
      return { ...current, configTarget: { ...current.configTarget, argv } };
    });
  };

  const saveDraft = async () => {
    if (!draft || pendingId || discoveryInFlightRef.current || draft.environment === "wsl") return;
    if (draft.configTarget.kind === "customHook" && (
      Object.keys(validateCustomHookTarget(draft.environment, draft.configTarget)).length > 0
      || Object.keys(validateCustomHookProfile(draft)).length > 0
    )) {
      setValidationFailed(true);
      return;
    }
    setValidationFailed(false);
    setPendingId(draft.id ?? "new-custom");
    setError(null);
    try {
      const saved = await saveAgentIntegrationProfile(draft);
      putProfile(saved);
      setDraft(null);
    } catch (cause) {
      setFailure(cause);
    } finally {
      setPendingId(null);
    }
  };

  const deleteCustom = async (profile: AgentIntegrationProfile) => {
    if (
      pendingId
      || discoveryInFlightRef.current
      || profile.installationState === "installed"
      || profile.installationState === "needsRepair"
      || !window.confirm(`${t("agentProfiles.action.delete")} ${profile.displayName}?`)
    ) return;
    setPendingId(profile.id);
    setError(null);
    try {
      await deleteAgentIntegrationProfile({ id: profile.id, expectedRevision: profile.revision, confirmDeletion: true });
      setProfiles((current) => current.filter((candidate) => candidate.id !== profile.id));
      if (draft?.id === profile.id) setDraft(null);
    } catch (cause) {
      setFailure(cause);
    } finally {
      setPendingId(null);
    }
  };

  const actionKey = (state: AgentProfileInstallationState) => state === "notInstalled" || state === "unsupported"
    ? "agents.integration.install"
    : state === "needsRepair"
      ? "agents.integration.repair"
      : "agents.integration.uninstall";
  const customDraft = isCustomHookDraft(draft) ? draft : null;
  const customDraftValidationErrors = customDraft ? validateCustomHookTarget(customDraft.environment, customDraft.configTarget) : {};
  const customDraftProfileValidationErrors = customDraft ? validateCustomHookProfile(customDraft) : {};

  return (
    <div className="agent-profiles" aria-busy={loading || discoveryPending || undefined}>
      <div className="agent-profiles__intro">
        <div className="settings-control__copy">
          <span>{t("settings.category.agents")}</span>
          <span>{t("agentProfiles.intro")}</span>
        </div>
        <div className="agent-profiles__intro-actions">
          <button type="button" className="settings-choice" disabled={profileMutationPending} onClick={() => void discoverCandidates()}>
            <ScanSearch size={14} aria-hidden="true" />
            {t("agentProfiles.discovery.scan")}
          </button>
          <div className="settings-choice-group" role="group" aria-label={t("agentProfiles.field.environment")}>
            {(["windows", "wsl"] as const).map((environment) => (
              <button
                key={environment}
                type="button"
                className={`settings-choice${presetEnvironment === environment ? " settings-choice--active" : ""}`}
                aria-pressed={presetEnvironment === environment}
                disabled={profileMutationPending}
                onClick={() => setPresetEnvironment(environment)}
              >
                {t(`agents.environments.${environment}`)}
              </button>
            ))}
          </div>
        </div>
      </div>

      {discovery && (
        <section className="agent-profile-discovery" aria-label={t("agentProfiles.discovery.title")}>
          <header>
            <strong>{t("agentProfiles.discovery.title")}</strong>
            <span>{t("agentProfiles.discovery.readOnly")}</span>
          </header>
          {discovery.candidates.length === 0 ? (
            <p>{t("agentProfiles.discovery.empty")}</p>
          ) : (
            <ul>
              {discovery.candidates.map((candidate) => (
                <li key={candidate.id}>
                  <div>
                    <strong>{candidate.displayName}</strong>
                    <span className="setting-value-pill">{t(discoveryStateKey(candidate))}</span>
                    <small>{candidate.evidence.map((evidence) => t(`agentProfiles.discovery.evidence.${evidence}`)).join(" · ")}</small>
                  </div>
                  {candidate.integrationKind !== "builtIn" && (
                    <button
                      type="button"
                      className="settings-choice"
                      aria-label={`${t(discoveryActionKey(candidate))} ${candidate.displayName}`}
                      disabled={profileMutationPending}
                      onClick={() => useDiscoveryCandidate(candidate)}
                    >
                      {t(discoveryActionKey(candidate))}
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <div className="agent-profiles__grid" role="list">
        {AGENT_PROFILE_PRESETS.map((preset) => {
          const profile = presetProfiles.get(presetProfileId(preset.id, presetEnvironment));
          const missing = !loading && profile === undefined;
          const state = profile?.installationState ?? "notInstalled";
          const detectionPending = profile?.reasonCode === "traeHooksVersionOrConfigUnavailable";
          const retryable = missing || detectionPending;
          const action = actionKey(state);
          return (
            <article
              key={preset.id}
              ref={(node) => {
                const profileId = profile?.id ?? presetProfileId(preset.id, presetEnvironment);
                if (node) profileCardRefs.current.set(profileId, node);
                else profileCardRefs.current.delete(profileId);
              }}
              className="agent-profile-card"
              data-profile-id={profile?.id ?? presetProfileId(preset.id, presetEnvironment)}
              role="listitem"
              tabIndex={-1}
            >
              <div className="agent-profile-card__heading">
                <span className="agent-profile-card__mark" aria-hidden="true"><Bot size={16} strokeWidth={1.6} /></span>
                <span className="setting-value-pill">{detectionPending ? t("agentProfiles.state.detectionPending") : t(stateKey(state))}</span>
              </div>
              <strong>{preset.displayName}</strong>
              <span>{t(preset.descriptionKey)}</span>
              {(missing || profile?.reasonCode) && <small>{missing ? t("agentProfiles.reason.unavailable") : profileReason(profile?.reasonCode ?? null)}</small>}
              <button
                type="button"
                className="settings-choice agent-profile-card__action"
                aria-label={`${t(action)} ${preset.displayName}`}
                disabled={loading || profileMutationPending || missing || state === "unsupported"}
                onClick={() => void runPreset(preset.id)}
              >
                {state === "needsRepair" ? <Wrench size={14} aria-hidden="true" /> : <Check size={14} aria-hidden="true" />}
                {t(action)}
              </button>
              {retryable && (
                <button
                  type="button"
                  className="settings-choice agent-profile-card__action"
                  aria-label={`${t("action.retry")} ${preset.displayName}`}
                  disabled={loading || profileMutationPending}
                  onClick={() => void retryPresetProfiles()}
                >
                  {t("action.retry")}
                </button>
              )}
            </article>
          );
        })}

        <button
          type="button"
          className="agent-profile-card agent-profile-card--add"
          aria-label={t("agentProfiles.addCustom")}
          disabled={profileMutationPending || presetEnvironment === "wsl"}
          onClick={() => {
            setValidationFailed(false);
            setDraft(createCustomHookDraft(presetEnvironment));
          }}
        >
          <span className="agent-profile-card__mark" aria-hidden="true"><Plus size={18} strokeWidth={1.6} /></span>
          <strong>Custom Hook</strong>
          <span>{t("agentProfiles.customHint")}</span>
        </button>

        {customProfiles.map((profile) => {
          const action = actionKey(profile.installationState);
          return (
            <article
              key={profile.id}
              ref={(node) => {
                if (node) profileCardRefs.current.set(profile.id, node);
                else profileCardRefs.current.delete(profile.id);
              }}
              className="agent-profile-card"
              data-profile-id={profile.id}
              role="listitem"
              tabIndex={-1}
            >
              <div className="agent-profile-card__heading">
                <span className="agent-profile-card__mark" aria-hidden="true"><Bot size={16} strokeWidth={1.6} /></span>
                <span className="setting-value-pill">{t(stateKey(profile.installationState))}</span>
              </div>
              <strong>{profile.displayName}</strong>
              <span>{t("agentProfiles.customHint")}</span>
              <div className="agent-profile-card__actions">
                <button type="button" className="settings-choice" aria-label={`${t("agentProfiles.action.edit")} ${profile.displayName}`} disabled={profileMutationPending || profile.environment === "wsl" || profile.installationState === "installed" || profile.installationState === "needsRepair"} onClick={() => setDraft(profileToSaveInput(profile))}><Pencil size={14} aria-hidden="true" />{t("agentProfiles.action.edit")}</button>
                <button type="button" className="settings-choice" aria-label={`${t(action)} ${profile.displayName}`} disabled={profileMutationPending || profile.installationState === "unsupported"} onClick={() => void runCustom(profile)}>{t(action)}</button>
                <button type="button" className="settings-choice" aria-label={`${t("agentProfiles.action.delete")} ${profile.displayName}`} disabled={profileMutationPending || profile.installationState === "installed" || profile.installationState === "needsRepair"} onClick={() => void deleteCustom(profile)}><Trash2 size={14} aria-hidden="true" /></button>
              </div>
            </article>
          );
        })}
      </div>

      {customDraft && (
        <section className="agent-profile-editor" aria-label={customDraft.id === null ? t("agentProfiles.editor.new") : t("agentProfiles.editor.edit")}>
          <header>
            <div>
              <strong>{customDraft.id === null ? t("agentProfiles.editor.new") : t("agentProfiles.editor.edit")}</strong>
              <span>{t("agentProfiles.customHint")}</span>
            </div>
          </header>
          <label>
            <span>{t("agentProfiles.field.name")}</span>
            <input disabled={profileMutationPending} value={customDraft.displayName} onChange={(event) => updateDraft({ displayName: event.currentTarget.value })} />
          </label>
          <label>
            <span>{t("agentProfiles.field.environment")}</span>
            <select disabled={profileMutationPending} value={customDraft.environment} onChange={(event) => updateDraft({ environment: event.currentTarget.value as AgentProfileEnvironment })}>
              <option value="windows">{t("agents.environments.windows")}</option>
              <option value="wsl">{t("agents.environments.wsl")}</option>
            </select>
          </label>
          <label>
            <span>{t(customDraft.environment === "windows" ? "agentProfiles.field.executable" : "agentProfiles.field.executableWslUnsupported")}</span>
            <input disabled={profileMutationPending} maxLength={CUSTOM_HOOK_LIMITS.maxPathLength} value={customDraft.configTarget.executable} onChange={(event) => updateDraft({ configTarget: { ...customDraft.configTarget, executable: event.currentTarget.value } })} />
          </label>
          <div className="agent-profile-editor__argv">
            <span>{t("agentProfiles.field.argv")}</span>
            {customDraft.configTarget.argv.map((argument, index) => (
              <div key={index} className="agent-profile-editor__argument">
                <input aria-label={`${t("agentProfiles.field.argv")} ${index + 1}`} disabled={profileMutationPending} maxLength={CUSTOM_HOOK_LIMITS.maxArgumentLength} value={argument} onChange={(event) => updateArgument(index, event.currentTarget.value)} />
                <button type="button" className="settings-choice" aria-label={t("agentProfiles.action.delete")} disabled={profileMutationPending} onClick={() => updateDraft({ configTarget: { ...customDraft.configTarget, argv: customDraft.configTarget.argv.filter((_, candidateIndex) => candidateIndex !== index) } })}><Trash2 size={13} aria-hidden="true" /></button>
              </div>
            ))}
            <small>{customDraft.configTarget.argv.length}/{CUSTOM_HOOK_LIMITS.maxArguments}</small>
            <button type="button" className="settings-choice" disabled={profileMutationPending || customDraft.configTarget.argv.length >= CUSTOM_HOOK_LIMITS.maxArguments} onClick={() => updateDraft({ configTarget: { ...customDraft.configTarget, argv: [...customDraft.configTarget.argv, ""] } })}><Plus size={14} aria-hidden="true" />{t("agentProfiles.action.addArgument")}</button>
          </div>
          <label>
            <span>{t("agentProfiles.field.workingDirectory")}</span>
            <input disabled={profileMutationPending} maxLength={CUSTOM_HOOK_LIMITS.maxPathLength} value={customDraft.configTarget.workingDirectory ?? ""} onChange={(event) => updateDraft({ configTarget: { ...customDraft.configTarget, workingDirectory: event.currentTarget.value || null } })} />
          </label>
          <label>
            <span>{t("agentProfiles.field.timeout")}</span>
            <input type="number" disabled={profileMutationPending} min={CUSTOM_HOOK_LIMITS.minTimeoutSeconds} max={CUSTOM_HOOK_LIMITS.maxTimeoutSeconds} value={customDraft.configTarget.timeoutSeconds} onChange={(event) => updateDraft({ configTarget: { ...customDraft.configTarget, timeoutSeconds: Number(event.currentTarget.value) } })} />
          </label>
          <div className="agent-profile-editor__mappings">
            <span>{t("agentProfiles.field.eventMapping")}</span>
            {customDraft.eventMapping.map((mapping, index) => (
              <div key={`${mapping.nativeEvent}-${index}`} className="agent-profile-editor__mapping">
                <input aria-label={t("agentProfiles.field.nativeEvent")} disabled={profileMutationPending} value={mapping.nativeEvent} onChange={(event) => updateMapping(index, { nativeEvent: event.currentTarget.value })} />
                <select aria-label={t("agentProfiles.field.normalizedStatus")} disabled={profileMutationPending} value={mapping.normalizedStatus} onChange={(event) => updateMapping(index, { normalizedStatus: event.currentTarget.value as AgentStatus })}>
                  {STATUS_OPTIONS.map((status) => <option key={status} value={status}>{t(`agents.status.${status}`)}</option>)}
                </select>
                <button type="button" className="settings-choice" aria-label={t("agentProfiles.action.delete")} disabled={profileMutationPending} onClick={() => updateDraft({ eventMapping: customDraft.eventMapping.filter((_, candidateIndex) => candidateIndex !== index) })}><Trash2 size={13} aria-hidden="true" /></button>
              </div>
            ))}
            <button type="button" className="settings-choice" disabled={profileMutationPending} onClick={() => updateDraft({ eventMapping: [...customDraft.eventMapping, { nativeEvent: "", normalizedStatus: "idle" }] })}><Plus size={14} aria-hidden="true" />{t("agentProfiles.action.addMapping")}</button>
          </div>
          <footer>
            <button type="button" className="settings-choice" onClick={() => setDraft(null)}>{t("action.cancel")}</button>
            <button type="button" className="settings-choice settings-choice--active" disabled={profileMutationPending || customDraft.environment === "wsl"} onClick={() => void saveDraft()}><Save size={14} aria-hidden="true" />{t("action.save")}</button>
          </footer>
          {validationFailed && <div className="settings-error" role="alert"><CircleAlert size={14} aria-hidden="true" /><span>{t("agentProfiles.error.invalidHook")}</span><ul>{[...Object.values(customDraftValidationErrors), ...Object.values(customDraftProfileValidationErrors)].map((reason) => <li key={reason}>{validationMessage(reason)}</li>)}</ul></div>}
        </section>
      )}
      {error && <p className="settings-error" role="alert">{t("agentProfiles.error.actionFailed")} {error.code}</p>}
    </div>
  );
}
