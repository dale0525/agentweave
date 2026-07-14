import { Theme } from "@radix-ui/themes";
import {
  PropsWithChildren,
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState
} from "react";

import { installPackagedFonts } from "./fontFaces";
import { isHighContrastTheme, isLightTheme, themeCssVariables } from "./themePalette";
import type { DesktopAppearanceBundle, DesktopThemeDefinition } from "./types";

const STORAGE_KEY = "generalagent.appearance.theme.v1";
const appearanceBundle = bundledAppearance();
const fallbackTheme = appearanceBundle.themes[0];

type AppearanceContextValue = {
  activeTheme: DesktopThemeDefinition;
  selectTheme: (themeId: string) => void;
  themes: DesktopThemeDefinition[];
};

const AppearanceContext = createContext<AppearanceContextValue>({
  activeTheme: fallbackTheme,
  selectTheme: () => undefined,
  themes: appearanceBundle.themes
});

export function AppearanceProvider({ children }: PropsWithChildren): JSX.Element {
  const [themeId, setThemeId] = useState(loadInitialThemeId);
  const activeTheme = useMemo(
    () => appearanceBundle.themes.find((theme) => theme.id === themeId) ?? fallbackTheme,
    [themeId]
  );

  useEffect(() => installPackagedFonts(appearanceBundle.fontFaces), []);
  useLayoutEffect(() => applyTheme(activeTheme), [activeTheme]);

  const selectTheme = (nextThemeId: string) => {
    if (!appearanceBundle.themes.some((theme) => theme.id === nextThemeId)) return;
    setThemeId(nextThemeId);
    try {
      window.localStorage.setItem(STORAGE_KEY, nextThemeId);
    } catch {
      // Theme selection still applies for the current window when storage is unavailable.
    }
  };

  return (
    <AppearanceContext.Provider
      value={{ activeTheme, selectTheme, themes: appearanceBundle.themes }}
    >
      <Theme
        accentColor="blue"
        appearance={isLightTheme(activeTheme) ? "light" : "dark"}
        grayColor="gray"
        hasBackground={false}
        radius="small"
        scaling="100%"
      >
        {children}
      </Theme>
    </AppearanceContext.Provider>
  );
}

export function useAppearance(): AppearanceContextValue {
  return useContext(AppearanceContext);
}

export function getBundledAppearance(): DesktopAppearanceBundle {
  return appearanceBundle;
}

function loadInitialThemeId(): string {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (saved && appearanceBundle.themes.some((theme) => theme.id === saved)) return saved;
  } catch {
    // The package default remains available when storage is unavailable.
  }
  return appearanceBundle.defaultTheme;
}

function applyTheme(theme: DesktopThemeDefinition): void {
  const root = document.documentElement;
  for (const [property, value] of Object.entries(themeCssVariables(theme))) {
    root.style.setProperty(property, value);
  }
  root.dataset.appearance = isLightTheme(theme) ? "light" : "dark";
  root.dataset.highContrast = String(isHighContrastTheme(theme));
  root.dataset.themeId = theme.id;
  root.style.colorScheme = isLightTheme(theme) ? "light" : "dark";
}

function bundledAppearance(): DesktopAppearanceBundle {
  if (typeof __GENERAL_AGENT_APPEARANCE__ !== "undefined") {
    const configured = __GENERAL_AGENT_APPEARANCE__;
    if (configured.themes.length > 0) {
      const defaultAvailable = configured.themes.some(
        (theme) => theme.id === configured.defaultTheme
      );
      return {
        ...configured,
        defaultTheme: defaultAvailable ? configured.defaultTheme : configured.themes[0].id
      };
    }
  }
  return {
    defaultTheme: "vscode.dark-2026",
    fontFaces: [],
    themes: [
      {
        colors: { "editor.background": "#121314", "editor.foreground": "#bfbfbf" },
        id: "vscode.dark-2026",
        label: "Dark 2026",
        source: "fallback",
        sourceKind: "builtin",
        type: "dark"
      }
    ]
  };
}
