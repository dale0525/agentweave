import { ArrowLeft, RefreshCw, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  deleteDevSkill,
  DevSkillInventory,
  DevSkillPackage,
  listDevSkills,
  reloadDevSkills,
  validateDevSkills
} from "../api";
import { AppIconButton } from "../components/AppIconButton";
import { DeleteSkillDialog } from "../components/developer/DeleteSkillDialog";
import { SkillCreatorPromptDialog } from "../components/developer/SkillCreatorPromptDialog";
import { SkillPackageDetail } from "../components/developer/SkillPackageDetail";
import { SkillPackageList } from "../components/developer/SkillPackageList";
import { useI18n } from "../i18n/I18nProvider";

type DeveloperToolsProps = {
  onBack: () => void;
};

export function DeveloperTools({ onBack }: DeveloperToolsProps): JSX.Element {
  const { t } = useI18n();
  const actionFailureMessage = t("developer.actionFailed");
  const [inventory, setInventory] = useState<DevSkillInventory | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [activeSnapshotGeneration, setActiveSnapshotGeneration] = useState<number | null>(null);
  const [promptPackage, setPromptPackage] = useState<DevSkillPackage | "new" | null>(null);
  const [deletePackage, setDeletePackage] = useState<DevSkillPackage | null>(null);
  const inventoryRef = useRef<DevSkillInventory | null>(null);
  const operationSequenceRef = useRef(0);
  const operationInFlightRef = useRef(false);

  useEffect(() => {
    inventoryRef.current = inventory;
  }, [inventory]);

  const selectedPackage = useMemo(
    () => inventory?.packages.find((item) => item.id === selectedId) ?? inventory?.packages[0] ?? null,
    [inventory, selectedId]
  );

  const beginOperation = useCallback(() => {
    if (operationInFlightRef.current) {
      return null;
    }
    operationInFlightRef.current = true;
    const operationId = ++operationSequenceRef.current;
    setIsLoading(true);
    setActionError(null);
    return operationId;
  }, []);

  const finishOperation = useCallback((operationId: number) => {
    if (operationId !== operationSequenceRef.current) {
      return;
    }
    operationInFlightRef.current = false;
    setIsLoading(false);
  }, []);

  const loadInventory = useCallback(
    async (
      loader: () => Promise<DevSkillInventory> = listDevSkills,
      options?: { failureMessage?: string; preserveInventoryOnError?: boolean }
    ) => {
      const operationId = beginOperation();
      if (operationId === null) {
        return;
      }
      if (!options?.preserveInventoryOnError) {
        setLoadError(null);
      }
      try {
        const nextInventory = await loader();
        if (operationId !== operationSequenceRef.current) {
          return;
        }
        setInventory(nextInventory);
        setLoadError(null);
        setSelectedId((current) => {
          if (current && nextInventory.packages.some((item) => item.id === current)) {
            return current;
          }
          return nextInventory.packages[0]?.id ?? null;
        });
      } catch {
        if (operationId !== operationSequenceRef.current) {
          return;
        }
        if (options?.preserveInventoryOnError && inventoryRef.current) {
          setActionError(
            options.failureMessage ?? actionFailureMessage
          );
          return;
        }

        setInventory(null);
        setSelectedId(null);
        setLoadError(t("developer.apiUnavailable"));
      } finally {
        finishOperation(operationId);
      }
    },
    [actionFailureMessage, beginOperation, finishOperation, t]
  );

  useEffect(() => {
    void loadInventory();
  }, [loadInventory]);

  const handleDelete = useCallback(async (skillPackage: DevSkillPackage) => {
    const operationId = beginOperation();
    if (operationId === null) {
      return;
    }
    try {
      const nextInventory = await deleteDevSkill(skillPackage.id);
      if (operationId !== operationSequenceRef.current) {
        return;
      }
      setInventory(nextInventory);
      setSelectedId(nextInventory.packages[0]?.id ?? null);
      setDeletePackage(null);
    } catch {
      if (operationId !== operationSequenceRef.current) {
        return;
      }
      setActionError(actionFailureMessage);
      setDeletePackage(null);
    } finally {
      finishOperation(operationId);
    }
  }, [actionFailureMessage, beginOperation, finishOperation]);

  const handleReload = useCallback(async () => {
    const operationId = beginOperation();
    if (operationId === null) {
      return;
    }
    try {
      const response = await reloadDevSkills();
      if (operationId !== operationSequenceRef.current) {
        return;
      }
      const nextInventory = response.inventory;
      setInventory(nextInventory);
      setLoadError(null);
      setSelectedId((current) => {
        if (current && nextInventory.packages.some((item) => item.id === current)) {
          return current;
        }
        return nextInventory.packages[0]?.id ?? null;
      });
      setActiveSnapshotGeneration(response.activeGeneration);
    } catch {
      if (operationId !== operationSequenceRef.current) {
        return;
      }
      setActionError(actionFailureMessage);
    } finally {
      finishOperation(operationId);
    }
  }, [actionFailureMessage, beginOperation, finishOperation]);

  return (
    <main className="developer-screen" aria-label={t("developer.ariaLabel")}>
      <header className="top-bar developer-top-bar">
        <AppIconButton label={t("common.backToSettings")} onClick={onBack}>
          <ArrowLeft aria-hidden="true" size={18} />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>{t("developer.title")}</h1>
          <p>{loadError ? t("developer.apiDisconnected") : t("developer.apiConnected")}</p>
        </div>
        <div className="developer-top-bar-actions">
          <AppIconButton
            disabled={isLoading}
            label={t("developer.refresh")}
            onClick={() => {
              void loadInventory(listDevSkills, {
                failureMessage: actionFailureMessage,
                preserveInventoryOnError: inventory !== null
              });
            }}
          >
            <RefreshCw aria-hidden="true" size={18} />
          </AppIconButton>
          <AppIconButton
            disabled={isLoading}
            label={t("developer.validateAll")}
            onClick={() => {
              void loadInventory(validateDevSkills, {
                failureMessage: actionFailureMessage,
                preserveInventoryOnError: true
              });
            }}
          >
            <ShieldCheck aria-hidden="true" size={18} />
          </AppIconButton>
        </div>
      </header>

      {actionError || activeSnapshotGeneration !== null ? (
        <div aria-live="polite" className="developer-status-banner" role="status">
          {activeSnapshotGeneration !== null ? (
            <span>{t("developer.activeSnapshot", { generation: activeSnapshotGeneration })}</span>
          ) : null}
          {actionError ? (
            <>
              {" "}
              <span>{actionError}</span>
            </>
          ) : null}
        </div>
      ) : null}

      <section className="developer-workbench" aria-busy={isLoading}>
        {loadError ? (
          <div className="developer-empty-state">
            <h2>{t("developer.apiUnavailable")}</h2>
            <p>{t("developer.apiUnavailableHint")}</p>
          </div>
        ) : (
          <>
            <SkillPackageList
              inventory={inventory}
              selectedId={selectedPackage?.id ?? null}
              onCreate={() => setPromptPackage("new")}
              onSelect={setSelectedId}
            />
            <SkillPackageDetail
              isBusy={isLoading}
              inventory={inventory}
              onDelete={setDeletePackage}
              onModify={setPromptPackage}
              onReload={() => {
                void handleReload();
              }}
              skillPackage={selectedPackage}
            />
          </>
        )}
      </section>

      <SkillCreatorPromptDialog
        inventory={inventory}
        onOpenChange={(open) => {
          if (!open) {
            setPromptPackage(null);
          }
        }}
        promptPackage={promptPackage}
      />
      <DeleteSkillDialog
        disabled={isLoading}
        onConfirm={handleDelete}
        onOpenChange={(open) => {
          if (!open) {
            setDeletePackage(null);
          }
        }}
        skillPackage={deletePackage}
      />
    </main>
  );
}
