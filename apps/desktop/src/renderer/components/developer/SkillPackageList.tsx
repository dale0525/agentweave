import { Plus, Search } from "lucide-react";
import { useMemo, useState } from "react";

import { DevSkillInventory, DevSkillPackage, DevSkillPackageKind } from "../../api";
import { packageStateLabel } from "./skillPackageDiagnostics";

type SkillPackageListProps = {
  inventory: DevSkillInventory | null;
  selectedId: string | null;
  onCreate: () => void;
  onSelect: (id: string) => void;
};

const kindLabel: Record<DevSkillPackageKind, string> = {
  combined: "Combined",
  empty: "Empty",
  instruction: "Instruction",
  invalid: "Invalid",
  runtime: "Runtime"
};

export function SkillPackageList({
  inventory,
  selectedId,
  onCreate,
  onSelect
}: SkillPackageListProps): JSX.Element {
  const [query, setQuery] = useState("");

  const filteredPackages = useMemo(() => {
    const packages = inventory?.packages ?? [];
    const normalizedQuery = query.trim().toLowerCase();
    if (!normalizedQuery) {
      return packages;
    }

    return packages.filter((item) =>
      [item.name, item.path, item.description].some((value) =>
        value.toLowerCase().includes(normalizedQuery)
      )
    );
  }, [inventory, query]);

  return (
    <aside className="developer-list-pane">
      <div className="developer-pane-header">
        <div className="developer-pane-heading">
          <h2>Skill packages</h2>
          <p>{inventory ? `${inventory.packages.length} package(s)` : "Loading inventory"}</p>
        </div>
        <button className="developer-primary-button" onClick={onCreate} type="button">
          <Plus aria-hidden="true" size={16} />
          <span>New skill</span>
        </button>
      </div>

      <label className="search-box developer-search-box">
        <Search aria-hidden="true" size={16} />
        <span className="sr-only">Search skill packages</span>
        <input
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search packages"
          type="search"
          value={query}
        />
      </label>

      {inventory && filteredPackages.length === 0 ? (
        <div className="developer-inline-empty-state">
          <h3>No skill packages found</h3>
          <p>Adjust the search or create a new package.</p>
        </div>
      ) : (
        <div aria-label="Skill packages" className="developer-package-list" role="list">
          {(filteredPackages.length > 0 ? filteredPackages : inventory?.packages ?? []).map(
            (skillPackage) => (
              <SkillPackageRow
                isSelected={selectedId === skillPackage.id}
                key={skillPackage.id}
                onSelect={onSelect}
                skillPackage={skillPackage}
              />
            )
          )}
        </div>
      )}
    </aside>
  );
}

function SkillPackageRow(props: {
  isSelected: boolean;
  onSelect: (id: string) => void;
  skillPackage: DevSkillPackage;
}): JSX.Element {
  const { isSelected, onSelect, skillPackage } = props;
  const stateText = packageStateLabel(skillPackage);

  return (
    <button
      className={`developer-package-row${isSelected ? " developer-package-row-selected" : ""}`}
      onClick={() => onSelect(skillPackage.id)}
      type="button"
    >
      <span className="developer-package-row-copy">
        <strong>{skillPackage.name}</strong>
        <small>{stateText}</small>
      </span>
      <span className="developer-kind-badge">{kindLabel[skillPackage.packageKind]}</span>
    </button>
  );
}
