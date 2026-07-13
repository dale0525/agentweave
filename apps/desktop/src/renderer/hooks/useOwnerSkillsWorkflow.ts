import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  OwnerSkillApproval,
  OwnerLayeredSkill,
  OwnerSkillPackage,
  OwnerSkillPackageSummary,
  OwnerSkillRevision,
  OwnerSkillValidation
} from "../api";
import { ownerClient } from "../ownerClient";
import { OwnerPolicy, requestApprovalSurface } from "../ownerBridge";
import { DraftForm } from "../components/ownerSkills/CreateDraftDialog";
import { OwnerApprovalOperation } from "../components/ownerSkills/SkillApprovalDialog";

export type PendingApproval = {
  approval: OwnerSkillApproval;
  baselineRevision: OwnerSkillRevision | null;
  operation: OwnerApprovalOperation;
  revision: OwnerSkillRevision;
};

type DraftValidationState = {
  packageId: string;
  revisionId: string;
  validation: OwnerSkillValidation;
};

export const pendingValidation: OwnerSkillValidation = {
  ok: false,
  errors: ["Validation has not run"],
  warnings: []
};

export function useOwnerSkillsWorkflow(policy: OwnerPolicy) {
  const [summaries, setSummaries] = useState<OwnerSkillPackageSummary[]>([]);
  const [details, setDetails] = useState<Record<string, OwnerSkillPackage>>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null);
  const [approvalError, setApprovalError] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [createdPackageId, setCreatedPackageId] = useState<string | null>(null);
  const [draftInstructions, setDraftInstructions] = useState("");
  const [draftRequiredTools, setDraftRequiredTools] = useState("");
  const [draftValidationState, setDraftValidationState] = useState<DraftValidationState | null>(null);
  const inventoryRequest = useRef(0);
  const detailRequest = useRef(0);
  const validationGeneration = useRef(0);
  const selected = selectedId ? details[selectedId] ?? null : null;
  const editableDraft = selected?.editable_draft ?? null;
  const selectedRevision = editableDraft
    ?? selected?.revisions.find((revision) => revision.revision_id === selected.active_revision_id)
    ?? selected?.revisions[0]
    ?? null;
  const draftIdentity = selected && editableDraft
    ? { packageId: selected.package_id, revisionId: editableDraft.revision_id }
    : null;
  const draftIdentityRef = useRef(draftIdentity);
  draftIdentityRef.current = draftIdentity;
  const draftValidation = draftIdentity
    && draftValidationState?.packageId === draftIdentity.packageId
    && draftValidationState.revisionId === draftIdentity.revisionId
    ? draftValidationState.validation
    : pendingValidation;

  const packages = useMemo(() => summaries.map((summary) => ({
    ...summary,
    display_name: details[summary.package_id]?.display_name ?? summary.display_name
  })), [details, summaries]);

  const loadDetail = useCallback(async (packageId: string): Promise<OwnerSkillPackage | null> => {
    const requestId = ++detailRequest.current;
    try {
      const detail = await ownerClient.skillDetail(packageId);
      if (requestId !== detailRequest.current) return null;
      setDetails((current) => ({ ...current, [packageId]: normalizeDetail(detail) }));
      return detail;
    } catch (error) {
      if (requestId === detailRequest.current) {
        setLoadError(errorMessage(error, "Unable to load skill package detail"));
      }
      return null;
    }
  }, []);

  const loadInventory = useCallback(async (retainExisting = false): Promise<void> => {
    const requestId = ++inventoryRequest.current;
    if (!retainExisting) setLoadError(null);
    try {
      const inventory = await ownerClient.listSkills();
      if (requestId !== inventoryRequest.current) return;
      const next = normalizeInventory(inventory.packages);
      setSummaries(next);
      const nextSelected = selectedId && next.some((item) => item.package_id === selectedId)
        ? selectedId
        : next[0]?.package_id ?? null;
      setSelectedId(nextSelected);
      setLoadError(null);
      if (nextSelected) await loadDetail(nextSelected);
    } catch (error) {
      if (requestId === inventoryRequest.current) {
        setLoadError(errorMessage(error, "Unable to refresh owner skill inventory"));
      }
    }
  }, [loadDetail, selectedId]);

  useEffect(() => { void loadInventory(); }, []);
  useEffect(() => {
    if (!selectedId) return;
    void loadDetail(selectedId);
  }, [loadDetail, selectedId]);
  useEffect(() => {
    validationGeneration.current += 1;
    if (!editableDraft) {
      setDraftInstructions("");
      setDraftRequiredTools("");
      setDraftValidationState(null);
      return;
    }
    setDraftInstructions(editableDraft.instructions);
    setDraftRequiredTools(editableDraft.requirements.runtime_tools.join(", "));
    setDraftValidationState({
      packageId: selected?.package_id ?? "",
      revisionId: editableDraft.revision_id,
      validation: normalizeValidation(editableDraft.validation)
    });
  }, [selected?.package_id, editableDraft?.revision_id, editableDraft?.instructions, editableDraft?.validation]);

  const mutate = async (name: string, operation: () => Promise<void>) => {
    if (busy) return;
    setBusy(name); setStatus(null);
    try { await operation(); } finally { setBusy(null); }
  };
  const reconcile = async (message: string) => {
    await loadInventory(true);
    setStatus(message);
  };
  const invalidateDraft = (instructions: string, requiredTools: string) => {
    validationGeneration.current += 1;
    if (!selected || !editableDraft) {
      setDraftValidationState(null);
      return;
    }
    setDraftValidationState({
      packageId: selected.package_id,
      revisionId: editableDraft.revision_id,
      validation: pendingValidation
    });
    setDetails((current) => ({
      ...current,
      [selected.package_id]: mergeValidation(
        current[selected.package_id] ?? selected,
        editableDraft.revision_id,
        pendingValidation,
        instructions,
        splitValues(requiredTools)
      )
    }));
  };

  return {
    policy, packages, selectedId, selected, selectedRevision, editableDraft, search, status,
    loadError, busy, pendingApproval, approvalError, createOpen,
    createdPackageId,
    draftInstructions, draftRequiredTools, draftValidation,
    setSearch, setCreateOpen,
    selectPackage: (item: OwnerSkillPackageSummary) => {
      validationGeneration.current += 1;
      setSelectedId(item.package_id);
      setStatus(null);
    },
    refresh: () => loadInventory(true),
    changeInstructions: (value: string) => {
      setDraftInstructions(value);
      invalidateDraft(value, draftRequiredTools);
    },
    changeRequiredTools: (value: string) => {
      setDraftRequiredTools(value);
      invalidateDraft(draftInstructions, value);
    },
    closeApproval: () => { if (busy !== "approval") setPendingApproval(null); },
    requestActivation: () => void mutate("activation", async () => {
      if (!selected || !editableDraft || !draftValidation.ok || !sameDraft(draftIdentity, selected, editableDraft)) return;
      try {
        const approval = await ownerClient.requestActivation(editableDraft.revision_id);
        setApprovalError(null);
        setPendingApproval({
          approval,
          baselineRevision: activeRevisionFor(selected),
          operation: "activation",
          revision: editableDraft
        });
      } catch (error) { setStatus(errorMessage(error, "Activation request failed")); }
    }),
    approvePending: () => void mutate("approval", async () => {
      if (!pendingApproval) return;
      try {
        setApprovalError(null);
        const observation = await requestApprovalSurface(
          pendingApproval.approval.approval_id
        );
        if (observation.status === "closed") {
          setApprovalError("Approval window closed. The request remains pending.");
          return;
        }
        if (observation.status === "disposed") {
          throw new Error("Approval observation was disposed before a decision");
        }
        if (observation.status === "load_failed") {
          throw new Error("Approval window failed to load");
        }
        setPendingApproval(null);
        if (observation.decision === "reject") {
          await reconcile("Skill operation rejected");
          return;
        }
        const report = isRecord(observation.resolution) ? observation.resolution : {};
        const generation = numberValue(report.active_generation) ?? numberValue(report.generation);
        await reconcile(pendingApproval.operation === "removal"
          ? "Skill removed"
          : generation ? `Active snapshot ${generation}` : "Skill operation approved");
      } catch (error) { setApprovalError(errorMessage(error, "Approval failed")); }
    }),
    rollback: (revision: OwnerSkillRevision) => void mutate("rollback", async () => {
      if (!selected) return;
      try {
        const result = await ownerClient.rollback(selected.package_id, revision.revision_id);
        if (result.approval_id) {
          setPendingApproval({
            approval: result as OwnerSkillApproval,
            baselineRevision: activeRevisionFor(selected),
            operation: "rollback",
            revision
          });
        } else await reconcile(`Rolled back to ${revision.version}`);
      } catch { setStatus("Rollback failed. The current revision remains active."); }
    }),
    disable: () => void mutate("disable", async () => {
      if (!selected) return;
      try { await ownerClient.disable(selected.package_id); await reconcile("Skill disabled"); }
      catch (error) { setStatus(errorMessage(error, "Disable failed. The current revision remains active.")); }
    }),
    requestRemoval: () => void mutate("removal", async () => {
      const latestValidation = editableDraft ? draftValidation : selectedRevision?.validation;
      if (!selected || !selectedRevision || !latestValidation?.ok) return;
      try {
        const approval = await ownerClient.requestRemoval(selected.package_id);
        setPendingApproval({
          approval,
          baselineRevision: activeRevisionFor(selected),
          operation: "removal",
          revision: selectedRevision
        });
      } catch (error) { setStatus(errorMessage(error, "Removal request failed")); }
    }),
    saveDraft: () => void mutate("save", async () => {
      if (!selected || !editableDraft) return;
      try {
        validationGeneration.current += 1;
        await ownerClient.updateDraft(editableDraft.revision_id, draftFiles(selected, editableDraft, draftInstructions, draftRequiredTools));
        setDraftValidationState({
          packageId: selected.package_id,
          revisionId: editableDraft.revision_id,
          validation: pendingValidation
        });
        await loadDetail(selected.package_id);
        setStatus("Draft saved");
      } catch (error) { setStatus(errorMessage(error, "Draft save failed")); }
    }),
    validateDraft: () => void mutate("validate", async () => {
      if (!selected || !editableDraft) return;
      const packageId = selected.package_id;
      const revisionId = editableDraft.revision_id;
      const instructions = draftInstructions;
      const requiredTools = draftRequiredTools;
      const generation = validationGeneration.current;
      try {
        await ownerClient.updateDraft(
          revisionId,
          draftFiles(selected, editableDraft, instructions, requiredTools)
        );
        const validation = normalizeValidation(await ownerClient.validateDraft(revisionId));
        if (!isCurrentDraft(draftIdentityRef.current, packageId, revisionId, generation, validationGeneration.current)) return;
        setDraftValidationState({ packageId, revisionId, validation });
        setDetails((current) => ({
          ...current,
          [packageId]: mergeValidation(
            current[packageId] ?? selected,
            revisionId,
            validation,
            instructions,
            splitValues(requiredTools)
          )
        }));
      } catch (error) {
        if (!isCurrentDraft(draftIdentityRef.current, packageId, revisionId, generation, validationGeneration.current)) return;
        setDraftValidationState({
          packageId,
          revisionId,
          validation: { ...draftValidation, ok: false, errors: [errorMessage(error, "Draft validation failed")] }
        });
      }
    }),
    createDraft: (form: DraftForm) => void mutate("create", async () => {
      try {
        await ownerClient.createDraft({
          package_id: form.packageId, display_name: form.displayName,
          description: form.description, kind: form.kind,
          required_tools: splitValues(form.requiredTools)
        });
        setCreateOpen(false);
        await loadInventory(true);
        setSelectedId(form.packageId);
        setCreatedPackageId(form.packageId);
        await loadDetail(form.packageId);
      } catch (error) { setStatus(errorMessage(error, "Draft creation failed")); }
    })
  };
}

