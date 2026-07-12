import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  OwnerSkillApproval,
  OwnerSkillPackage,
  OwnerSkillPackageSummary,
  OwnerSkillRevision,
  OwnerSkillValidation
} from "../api";
import { ownerClient } from "../ownerClient";
import { getApproverPolicy, OwnerPolicy } from "../ownerBridge";
import { DraftForm } from "../components/ownerSkills/CreateDraftDialog";
import { OwnerApprovalOperation } from "../components/ownerSkills/SkillApprovalDialog";

export type PendingApproval = {
  approval: OwnerSkillApproval;
  operation: OwnerApprovalOperation;
  revision: OwnerSkillRevision;
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
  const [approver, setApprover] = useState<OwnerPolicy | null>(null);
  const [approverError, setApproverError] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [createdPackageId, setCreatedPackageId] = useState<string | null>(null);
  const [draftInstructions, setDraftInstructions] = useState("");
  const [draftRequiredTools, setDraftRequiredTools] = useState("");
  const [draftValidation, setDraftValidation] = useState<OwnerSkillValidation>(pendingValidation);
  const inventoryRequest = useRef(0);
  const detailRequest = useRef(0);
  const selected = selectedId ? details[selectedId] ?? null : null;
  const editableDraft = selected?.editable_draft ?? null;
  const selectedRevision = editableDraft
    ?? selected?.revisions.find((revision) => revision.revision_id === selected.active_revision_id)
    ?? selected?.revisions[0]
    ?? null;

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
      const next = normalizeInventory([...inventory.effective, ...inventory.managed]);
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
    getApproverPolicy()
      .then((value) => { setApprover(value); setApproverError(null); })
      .catch((error) => setApproverError(errorMessage(error, "Independent approver unavailable")));
  }, []);
  useEffect(() => {
    if (!selectedId) return;
    void loadDetail(selectedId);
  }, [loadDetail, selectedId]);
  useEffect(() => {
    if (!editableDraft) {
      setDraftInstructions("");
      setDraftRequiredTools("");
      setDraftValidation(pendingValidation);
      return;
    }
    setDraftInstructions(editableDraft.instructions);
    setDraftRequiredTools(editableDraft.requirements.runtime_tools.join(", "));
    setDraftValidation(normalizeValidation(editableDraft.validation));
  }, [editableDraft?.revision_id, editableDraft?.instructions, editableDraft?.validation]);

  const mutate = async (name: string, operation: () => Promise<void>) => {
    if (busy) return;
    setBusy(name); setStatus(null);
    try { await operation(); } finally { setBusy(null); }
  };
  const reconcile = async (message: string) => {
    await loadInventory(true);
    setStatus(message);
  };
  const invalidateDraft = () => setDraftValidation(pendingValidation);

  return {
    policy, packages, selectedId, selected, selectedRevision, editableDraft, search, status,
    loadError, busy, pendingApproval, approvalError, approver, approverError, createOpen,
    createdPackageId,
    draftInstructions, draftRequiredTools, draftValidation,
    setSearch, setCreateOpen,
    selectPackage: (item: OwnerSkillPackageSummary) => { setSelectedId(item.package_id); setStatus(null); },
    refresh: () => loadInventory(true),
    changeInstructions: (value: string) => { setDraftInstructions(value); invalidateDraft(); },
    changeRequiredTools: (value: string) => { setDraftRequiredTools(value); invalidateDraft(); },
    closeApproval: () => { if (busy !== "approval") setPendingApproval(null); },
    requestActivation: () => void mutate("activation", async () => {
      if (!editableDraft || !draftValidation.ok) return;
      try {
        const approval = await ownerClient.requestActivation(editableDraft.revision_id);
        setApprovalError(null);
        setPendingApproval({ approval, operation: "activation", revision: editableDraft });
      } catch (error) { setStatus(errorMessage(error, "Activation request failed")); }
    }),
    approvePending: () => void mutate("approval", async () => {
      if (!pendingApproval || !approver) return;
      try {
        const report = await ownerClient.resolveApproval(pendingApproval.approval.approval_id);
        const generation = report.active_generation ?? report.generation;
        setPendingApproval(null); setApprovalError(null);
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
          setPendingApproval({ approval: result as OwnerSkillApproval, operation: "rollback", revision });
        } else await reconcile(`Rolled back to ${revision.version}`);
      } catch { setStatus("Rollback failed. The current revision remains active."); }
    }),
    disable: () => void mutate("disable", async () => {
      if (!selected) return;
      try { await ownerClient.disable(selected.package_id); await reconcile("Skill disabled"); }
      catch (error) { setStatus(errorMessage(error, "Disable failed. The current revision remains active.")); }
    }),
    requestRemoval: () => void mutate("removal", async () => {
      if (!selected || !selectedRevision || !selectedRevision.validation.ok) return;
      try {
        const approval = await ownerClient.requestRemoval(selected.package_id);
        setPendingApproval({ approval, operation: "removal", revision: selectedRevision });
      } catch (error) { setStatus(errorMessage(error, "Removal request failed")); }
    }),
    saveDraft: () => void mutate("save", async () => {
      if (!selected || !editableDraft) return;
      try {
        await ownerClient.updateDraft(editableDraft.revision_id, draftFiles(selected, editableDraft, draftInstructions, draftRequiredTools));
        setDraftValidation(pendingValidation);
        await loadDetail(selected.package_id);
        setStatus("Draft saved");
      } catch (error) { setStatus(errorMessage(error, "Draft save failed")); }
    }),
    validateDraft: () => void mutate("validate", async () => {
      if (!selected || !editableDraft) return;
      try {
        await ownerClient.updateDraft(
          editableDraft.revision_id,
          draftFiles(selected, editableDraft, draftInstructions, draftRequiredTools)
        );
        const validation = normalizeValidation(await ownerClient.validateDraft(editableDraft.revision_id));
        setDraftValidation(validation);
        setDetails((current) => ({
          ...current,
          [selected.package_id]: mergeValidation(
            selected,
            editableDraft.revision_id,
            validation,
            draftInstructions,
            splitValues(draftRequiredTools)
          )
        }));
      } catch (error) {
        setDraftValidation({ ...draftValidation, ok: false, errors: [errorMessage(error, "Draft validation failed")] });
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

function normalizeInventory(items: OwnerSkillPackageSummary[]): OwnerSkillPackageSummary[] {
  const byId = new Map<string, OwnerSkillPackageSummary>();
  for (const item of items) {
    const current = byId.get(item.package_id);
    if (!current || item.source_layer === "managed") byId.set(item.package_id, item);
  }
  return [...byId.values()].sort((a, b) => a.package_id.localeCompare(b.package_id));
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
