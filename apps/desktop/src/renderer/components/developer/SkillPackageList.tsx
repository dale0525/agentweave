import { Plus, Search } from "lucide-react";
import { useMemo, useState } from "react";

import { DevSkillInventory, DevSkillPackage, DevSkillPackageKind } from "../../api";
import { packageStateLabel } from "./skillPackageDiagnostics";
import { useI18n } from "../../i18n/I18nProvider";

type SkillPackageListProps = {
  inventory: DevSkillInventory | null;
  selectedId: string | null;
  onCreate: () => void;
  onSelect: (id: string) => void;
};

const kindKey: Record<DevSkillPackageKind, string> = {
  combined: "developer.kindCombined",
  empty: "developer.kindEmpty",
  instruction: "developer.kindInstruction",
  invalid: "developer.kindInvalid",
  runtime: "developer.kindRuntime"
};

export function SkillPackageList({
  inventory,
  selectedId,
  onCreate,
  onSelect
}: SkillPackageListProps): JSX.Element {
  const { t } = useI18n();
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
          <h2>{t("developer.packages")}</h2>
          <p>{inventory ? t("developer.packageCount", { count: inventory.packages.length }) : t("developer.loadingInventory")}</p>
        </div>
        <button className="developer-primary-button" onClick={onCreate} type="button">
          <Plus aria-hidden="true" size={16} />
          <span>{t("developer.newSkill")}</span>
        </button>
      </div>

      <label className="search-box developer-search-box">
        <Search aria-hidden="true" size={16} />
        <span className="sr-only">{t("developer.search")}</span>
        <input
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("developer.searchPlaceholder")}
          type="search"
          value={query}
        />
      </label>

      {inventory && filteredPackages.length === 0 ? (
        <div className="developer-inline-empty-state">
          <h3>{t("developer.noPackages")}</h3>
          <p>{t("developer.noPackagesHint")}</p>
        </div>
      ) : (
        <div aria-label={t("developer.packages")} className="developer-package-list" role="list">
          {(filteredPackages.length > 0 ? filteredPackages : inventory?.packages ?? []).map(
            (skillPackage) => (
              <SkillPackageRow
                isSelected={selectedId === skillPackage.id}
                key={skillPackage.id}
                onSelect={onSelect}
                skillPackage={skillPackage}
                t={t}
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
  t: ReturnType<typeof useI18n>["t"];
}): JSX.Element {
  const { isSelected, onSelect, skillPackage, t } = props;
  const stateText = packageStateLabel(skillPackage, t);

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
      <span className="developer-kind-badge">{t(kindKey[skillPackage.packageKind])}</span>
    </button>
  );
}
