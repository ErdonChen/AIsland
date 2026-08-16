import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useState } from "react";
import { afterEach, expect, test, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { I18nProvider, useI18n } from "./I18nProvider";

const LANGUAGE_KEY = "aiceland.ui.language";

function LanguageProbe() {
  const { language, languageError, languagePending, setLanguage, t } = useI18n();
  const [result, setResult] = useState<string | null>(null);
  const agentName = "WorkBuddy-Pro";

  const chooseLanguage = async (candidate: "zh-CN" | "en-US") => {
    setResult(String(await setLanguage(candidate)));
  };

  return (
    <section data-testid="probe">
      <output data-testid="language">{language}</output>
      <output data-testid="home-label">{t("tab.home")}</output>
      <output data-testid="agent-name">{agentName}</output>
      <output data-testid="pending">{String(languagePending)}</output>
      <output data-testid="error">{languageError ?? ""}</output>
      <output data-testid="result">{result ?? ""}</output>
      <button type="button" onClick={() => void chooseLanguage("en-US")}>
        English
      </button>
      <button type="button" onClick={() => void chooseLanguage("zh-CN")}>
        Chinese
      </button>
    </section>
  );
}

function renderProvider(strict = false) {
  const content = (
    <I18nProvider>
      <LanguageProbe />
    </I18nProvider>
  );
  return render(strict ? <StrictMode>{content}</StrictMode> : content);
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

afterEach(() => {
  cleanup();
  localStorage.clear();
  invokeMock.mockReset();
  vi.restoreAllMocks();
});

test("uses Chinese by default and leaves a dynamic agent name untouched", () => {
  renderProvider();

  expect(screen.getByTestId("language")).toHaveTextContent("zh-CN");
  expect(screen.getByTestId("home-label")).toHaveTextContent("主页");
  expect(screen.getByTestId("agent-name")).toHaveTextContent("WorkBuddy-Pro");
  expect(invokeMock).not.toHaveBeenCalled();
});

test("restores a saved English language only after native synchronization", async () => {
  localStorage.setItem(LANGUAGE_KEY, "en-US");
  invokeMock.mockResolvedValue(undefined);

  renderProvider();

  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith("set_ui_language", { language: "en-US" });
  });
  expect(screen.getByTestId("language")).toHaveTextContent("en-US");
  expect(screen.getByTestId("home-label")).toHaveTextContent("Home");
});

test("waits for native success before committing a selected language", async () => {
  const nativeChange = deferred<void>();
  invokeMock.mockReturnValue(nativeChange.promise);
  const user = userEvent.setup();

  renderProvider();
  const probe = screen.getByTestId("probe");
  await user.click(screen.getByRole("button", { name: "English" }));

  expect(localStorage.getItem(LANGUAGE_KEY)).toBe("en-US");
  expect(screen.getByTestId("language")).toHaveTextContent("zh-CN");
  expect(screen.getByTestId("pending")).toHaveTextContent("true");

  await act(async () => {
    nativeChange.resolve();
    await nativeChange.promise;
  });

  expect(screen.getByTestId("language")).toHaveTextContent("en-US");
  expect(screen.getByTestId("home-label")).toHaveTextContent("Home");
  expect(screen.getByTestId("probe")).toBe(probe);
  expect(screen.getByTestId("result")).toHaveTextContent("true");
});

test("rejects a second selection while the first native transaction is pending", async () => {
  const nativeChange = deferred<void>();
  invokeMock.mockReturnValue(nativeChange.promise);
  const user = userEvent.setup();

  renderProvider();
  await user.click(screen.getByRole("button", { name: "English" }));
  await user.click(screen.getByRole("button", { name: "Chinese" }));

  expect(invokeMock).toHaveBeenCalledTimes(1);
  expect(localStorage.getItem(LANGUAGE_KEY)).toBe("en-US");
  expect(screen.getByTestId("result")).toHaveTextContent("false");

  await act(async () => {
    nativeChange.resolve();
    await nativeChange.promise;
  });
  expect(screen.getByTestId("language")).toHaveTextContent("en-US");
});

test("does not invoke native code when persisting the candidate language fails", async () => {
  const user = userEvent.setup();
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("storage unavailable");
  });

  renderProvider();
  await user.click(screen.getByRole("button", { name: "English" }));

  await waitFor(() => {
    expect(screen.getByTestId("result")).toHaveTextContent("false");
  });
  expect(invokeMock).not.toHaveBeenCalled();
  expect(screen.getByTestId("language")).toHaveTextContent("zh-CN");
  expect(screen.getByTestId("error")).toHaveTextContent("无法保存界面语言设置。");
});

test("restores the previous storage value when native synchronization fails", async () => {
  localStorage.setItem(LANGUAGE_KEY, "zh-CN");
  invokeMock.mockRejectedValue(new Error("native menu rejected"));
  const user = userEvent.setup();

  renderProvider();
  await user.click(screen.getByRole("button", { name: "English" }));

  await waitFor(() => {
    expect(screen.getByTestId("result")).toHaveTextContent("false");
  });
  expect(localStorage.getItem(LANGUAGE_KEY)).toBe("zh-CN");
  expect(screen.getByTestId("language")).toHaveTextContent("zh-CN");
  expect(screen.getByTestId("error")).toHaveTextContent("无法同步原生界面语言。");
});

test("reuses the first StrictMode initialization when a second native request would reject", async () => {
  localStorage.setItem(LANGUAGE_KEY, "en-US");
  const firstInitialization = deferred<void>();
  invokeMock
    .mockReturnValueOnce(firstInitialization.promise)
    .mockRejectedValueOnce(new Error("second request rejected"));

  renderProvider(true);

  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalled();
  });
  expect(invokeMock).toHaveBeenCalledTimes(1);

  await act(async () => {
    firstInitialization.resolve();
    await firstInitialization.promise;
  });
  expect(screen.getByTestId("language")).toHaveTextContent("en-US");
  expect(localStorage.getItem(LANGUAGE_KEY)).toBe("en-US");
  expect(screen.getByTestId("pending")).toHaveTextContent("false");
});

test("keeps the saved language rolled back when the shared StrictMode initialization rejects", async () => {
  localStorage.setItem(LANGUAGE_KEY, "en-US");
  const firstInitialization = deferred<void>();
  invokeMock
    .mockReturnValueOnce(firstInitialization.promise)
    .mockResolvedValueOnce(undefined);

  renderProvider(true);

  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalled();
  });
  expect(invokeMock).toHaveBeenCalledTimes(1);

  await act(async () => {
    firstInitialization.reject(new Error("first request rejected"));
    await firstInitialization.promise.catch(() => undefined);
  });

  await waitFor(() => {
    expect(screen.getByTestId("error")).toHaveTextContent("无法同步原生界面语言。");
  });
  expect(screen.getByTestId("language")).toHaveTextContent("zh-CN");
  expect(localStorage.getItem(LANGUAGE_KEY)).toBe("zh-CN");
  expect(screen.getByTestId("pending")).toHaveTextContent("false");
});
