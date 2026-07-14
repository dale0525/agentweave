import type { DesktopThemeDefinition } from "./types";

const DARK_FALLBACK = {
  background: "#1e1e1e",
  border: "#3c3c3c",
  danger: "#f48771",
  dangerSurface: "#3a1d1d",
  muted: "#9d9d9d",
  primary: "#0078d4",
  primaryHover: "#026ec1",
  primaryText: "#4daafc",
  surface: "#252526",
  surfaceMuted: "#2d2d2d",
  text: "#cccccc",
  userMessage: "#2c2d2e"
};

const LIGHT_FALLBACK = {
  background: "#ffffff",
  border: "#d4d4d4",
  danger: "#a1260d",
  dangerSurface: "#fdeded",
  muted: "#616161",
  primary: "#0069cc",
  primaryHover: "#005fb8",
  primaryText: "#005fb8",
  surface: "#f8f8f8",
  surfaceMuted: "#f0f0f0",
  text: "#202020",
  userMessage: "#eef4fb"
};

export type ThemePalette = ReturnType<typeof themePalette>;

export function themePalette(theme: DesktopThemeDefinition) {
  const colors = theme.colors;
  const fallback = isLightTheme(theme) ? LIGHT_FALLBACK : DARK_FALLBACK;
  const background = color(colors, "editor.background", fallback.background);
  const surface = color(
    colors,
    "sideBar.background",
    color(colors, "panel.background", fallback.surface)
  );
  const surfaceMuted = color(
    colors,
    "editorWidget.background",
    color(colors, "input.background", fallback.surfaceMuted)
  );
  const border = color(
    colors,
    "sideBar.border",
    color(colors, "panel.border", color(colors, "editorWidget.border", fallback.border))
  );
  const primary = color(colors, "button.background", color(colors, "focusBorder", fallback.primary));
  return {
    background,
    border,
    borderStrong: color(colors, "input.border", border),
    danger: color(colors, "errorForeground", color(colors, "inputValidation.errorBorder", fallback.danger)),
    dangerSurface: color(colors, "inputValidation.errorBackground", fallback.dangerSurface),
    focus: color(colors, "focusBorder", primary),
    muted: color(colors, "descriptionForeground", fallback.muted),
    primary,
    primaryHover: color(colors, "button.hoverBackground", fallback.primaryHover),
    primaryText: color(colors, "textLink.foreground", fallback.primaryText),
    primaryTextOnFill: color(colors, "button.foreground", "#ffffff"),
    subtle: color(colors, "disabledForeground", color(colors, "descriptionForeground", fallback.muted)),
    surface,
    surfaceMuted,
    surfaceRaised: color(colors, "list.hoverBackground", surfaceMuted),
    text: color(colors, "foreground", color(colors, "editor.foreground", fallback.text)),
    userMessage: color(
      colors,
      "chat.requestBubbleBackground",
      color(colors, "list.inactiveSelectionBackground", fallback.userMessage)
    )
  };
}

export function themeCssVariables(theme: DesktopThemeDefinition): Record<string, string> {
  const palette = themePalette(theme);
  return {
    "--color-background": palette.background,
    "--color-border": palette.border,
    "--color-border-strong": palette.borderStrong,
    "--color-danger": palette.danger,
    "--color-danger-surface": palette.dangerSurface,
    "--color-primary": palette.primary,
    "--color-primary-fill": palette.primary,
    "--color-primary-fill-hover": palette.primaryHover,
    "--color-primary-fill-text": palette.primaryTextOnFill,
    "--color-primary-hover": palette.primaryHover,
    "--color-primary-soft": palette.userMessage,
    "--color-primary-text": palette.primaryText,
    "--color-surface": palette.surface,
    "--color-surface-muted": palette.surfaceMuted,
    "--color-surface-raised": palette.surfaceRaised,
    "--color-text": palette.text,
    "--color-text-muted": palette.muted,
    "--color-text-subtle": palette.subtle,
    "--color-user-message": palette.userMessage,
    "--shadow-focus": `0 0 0 2px ${palette.focus}`
  };
}

export function isLightTheme(theme: DesktopThemeDefinition): boolean {
  return theme.type === "light" || theme.type === "hcLight";
}

export function isHighContrastTheme(theme: DesktopThemeDefinition): boolean {
  return theme.type === "hcDark" || theme.type === "hcLight";
}

function color(colors: Record<string, string>, key: string, fallback: string): string {
  const value = colors[key];
  return typeof value === "string" && value.trim() ? value : fallback;
}
