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

type DeveloperToolsProps = {
  onBack: () => void;
};

export function DeveloperTools({ onBack }: DeveloperToolsProps): JSX.Element {
  const [inventory, setInventory] = useState<DevSkillInventory | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [promptPackage, setPromptPackage] = useState<DevSkillPackage | "new" | null>(null);
  const [deletePackage, setDeletePackage] = useState<DevSkillPackage | null>(null);
  const inventoryRef = useRef<DevSkillInventory | null>(null);

  useEffect(() => {
    inventoryRef.current = inventory;
  }, [inventory]);

  const selectedPackage = useMemo(
    () => inventory?.packages.find((item) => item.id === selectedId) ?? inventory?.packages[0] ?? null,
    [inventory, selectedId]
  );

  const loadInventory = useCallback(
    async (
      loader: () => Promise<DevSkillInventory> = listDevSkills,
      options?: { failureMessage?: string; preserveInventoryOnError?: boolean }
    ) => {
      setIsLoading(true);
      setActionError(null);
      if (!options?.preserveInventoryOnError) {
        setLoadError(null);
      }
      try {
        const nextInventory = await loader();
        setInventory(nextInventory);
        setLoadError(null);
        setSelectedId((current) => {
          if (current && nextInventory.packages.some((item) => item.id === current)) {
            return current;
          }
          return nextInventory.packages[0]?.id ?? null;
        });
      } catch {
        if (options?.preserveInventoryOnError && inventoryRef.current) {
          setActionError(
            options.failureMessage ?? "Action failed. Keep the current inventory and try again."
          );
          return;
        }

        setInventory(null);
        setSelectedId(null);
        setLoadError("Development API is not available");
      } finally {
        setIsLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    void loadInventory();
  }, [loadInventory]);

  const handleDelete = useCallback(async (skillPackage: DevSkillPackage) => {
    setActionError(null);
    try {
      const nextInventory = await deleteDevSkill(skillPackage.id);
      setInventory(nextInventory);
      setSelectedId(nextInventory.packages[0]?.id ?? null);
      setDeletePackage(null);
    } catch {
      setActionError("Action failed. Keep the current inventory and try again.");
    }
  }, []);

  return (
    <main className="developer-screen" aria-label="Developer Tools">
      <header className="top-bar developer-top-bar">
        <AppIconButton label="Back to settings" onClick={onBack}>
          <ArrowLeft aria-hidden="true" size={18} />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>Developer Tools</h1>
          <p>{loadError ? "Development API disconnected" : "Development API connected"}</p>
        </div>
        <div className="developer-top-bar-actions">
          <AppIconButton
            label="Refresh skill packages"
            onClick={() => {
              void loadInventory(listDevSkills, {
                failureMessage: "Action failed. Keep the current inventory and try again.",
                preserveInventoryOnError: inventory !== null
              });
            }}
          >
            <RefreshCw aria-hidden="true" size={18} />
          </AppIconButton>
          <AppIconButton
            label="Validate all skill packages"
            onClick={() => {
              void loadInventory(validateDevSkills, {
                failureMessage: "Action failed. Keep the current inventory and try again.",
                preserveInventoryOnError: true
              });
            }}
          >
            <ShieldCheck aria-hidden="true" size={18} />
          </AppIconButton>
        </div>
      </header>

      {actionError ? (
        <div aria-live="polite" className="developer-status-banner" role="status">
          {actionError}
        </div>
      ) : null}

      <section className="developer-workbench" aria-busy={isLoading}>
        {loadError ? (
          <div className="developer-empty-state">
            <h2>Development API is not available</h2>
            <p>Start the server with GENERAL_AGENT_DEV_API=1 to manage local skills.</p>
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
              inventory={inventory}
              onDelete={setDeletePackage}
              onModify={setPromptPackage}
              onReload={() => {
                void loadInventory(reloadDevSkills, {
                  failureMessage: "Action failed. Keep the current inventory and try again.",
                  preserveInventoryOnError: true
                });
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
