import * as Dialog from "@radix-ui/react-dialog";
import { LoaderCircle, Trash2, X } from "lucide-react";
import { useState } from "react";

import { DevSkillPackage } from "../../api";
import { AppIconButton } from "../AppIconButton";
import { useI18n } from "../../i18n/I18nProvider";

type DeleteSkillDialogProps = {
  disabled?: boolean;
  skillPackage: DevSkillPackage | null;
  onConfirm: (skillPackage: DevSkillPackage) => Promise<void>;
  onOpenChange: (open: boolean) => void;
};

export function DeleteSkillDialog({
  disabled = false,
  skillPackage,
  onConfirm,
  onOpenChange
}: DeleteSkillDialogProps): JSX.Element {
  const { t } = useI18n();
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
        <Dialog.Content aria-label={t("developer.deleteDialogLabel")} className="developer-dialog-content">
          <header className="developer-dialog-header">
            <Dialog.Title>{t("developer.deletePackage")}</Dialog.Title>
            <AppIconButton label={t("developer.closeDelete")} onClick={() => onOpenChange(false)}>
              <X aria-hidden="true" size={16} />
            </AppIconButton>
          </header>

          <div className="developer-dialog-body">
            <p>
              {skillPackage
                ? t("developer.deleteQuestion", { name: skillPackage.name })
                : t("developer.deleteFallbackQuestion")}
            </p>
          </div>

          <footer className="developer-dialog-footer">
            <button className="developer-secondary-button" onClick={() => onOpenChange(false)} type="button">
              {t("common.cancel")}
            </button>
            <button
              className="developer-danger-button"
              disabled={disabled || !skillPackage || isDeleting}
              onClick={() => void confirmDelete()}
              type="button"
            >
              {isDeleting ? <LoaderCircle aria-hidden="true" className="activity-spin" size={16} /> : <Trash2 aria-hidden="true" size={16} />}
              <span>{skillPackage ? t("developer.deleteNamed", { name: skillPackage.name }) : t("developer.deletePackage")}</span>
            </button>
          </footer>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
