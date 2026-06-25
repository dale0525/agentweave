import { FormEvent } from "react";
import { Send } from "lucide-react";

import { AppIconButton } from "./AppIconButton";

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
  return (
    <form aria-label="Message composer" className="composer" onSubmit={onSubmit}>
      {error ? (
        <p className="composer-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="composer-input-row">
        <label className="sr-only" htmlFor="generalagent-message">
          Message GeneralAgent
        </label>
        <input
          id="generalagent-message"
          aria-label="Message GeneralAgent"
          value={draft}
          onChange={(event) => onChange(event.target.value)}
        />
        <AppIconButton disabled={isSending} label="Send message" type="submit">
          <Send size={18} aria-hidden="true" />
        </AppIconButton>
      </div>
    </form>
  );
}
