import { Copy, RefreshCw, ShieldAlert, Sparkles, Trash2 } from "lucide-react";
import { useState } from "react";

import { DevSkillInventory, DevSkillPackage, DevSkillPackageKind } from "../../api";
import { buildModifySkillPrompt } from "../../devSkillPrompts";
import {
  packageHasBlockingDiagnostics,
  packageValidationHeading
} from "./skillPackageDiagnostics";

type SkillPackageDetailProps = {
  isBusy: boolean;
  inventory: DevSkillInventory | null;
  skillPackage: DevSkillPackage | null;
  onDelete: (skillPackage: DevSkillPackage) => void;
  onModify: (skillPackage: DevSkillPackage) => void;
  onReload: () => void;
};

const kindLabel: Record<DevSkillPackageKind, string> = {
  combined: "Combined",
  empty: "Empty",
  instruction: "Instruction",
  invalid: "Invalid",
  runtime: "Runtime"
};

export function SkillPackageDetail({
  isBusy,
  inventory,
  skillPackage,
  onDelete,
  onModify,
  onReload
}: SkillPackageDetailProps): JSX.Element {
  const [copyLabel, setCopyLabel] = useState("Copy prompt");

  if (!inventory || inventory.packages.length === 0 || !skillPackage) {
    return (
      <section className="developer-detail-pane">
        <div className="developer-empty-state developer-detail-empty-state">
          <h2>Package details</h2>
          <p>Select a package to inspect prompts, diagnostics, and exported tools.</p>
        </div>
      </section>
    );
  }

  const prompt = buildModifySkillPrompt(inventory.root, skillPackage);
  const diagnosticsCount =
    skillPackage.validation.errors.length + skillPackage.validation.warnings.length;
  const hasBlockingDiagnostics = packageHasBlockingDiagnostics(skillPackage);
  const validationHeading = packageValidationHeading(skillPackage);

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
    <section className="developer-detail-pane">
      <div className="developer-detail-header">
        <div className="developer-detail-path">skills/{skillPackage.path}</div>
        <div className="developer-detail-title-row">
          <div className="developer-detail-title-copy">
            <h2>{skillPackage.name}</h2>
            <p>{skillPackage.description}</p>
          </div>
          <span className="developer-kind-badge">{kindLabel[skillPackage.packageKind]}</span>
        </div>
      </div>

      <div className="developer-detail-grid">
        <section className="developer-detail-section">
          <header className="developer-section-heading">
            <h3>Package files</h3>
          </header>
          <div className="developer-file-list" role="list">
            <div className="developer-file-row" role="listitem">
              <span className="developer-file-name">skill.json</span>
              <span className="developer-file-state">
                {skillPackage.hasRuntimeManifest ? "Present" : "Missing"}
              </span>
            </div>
            <div className="developer-file-row" role="listitem">
              <span className="developer-file-name">SKILL.md</span>
              <span className="developer-file-state">
                {skillPackage.hasSkillMd ? "Present" : "SKILL.md missing"}
              </span>
            </div>
          </div>
        </section>

        <section className="developer-detail-section">
          <header className="developer-section-heading">
            <h3>Validation</h3>
          </header>
          <div
            className={`developer-validation-panel${
              hasBlockingDiagnostics ? "" : " developer-validation-panel-pass"
            }`}
          >
            <strong>{validationHeading}</strong>
            <p>
              {!hasBlockingDiagnostics
                ? `Bundle readiness: ${skillPackage.bundleReady ? "ready" : "not ready"}`
                : `${diagnosticsCount} diagnostic item(s) reported`}
            </p>
            {skillPackage.validation.errors.length > 0 ? (
              <ul className="developer-validation-list">
                {skillPackage.validation.errors.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : null}
            {skillPackage.validation.warnings.length > 0 ? (
              <ul className="developer-validation-list">
                {skillPackage.validation.warnings.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : null}
          </div>
        </section>

        <section className="developer-detail-section developer-detail-section-wide">
          <header className="developer-section-heading">
            <h3>Exported tools</h3>
          </header>
          {skillPackage.runtimeTools.length > 0 ? (
            <div className="developer-tool-list" role="list">
              {skillPackage.runtimeTools.map((toolName) => (
                <div className="developer-tool-row" key={toolName} role="listitem">
                  <span className="developer-tool-icon" aria-hidden="true">
                    <Sparkles size={16} />
                  </span>
                  <span className="developer-tool-copy">
                    <strong>{toolName}</strong>
                    <small>Runtime export</small>
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <div className="developer-inline-empty-state">
              <h3>No runtime tools exported</h3>
              <p>This package currently provides instruction assets only.</p>
            </div>
          )}
        </section>
      </div>

      <div className="developer-detail-actions">
        <button
          className="developer-primary-button"
          onClick={() => onModify(skillPackage)}
          type="button"
        >
          <ShieldAlert aria-hidden="true" size={16} />
          <span>Modify with skill-creator</span>
        </button>
        <button className="developer-secondary-button" onClick={() => void copyPrompt()} type="button">
          <Copy aria-hidden="true" size={16} />
          <span>{copyLabel}</span>
        </button>
        <button
          className="developer-secondary-button"
          disabled={isBusy}
          onClick={onReload}
          type="button"
        >
          <RefreshCw aria-hidden="true" size={16} />
          <span>Reload diagnostics</span>
        </button>
      </div>

      <section className="developer-danger-zone">
        <div>
          <h3>Danger zone</h3>
          <p>Deleting this package removes local developer assets for {skillPackage.name}.</p>
        </div>
        <button
          className="developer-danger-button"
          disabled={isBusy}
          onClick={() => onDelete(skillPackage)}
          type="button"
        >
          <Trash2 aria-hidden="true" size={16} />
          <span>Delete package</span>
        </button>
      </section>

      <aside className="developer-prompt-preview" aria-label="Prompt preview">
        <div className="developer-prompt-preview-header">
          <span>Prompt preview</span>
        </div>
        <pre>{prompt}</pre>
      </aside>
    </section>
  );
}
