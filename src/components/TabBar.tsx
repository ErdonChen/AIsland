import {
  Activity,
  Bell,
  ClipboardList,
  Home,
  NotebookPen,
  Settings,
} from "lucide-react";
import { useI18n } from "../i18n/I18nProvider";
import type { TranslationKey } from "../i18n/catalog";
import type { IslandPage } from "../types";

const TABS: { page: IslandPage; icon: typeof Home; labelKey: TranslationKey }[] = [
  { page: "home", icon: Home, labelKey: "tab.home" },
  { page: "note", icon: NotebookPen, labelKey: "tab.notes" },
  { page: "clipboard", icon: ClipboardList, labelKey: "tab.clipboard" },
  { page: "monitor", icon: Activity, labelKey: "tab.monitor" },
  { page: "notify", icon: Bell, labelKey: "tab.notifications" },
  { page: "settings", icon: Settings, labelKey: "tab.settings" },
];

type TabBarProps = {
  page: IslandPage;
  onSelect: (page: IslandPage) => void;
};

export default function TabBar({ page, onSelect }: TabBarProps) {
  const { t } = useI18n();

  return (
    <div className="tab-bar" role="tablist" aria-label={t("aria.tabList")}>
      {TABS.map(({ page: candidate, icon: Icon, labelKey }) => (
        <button
          key={candidate}
          className={`tab-btn${page === candidate ? " tab-btn--active" : ""}`}
          title={t(labelKey)}
          aria-label={t(labelKey)}
          aria-selected={page === candidate}
          role="tab"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={() => onSelect(candidate)}
        >
          <Icon size={16} strokeWidth={1.5} />
        </button>
      ))}
    </div>
  );
}
