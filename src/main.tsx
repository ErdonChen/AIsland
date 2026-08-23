import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import ReminderAlertApp, { isReminderAlertWindow } from "./reminders/ReminderAlertApp";
import { I18nProvider } from "./i18n/I18nProvider";
import { TEXT_COLOR_PREFERENCE_KEY, applyTextColorToDocument, loadTextColorPreference } from "./appearancePreferences";
import "./App.css";

function currentWindowLabel(): string {
  const runtimeWindow = window as typeof window & { __TAURI_INTERNALS__?: unknown };
  if (runtimeWindow.__TAURI_INTERNALS__ === undefined) return "main";
  return getCurrentWindow().label;
}

function synchronizeDocumentTextColor() {
  applyTextColorToDocument(loadTextColorPreference());
}

synchronizeDocumentTextColor();
window.addEventListener("storage", (event) => {
  if (event.key === TEXT_COLOR_PREFERENCE_KEY) synchronizeDocumentTextColor();
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      {isReminderAlertWindow(currentWindowLabel()) ? <ReminderAlertApp consumerId="reminder-alert-window" /> : <App />}
    </I18nProvider>
  </React.StrictMode>,
);
