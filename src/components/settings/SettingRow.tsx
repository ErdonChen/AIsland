import { ChevronRight, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

type SettingRowProps = {
  label: string;
  summary?: ReactNode;
  icon?: LucideIcon;
  onActivate?: () => void;
  readOnly?: boolean;
};

export default function SettingRow({
  label,
  summary,
  icon: Icon,
  onActivate,
  readOnly = false,
}: SettingRowProps) {
  const content = (
    <>
      <span className="setting-row__identity">
        {Icon && <Icon size={16} strokeWidth={1.5} aria-hidden="true" />}
        <span className="setting-row__label" title={label}>{label}</span>
      </span>
      {summary && <span className="setting-row__summary">{summary}</span>}
      {onActivate && <ChevronRight size={16} strokeWidth={1.5} aria-hidden="true" />}
    </>
  );

  if (onActivate) {
    return (
      <button
        className="setting-row"
        type="button"
        aria-label={label}
        title={label}
        onPointerDown={(event) => event.stopPropagation()}
        onClick={onActivate}
      >
        {content}
      </button>
    );
  }

  return (
    <div className="setting-row setting-row--readonly" aria-disabled={readOnly || undefined}>
      {content}
    </div>
  );
}
