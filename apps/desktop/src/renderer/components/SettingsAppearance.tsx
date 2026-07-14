import { Badge, RadioCards, Text } from "@radix-ui/themes";
import { Check } from "lucide-react";
import type { CSSProperties } from "react";

import {
  getBundledAppearance,
  useAppearance
} from "../appearance/AppearanceProvider";
import { isHighContrastTheme, themePalette } from "../appearance/themePalette";
import type { DesktopThemeDefinition } from "../appearance/types";
import { useI18n } from "../i18n/I18nProvider";

const bundle = getBundledAppearance();

export function SettingsAppearance(): JSX.Element {
  const { t } = useI18n();
  const { activeTheme, selectTheme, themes } = useAppearance();
  const fontSlots = [...new Set(bundle.fontFaces.map((font) => font.slot))];

  return (
    <section className="settings-panel appearance-panel" aria-labelledby="settings-appearance-title">
      <div className="settings-panel-heading appearance-heading">
        <div>
          <h2 id="settings-appearance-title">{t("appearance.title")}</h2>
          <p>{t("appearance.description")}</p>
        </div>
        <Badge color="blue" radius="full" variant="soft">
          {t("appearance.themeCount", { count: themes.length })}
        </Badge>
      </div>
      <RadioCards.Root
        aria-label={t("appearance.colorTheme")}
        className="theme-radio-grid"
        onValueChange={selectTheme}
        value={activeTheme.id}
      >
        {themes.map((theme) => (
          <ThemeChoice
            active={theme.id === activeTheme.id}
            defaultTheme={theme.id === bundle.defaultTheme}
            key={theme.id}
            theme={theme}
            t={t}
          />
        ))}
      </RadioCards.Root>
      <Text as="p" className="appearance-footnote" color="gray" size="1">
        {fontSlots.length > 0
          ? t("appearance.fontsLoaded", { slots: fontSlots.join(", ") })
          : t("appearance.systemFont")}
      </Text>
    </section>
  );
}

type ThemeChoiceProps = {
  active: boolean;
  defaultTheme: boolean;
  theme: DesktopThemeDefinition;
  t: ReturnType<typeof useI18n>["t"];
};

function ThemeChoice({ active, defaultTheme, theme, t }: ThemeChoiceProps): JSX.Element {
  const palette = themePalette(theme);
  const previewStyle = {
    "--theme-preview-accent": palette.primary,
    "--theme-preview-background": palette.background,
    "--theme-preview-border": palette.border,
    "--theme-preview-muted": palette.muted,
    "--theme-preview-surface": palette.surface,
    "--theme-preview-text": palette.text,
    "--theme-preview-user": palette.userMessage
  } as CSSProperties;

  return (
    <RadioCards.Item className="theme-choice" value={theme.id}>
      <span className="theme-preview" style={previewStyle} aria-hidden="true">
        <span className="theme-preview-rail" />
        <span className="theme-preview-workspace">
          <span className="theme-preview-line long" />
          <span className="theme-preview-line" />
          <span className="theme-preview-message" />
        </span>
      </span>
      <span className="theme-choice-copy">
        <span className="theme-choice-title">
          <strong>{theme.label}</strong>
          {active ? <Check aria-label={t("common.selected")} size={15} /> : null}
        </span>
        <span className="theme-choice-meta">
          {defaultTheme ? t("common.default") : theme.sourceKind === "custom" ? t("appearance.custom") : "VS Code"}
          {isHighContrastTheme(theme) ? ` · ${t("appearance.highContrast")}` : ""}
        </span>
      </span>
    </RadioCards.Item>
  );
}
