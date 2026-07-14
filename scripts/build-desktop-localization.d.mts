export type DesktopLocaleDefinition = {
  id: string;
  label: string;
  messages: Record<string, string>;
};

export type DesktopLocalizationBundle = {
  defaultLocale: string;
  locales: DesktopLocaleDefinition[];
};

export function buildDesktopLocalization(appRootInput?: string): DesktopLocalizationBundle;
