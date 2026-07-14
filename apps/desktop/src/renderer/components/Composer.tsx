import { FormEvent } from "react";
import { Send } from "lucide-react";

import { AppIconButton } from "./AppIconButton";
import { useI18n } from "../i18n/I18nProvider";

type ComposerProps = {
  draft: string;
  error: string | null;
  isSending: boolean;
  onChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
};

export function Composer({
  draft,
  error,
  isSending,
  onChange,
  onSubmit
}: ComposerProps): JSX.Element {
  const { t } = useI18n();
  return (
    <form aria-label={t("composer.ariaLabel")} className="composer" onSubmit={onSubmit}>
      {error ? (
        <p className="composer-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="composer-input-row">
        <label className="sr-only" htmlFor="agentweave-message">
          {t("composer.message")}
        </label>
        <input
          id="agentweave-message"
          aria-label={t("composer.message")}
          value={draft}
          onChange={(event) => onChange(event.target.value)}
        />
        <AppIconButton disabled={isSending} label={t("composer.send")} type="submit">
          <Send size={18} aria-hidden="true" />
        </AppIconButton>
      </div>
    </form>
  );
}
