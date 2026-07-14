import { Badge, RadioCards } from "@radix-ui/themes";
import { Check, Languages } from "lucide-react";

import { useI18n } from "../i18n/I18nProvider";

export function SettingsLanguage(): JSX.Element {
  const { locale, locales, selectLocale, t } = useI18n();

  return (
    <section className="settings-panel language-panel" aria-labelledby="settings-language-title">
      <div className="settings-panel-heading appearance-heading">
        <div>
          <h2 id="settings-language-title">{t("language.title")}</h2>
          <p>{t("language.description")}</p>
        </div>
        <Badge color="blue" radius="full" variant="soft">
          {t("language.availableCount", { count: locales.length })}
        </Badge>
      </div>
      <RadioCards.Root
        aria-label={t("language.displayLanguage")}
        className="language-radio-grid"
        onValueChange={selectLocale}
        value={locale}
      >
        {locales.map((entry) => (
          <RadioCards.Item className="language-choice" key={entry.id} value={entry.id}>
            <Languages aria-hidden="true" size={17} />
            <span className="language-choice-copy">
              <strong>{entry.label}</strong>
              <small>{entry.id}</small>
            </span>
            {entry.id === locale ? <Check aria-label={t("common.selected")} size={15} /> : null}
          </RadioCards.Item>
        ))}
      </RadioCards.Root>
    </section>
  );
}
