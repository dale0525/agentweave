export type TranslationValues = Record<string, string | number>;

export type DesktopLocaleDefinition = {
  id: string;
  label: string;
  messages: Record<string, string>;
};

export type DesktopLocalizationBundle = {
  defaultLocale: string;
  locales: DesktopLocaleDefinition[];
};
