import * as Dialog from "@radix-ui/react-dialog";
import { LoaderCircle, Trash2, X } from "lucide-react";
import { useState } from "react";

import { DevSkillPackage } from "../../api";
import { AppIconButton } from "../AppIconButton";

type DeleteSkillDialogProps = {
  skillPackage: DevSkillPackage | null;
  onConfirm: (skillPackage: DevSkillPackage) => Promise<void>;
  onOpenChange: (open: boolean) => void;
};

export function DeleteSkillDialog({
  skillPackage,
  onConfirm,
  onOpenChange
}: DeleteSkillDialogProps): JSX.Element {
  const [isDeleting, setIsDeleting] = useState(false);

  const confirmDelete = async () => {
    if (!skillPackage) {
      return;
    }

    setIsDeleting(true);
    try {
      await onConfirm(skillPackage);
    } finally {
      setIsDeleting(false);
    }
  };

  return (
    <Dialog.Root open={skillPackage !== null} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="developer-dialog-overlay" />
        <Dialog.Content aria-label="delete skill package" className="developer-dialog-content">
          <header className="developer-dialog-header">
            <Dialog.Title>Delete package</Dialog.Title>
            <AppIconButton label="Close delete dialog" onClick={() => onOpenChange(false)}>
              <X aria-hidden="true" size={16} />
            </AppIconButton>
          </header>

          <div className="developer-dialog-body">
            <p>
              {skillPackage
                ? `Delete ${skillPackage.name} and remove its local development assets?`
                : "Delete this package?"}
            </p>
          </div>

          <footer className="developer-dialog-footer">
            <button className="developer-secondary-button" onClick={() => onOpenChange(false)} type="button">
              Cancel
            </button>
            <button
              className="developer-danger-button"
              disabled={!skillPackage || isDeleting}
              onClick={() => void confirmDelete()}
              type="button"
            >
              {isDeleting ? <LoaderCircle aria-hidden="true" className="activity-spin" size={16} /> : <Trash2 aria-hidden="true" size={16} />}
              <span>{skillPackage ? `Delete ${skillPackage.name}` : "Delete package"}</span>
            </button>
          </footer>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
