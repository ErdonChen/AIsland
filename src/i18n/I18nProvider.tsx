import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  DEFAULT_LANGUAGE,
  parseUiLanguage,
  translate,
  type TranslationKey,
  type UiLanguage,
} from "./catalog";

const LANGUAGE_STORAGE_KEY = "aiceland.ui.language";

export type I18nContextValue = {
  language: UiLanguage;
  t: (key: TranslationKey) => string;
  setLanguage: (language: UiLanguage) => Promise<boolean>;
  languagePending: boolean;
  languageError: string | null;
};

const I18nContext = createContext<I18nContextValue | null>(null);

type LanguageOperation = {
  lifecycle: number;
};

type InitializationOperation = {
  candidate: UiLanguage;
  promise: Promise<unknown>;
};

function restoreStoredLanguage(previous: string | null) {
  if (previous === null) {
    localStorage.removeItem(LANGUAGE_STORAGE_KEY);
  } else {
    localStorage.setItem(LANGUAGE_STORAGE_KEY, previous);
  }
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [language, setLanguageState] = useState<UiLanguage>(DEFAULT_LANGUAGE);
  const [languagePending, setLanguagePending] = useState(false);
  const [languageError, setLanguageError] = useState<string | null>(null);
  const languageRef = useRef<UiLanguage>(DEFAULT_LANGUAGE);
  const lifecycleRef = useRef(0);
  const operationRef = useRef<LanguageOperation | null>(null);
  const initializationRef = useRef<InitializationOperation | null>(null);

  const commitLanguage = useCallback((nextLanguage: UiLanguage) => {
    languageRef.current = nextLanguage;
    setLanguageState(nextLanguage);
  }, []);

  const setLanguage = useCallback(async (candidate: UiLanguage): Promise<boolean> => {
    if (operationRef.current !== null || initializationRef.current !== null) return false;

    const confirmed = languageRef.current;
    if (candidate === confirmed) {
      setLanguageError(null);
      return true;
    }

    let previousStorageValue: string | null;
    try {
      previousStorageValue = localStorage.getItem(LANGUAGE_STORAGE_KEY);
      localStorage.setItem(LANGUAGE_STORAGE_KEY, candidate);
    } catch {
      setLanguageError(translate(confirmed, "error.languageStorage"));
      return false;
    }

    const operation = { lifecycle: lifecycleRef.current };
    operationRef.current = operation;
    setLanguageError(null);
    setLanguagePending(true);
    try {
      await invoke("set_ui_language", { language: candidate });
      if (lifecycleRef.current !== operation.lifecycle) return false;

      commitLanguage(candidate);
      return true;
    } catch {
      if (lifecycleRef.current !== operation.lifecycle) return false;
      try {
        restoreStoredLanguage(previousStorageValue);
      } catch {
        setLanguageError(translate(confirmed, "error.languageStorage"));
        return false;
      }
      if (lifecycleRef.current === operation.lifecycle) {
        setLanguageError(translate(confirmed, "error.languageNative"));
      }
      return false;
    } finally {
      if (operationRef.current === operation) {
        operationRef.current = null;
        if (lifecycleRef.current === operation.lifecycle) setLanguagePending(false);
      }
    }
  }, [commitLanguage]);

  useEffect(() => {
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    let active = true;

    const initialize = async () => {
      let storedLanguage: string | null;
      try {
        storedLanguage = localStorage.getItem(LANGUAGE_STORAGE_KEY);
      } catch {
        if (active) setLanguageError(translate(DEFAULT_LANGUAGE, "error.languageStorage"));
        return;
      }

      const candidate = parseUiLanguage(storedLanguage);
      if (candidate === DEFAULT_LANGUAGE) return;

      let operation = initializationRef.current;
      if (operation === null) {
        operation = {
          candidate,
          promise: invoke("set_ui_language", { language: candidate }),
        };
        initializationRef.current = operation;
      }
      if (operation.candidate !== candidate) return;

      setLanguagePending(true);
      setLanguageError(null);
      try {
        await operation.promise;
        if (!active || lifecycleRef.current !== lifecycle) return;

        commitLanguage(candidate);
      } catch {
        if (!active || lifecycleRef.current !== lifecycle) return;
        try {
          localStorage.setItem(LANGUAGE_STORAGE_KEY, DEFAULT_LANGUAGE);
          setLanguageError(translate(DEFAULT_LANGUAGE, "error.languageNative"));
        } catch {
          setLanguageError(translate(DEFAULT_LANGUAGE, "error.languageStorage"));
        }
      } finally {
        if (initializationRef.current === operation) {
          initializationRef.current = null;
        }
        if (active && lifecycleRef.current === lifecycle) setLanguagePending(false);
      }
    };

    void initialize();
    return () => {
      active = false;
      if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1;
    };
  }, [commitLanguage]);

  const value = useMemo<I18nContextValue>(() => ({
    language,
    t: (key) => translate(language, key),
    setLanguage,
    languagePending,
    languageError,
  }), [language, languageError, languagePending, setLanguage]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const value = useContext(I18nContext);
  if (value === null) throw new Error("useI18n must be used within I18nProvider");
  return value;
}
