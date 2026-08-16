import { useMemo, useState } from "react";
import { parseCommandError } from "../api/commandError";
import type { CommandError, DiagnosticEvent, ServiceHealthSnapshot } from "../api/contracts";
import { useI18n } from "../i18n/I18nProvider";
import { translateRegisteredMessage } from "../i18n/catalog";

export interface DiagnosticsSettingsProps {
  health: ServiceHealthSnapshot[];
  events: DiagnosticEvent[];
  storageIntegrity: "unknown" | "checking" | "ok" | "failed";
  onCheckStorageIntegrity(): Promise<void>;
  onRetry(): Promise<void>;
  retryPending?: boolean;
  error?: CommandError | null;
}

export default function DiagnosticsSettings({
  health,
  events,
  storageIntegrity,
  onCheckStorageIntegrity,
  onRetry,
  retryPending = false,
  error,
}: DiagnosticsSettingsProps) {
  const { language, t } = useI18n();
  const [actionError, setActionError] = useState<CommandError | null>(null);
  const services = useMemo(
    () => [...health].sort((left, right) => left.serviceId < right.serviceId ? -1 : left.serviceId > right.serviceId ? 1 : 0),
    [health],
  );

  const runAction = async (action: () => Promise<void>) => {
    setActionError(null);
    try {
      await action();
    } catch (reason) {
      setActionError(parseCommandError(reason));
    }
  };
  const renderedError = actionError ?? error;

  return (
    <div className="settings-body">
      <section className="settings-control" aria-labelledby="diagnostics-storage-title">
        <div className="settings-control__copy">
          <span id="diagnostics-storage-title">{t("diagnostics.storage.title")}</span>
          {storageIntegrity === "ok" && <span role="status">{t("diagnostics.storage.healthy")}</span>}
        </div>
        <button
          className="settings-choice"
          type="button"
          disabled={storageIntegrity === "checking"}
          aria-busy={storageIntegrity === "checking" || undefined}
          onPointerDown={(event) => event.stopPropagation()}
          onClick={() => void runAction(onCheckStorageIntegrity)}
        >
          {t("diagnostics.storage.check")}
        </button>
      </section>

      <section className="settings-control" aria-labelledby="diagnostics-services-title">
        <div className="settings-control__copy">
          <span id="diagnostics-services-title">{t("diagnostics.services.title")}</span>
        </div>
        <div role="list" aria-label={t("diagnostics.services.title")}>
          {services.map((service) => {
            const state = t(`diagnostics.states.${service.state}` as const);
            const label = `${service.serviceId}${language === "zh-CN" ? "：" : ": "}${state}`;
            return (
              <div key={service.serviceId} role="listitem">
                <span role="status" aria-label={label}>{state}</span>{" "}
                <span>{translateRegisteredMessage(language, service.messageKey, service.parameters)}</span>
              </div>
            );
          })}
        </div>
      </section>

      <section className="settings-control" aria-labelledby="diagnostics-events-title">
        <div className="settings-control__copy">
          <span id="diagnostics-events-title">{t("diagnostics.events.title")}</span>
        </div>
        {events.length === 0 ? (
          <span>{t("diagnostics.events.empty")}</span>
        ) : (
          <div role="list" aria-label={t("diagnostics.events.title")}>
            {events.map((event) => <div key={event.id} role="listitem">{event.serviceId}: {event.code}</div>)}
          </div>
        )}
      </section>

      {renderedError && (
        <div className="settings-error" role="alert">
          <span>{translateRegisteredMessage(language, renderedError.messageKey, renderedError.details)}</span>
          {renderedError.retryable && (
            <button
              className="settings-choice"
              type="button"
              disabled={retryPending}
              aria-busy={retryPending || undefined}
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => void runAction(onRetry)}
            >
              {t("diagnostics.actions.retry")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
