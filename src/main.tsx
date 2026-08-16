import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import ReminderAlertApp, { isReminderAlertWindow } from "./reminders/ReminderAlertApp";
import { I18nProvider } from "./i18n/I18nProvider";
import "./App.css";

function currentWindowLabel(): string {
  const runtimeWindow = window as typeof window & { __TAURI_INTERNALS__?: unknown };
  if (runtimeWindow.__TAURI_INTERNALS__ === undefined) return "main";
  return getCurrentWindow().label;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      {isReminderAlertWindow(currentWindowLabel()) ? <ReminderAlertApp consumerId="reminder-alert-window" /> : <App />}
    </I18nProvider>
  </React.StrictMode>,
);
