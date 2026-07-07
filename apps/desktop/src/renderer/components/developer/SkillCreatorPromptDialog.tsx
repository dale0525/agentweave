import * as Dialog from "@radix-ui/react-dialog";
import { Copy, X } from "lucide-react";
import { useState } from "react";

import { DevSkillInventory, DevSkillPackage } from "../../api";
import {
  buildCreateSkillPrompt,
  buildModifySkillPrompt
} from "../../devSkillPrompts";
import { AppIconButton } from "../AppIconButton";

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
  const [copyLabel, setCopyLabel] = useState("Copy prompt");
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
        setCopyLabel("Copied");
        window.setTimeout(() => setCopyLabel("Copy prompt"), 1200);
      }
    } catch {
      setCopyLabel("Copy failed");
      window.setTimeout(() => setCopyLabel("Copy prompt"), 1200);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="developer-dialog-overlay" />
        <Dialog.Content aria-label="skill-creator prompt" className="developer-dialog-content">
          <header className="developer-dialog-header">
            <Dialog.Title>skill-creator prompt</Dialog.Title>
            <AppIconButton label="Close prompt dialog" onClick={() => onOpenChange(false)}>
              <X aria-hidden="true" size={16} />
            </AppIconButton>
          </header>

          <div className="developer-dialog-body">
            <p>
              Use this prompt with the existing `skill-creator` skill to author or
              modify the local package.
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
