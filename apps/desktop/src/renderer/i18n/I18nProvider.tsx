import {
  PropsWithChildren,
  createContext,
  useContext,
  useLayoutEffect,
  useMemo,
  useState
} from "react";

import type {
  DesktopLocaleDefinition,
  DesktopLocalizationBundle,
  TranslationValues
} from "./types";

const STORAGE_KEY = "agentweave.localization.locale.v1";
const localizationBundle = bundledLocalization();
const fallbackMessages = localizationBundle.locales.find(
  (entry) => entry.id === localizationBundle.defaultLocale
)?.messages ?? localizationBundle.locales[0].messages;

type I18nContextValue = {
  locale: string;
  locales: DesktopLocaleDefinition[];
  selectLocale: (locale: string) => void;
  t: (key: string, values?: TranslationValues) => string;
};

const I18nContext = createContext<I18nContextValue>({
  locale: localizationBundle.defaultLocale,
  locales: localizationBundle.locales,
  selectLocale: () => undefined,
  t: (key, values = {}) => interpolate(fallbackMessages[key] ?? key, values)
});

export function I18nProvider({ children }: PropsWithChildren): JSX.Element {
  const [locale, setLocale] = useState(loadInitialLocale);
  const messages = useMemo(
    () => localizationBundle.locales.find((entry) => entry.id === locale)?.messages
      ?? localizationBundle.locales[0].messages,
    [locale]
  );

  useLayoutEffect(() => {
    document.documentElement.lang = locale;
    document.documentElement.dir = "ltr";
  }, [locale]);

  const selectLocale = (nextLocale: string) => {
    if (!localizationBundle.locales.some((entry) => entry.id === nextLocale)) return;
    setLocale(nextLocale);
    try {
      window.localStorage.setItem(STORAGE_KEY, nextLocale);
    } catch {
      // The selected locale remains active for this window when storage is unavailable.
    }
  };
  const t = (key: string, values: TranslationValues = {}) => interpolate(messages[key] ?? key, values);

  return (
    <I18nContext.Provider value={{ locale, locales: localizationBundle.locales, selectLocale, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}

export function getBundledLocalization(): DesktopLocalizationBundle {
  return localizationBundle;
}

function loadInitialLocale(): string {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (saved && hasLocale(saved)) return canonicalLocale(saved);
  } catch {
    // Continue with system and package defaults.
  }
  return hasLocale(localizationBundle.defaultLocale)
    ? canonicalLocale(localizationBundle.defaultLocale)
    : localizationBundle.locales[0].id;
}

function hasLocale(locale: string): boolean {
  return localizationBundle.locales.some(
    (entry) => entry.id.toLowerCase() === locale.toLowerCase()
  );
}

function canonicalLocale(locale: string): string {
  return localizationBundle.locales.find(
    (entry) => entry.id.toLowerCase() === locale.toLowerCase()
  )?.id ?? localizationBundle.defaultLocale;
}

function interpolate(message: string, values: TranslationValues): string {
  return message.replace(/\{([A-Za-z][A-Za-z0-9_]*)\}/g, (placeholder, key: string) => (
    Object.hasOwn(values, key) ? String(values[key]) : placeholder
  ));
}

function bundledLocalization(): DesktopLocalizationBundle {
  if (typeof __AGENTWEAVE_LOCALIZATION__ !== "undefined") {
    const configured = __AGENTWEAVE_LOCALIZATION__;
    if (configured.locales.length > 0) return configured;
  }
  return {
    defaultLocale: "en",
    locales: [{
      id: "en",
      label: "English",
      messages: {
        "app.name": "AgentWeave",
        "app.tagline": "Ask naturally. The agent will handle the work."
      }
    }]
  };
}
