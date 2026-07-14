import * as Dialog from "@radix-ui/react-dialog";
import { Copy, X } from "lucide-react";
import { useState } from "react";

import { DevSkillInventory, DevSkillPackage } from "../../api";
import {
  buildCreateSkillPrompt,
  buildModifySkillPrompt
} from "../../devSkillPrompts";
import { AppIconButton } from "../AppIconButton";
import { useI18n } from "../../i18n/I18nProvider";

type SkillCreatorPromptDialogProps = {
  inventory: DevSkillInventory | null;
  promptPackage: DevSkillPackage | "new" | null;
  onOpenChange: (open: boolean) => void;
};

export function SkillCreatorPromptDialog({
  inventory,
  promptPackage,
  onOpenChange
}: SkillCreatorPromptDialogProps): JSX.Element {
  const { t } = useI18n();
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const copyLabel = t(copyState === "copied" ? "common.copied" : copyState === "failed" ? "common.copyFailed" : "common.copyPrompt");
  const open = promptPackage !== null && inventory !== null;
  const prompt =
    promptPackage === null || inventory === null
      ? ""
      : promptPackage === "new"
        ? buildCreateSkillPrompt(inventory.root)
        : buildModifySkillPrompt(inventory.root, promptPackage);

  const copyPrompt = async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(prompt);
        setCopyState("copied");
        window.setTimeout(() => setCopyState("idle"), 1200);
      }
    } catch {
      setCopyState("failed");
      window.setTimeout(() => setCopyState("idle"), 1200);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="developer-dialog-overlay" />
        <Dialog.Content aria-label={t("developer.promptTitle")} className="developer-dialog-content">
          <header className="developer-dialog-header">
            <Dialog.Title>{t("developer.promptTitle")}</Dialog.Title>
            <AppIconButton label={t("developer.closePrompt")} onClick={() => onOpenChange(false)}>
              <X aria-hidden="true" size={16} />
            </AppIconButton>
          </header>

          <div className="developer-dialog-body">
            <p>
              {t("developer.promptDescription")}
            </p>
            <pre className="developer-dialog-prompt">{prompt}</pre>
          </div>

          <footer className="developer-dialog-footer">
            <button className="developer-secondary-button" onClick={() => void copyPrompt()} type="button">
              <Copy aria-hidden="true" size={16} />
              <span>{copyLabel}</span>
            </button>
          </footer>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