function normalizeInventory(items: OwnerLayeredSkill[]): OwnerSkillPackageSummary[] {
  return items
    .map((item) => item.effective ?? item.managed)
    .filter((item): item is OwnerSkillPackageSummary => item !== null)
    .sort((a, b) => a.package_id.localeCompare(b.package_id));
}

function activeRevisionFor(detail: OwnerSkillPackage): OwnerSkillRevision | null {
  return detail.revisions.find((revision) => revision.revision_id === detail.active_revision_id) ?? null;
}

function sameDraft(
  identity: { packageId: string; revisionId: string } | null,
  detail: OwnerSkillPackage,
  revision: OwnerSkillRevision
): boolean {
  return identity?.packageId === detail.package_id && identity.revisionId === revision.revision_id;
}

function isCurrentDraft(
  identity: { packageId: string; revisionId: string } | null,
  packageId: string,
  revisionId: string,
  startedGeneration: number,
  currentGeneration: number
): boolean {
  return startedGeneration === currentGeneration
    && identity?.packageId === packageId
    && identity.revisionId === revisionId;
}

function normalizeDetail(detail: OwnerSkillPackage): OwnerSkillPackage {
  return {
    ...detail,
    revisions: detail.revisions.map((revision) => ({ ...revision, validation: normalizeValidation(revision.validation) })),
    editable_draft: detail.editable_draft
      ? { ...detail.editable_draft, validation: normalizeValidation(detail.editable_draft.validation) }
      : null
  };
}

