import { ArrowLeft, RefreshCw, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

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
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [promptPackage, setPromptPackage] = useState<DevSkillPackage | "new" | null>(null);
  const [deletePackage, setDeletePackage] = useState<DevSkillPackage | null>(null);

  const selectedPackage = useMemo(
    () => inventory?.packages.find((item) => item.id === selectedId) ?? inventory?.packages[0] ?? null,
    [inventory, selectedId]
  );

  const loadInventory = useCallback(
    async (loader: () => Promise<DevSkillInventory> = listDevSkills) => {
      setIsLoading(true);
      setError(null);
      try {
        const nextInventory = await loader();
        setInventory(nextInventory);
        setSelectedId((current) => {
          if (current && nextInventory.packages.some((item) => item.id === current)) {
            return current;
          }
          return nextInventory.packages[0]?.id ?? null;
        });
      } catch {
        setInventory(null);
        setSelectedId(null);
        setError("Development API is not available");
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
    const nextInventory = await deleteDevSkill(skillPackage.id);
    setInventory(nextInventory);
    setSelectedId(nextInventory.packages[0]?.id ?? null);
    setDeletePackage(null);
  }, []);

  return (
    <main className="developer-screen" aria-label="Developer Tools">
      <header className="top-bar developer-top-bar">
        <AppIconButton label="Back to settings" onClick={onBack}>
          <ArrowLeft aria-hidden="true" size={18} />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>Developer Tools</h1>
          <p>{error ? "Development API disconnected" : "Development API connected"}</p>
        </div>
        <div className="developer-top-bar-actions">
          <AppIconButton
            label="Refresh skill packages"
            onClick={() => {
              void loadInventory();
            }}
          >
            <RefreshCw aria-hidden="true" size={18} />
          </AppIconButton>
          <AppIconButton
            label="Validate all skill packages"
            onClick={() => {
              void loadInventory(validateDevSkills);
            }}
          >
            <ShieldCheck aria-hidden="true" size={18} />
          </AppIconButton>
        </div>
      </header>

      <section className="developer-workbench" aria-busy={isLoading}>
        {error ? (
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
                void loadInventory(reloadDevSkills);
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
