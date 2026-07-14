export type DesktopThemeType = "dark" | "light" | "hcDark" | "hcLight";

export type DesktopAppearanceBundle = {
  defaultTheme: string;
  fontFaces: Array<{
    dataUrl: string;
    format: string;
    slot: "ui" | "display" | "mono";
    style: "italic" | "normal";
    weight: string;
  }>;
  themes: Array<{
    colors: Record<string, string>;
    id: string;
    label: string;
    source: string;
    sourceKind: "builtin" | "custom";
    type: DesktopThemeType;
  }>;
};

export function buildDesktopAppearance(appRootInput?: string): DesktopAppearanceBundle;