function normalizeValidation(validation: OwnerSkillValidation): OwnerSkillValidation {
  return { ...validation, ok: Boolean(validation.ok), errors: validation.errors ?? ["Validation has not run"], warnings: validation.warnings ?? [] };
}

function mergeValidation(
  detail: OwnerSkillPackage,
  revisionId: string,
  validation: OwnerSkillValidation,
  instructions: string,
  requiredTools: string[]
): OwnerSkillPackage {
  const merge = (revision: OwnerSkillRevision) => revision.revision_id === revisionId ? {
    ...revision,
    instructions,
    validation,
    requirements: {
      runtime_tools: validation.requiredTools ?? requiredTools,
      capabilities: validation.requiredCapabilities ?? revision.requirements.capabilities,
      connectors: validation.requiredConnectors ?? revision.requirements.connectors,
      packages: validation.dependencies ?? revision.requirements.packages
    },
    permission_diff: validation.permissionDiff ?? revision.permission_diff
  } : revision;
  return {
    ...detail,
    revisions: detail.revisions.map(merge),
    editable_draft: detail.editable_draft ? merge(detail.editable_draft) : null
  };
}

function draftFiles(skill: OwnerSkillPackage, revision: OwnerSkillRevision, instructions: string, requiredTools: string) {
  const descriptor = {
    schemaVersion: 1, id: skill.package_id, version: revision.version,
    displayName: skill.display_name, kind: revision.kind,
    package: { includeInstructions: true, includeRuntime: false },
    compatibility: { minimumRuntimeVersion: null, platforms: [] },
    requires: {
      packages: revision.requirements.packages,
      capabilities: revision.requirements.capabilities,
      runtimeTools: splitValues(requiredTools),
      connectors: revision.requirements.connectors
    }
  };
  return [{ path: "SKILL.md", content: instructions }, { path: "general-agent.json", content: `${JSON.stringify(descriptor, null, 2)}\n` }];
}

function splitValues(value: string): string[] { return value.split(",").map((item) => item.trim()).filter(Boolean); }
function errorMessage(error: unknown, fallback: string): string { return error instanceof Error && error.message ? error.message : fallback; }
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null; }
function numberValue(value: unknown): number | null { return typeof value === "number" ? value : null; }
