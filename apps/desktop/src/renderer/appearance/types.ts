export type DesktopThemeType = "dark" | "light" | "hcDark" | "hcLight";

export type DesktopThemeDefinition = {
  colors: Record<string, string>;
  id: string;
  label: string;
  source: string;
  sourceKind: "builtin" | "custom";
  type: DesktopThemeType;
};

export type PackagedFontFace = {
  dataUrl: string;
  format: string;
  slot: "ui" | "display" | "mono";
  style: "italic" | "normal";
  weight: string;
};

export type DesktopAppearanceBundle = {
  defaultTheme: string;
  fontFaces: PackagedFontFace[];
  themes: DesktopThemeDefinition[];
};
