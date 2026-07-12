import * as Dialog from "@radix-ui/react-dialog";
import {
  Badge,
  Box,
  Button,
  Flex,
  Heading,
  Text,
  TextArea,
  TextField,
  Theme
} from "@radix-ui/themes";
import { ArrowLeft, LoaderCircle, RefreshCw, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import {
  OwnerSkillApproval,
  OwnerSkillAuditRecord,
  OwnerSkillDraftSummary,
  OwnerSkillInventory,
  OwnerSkillMutationReport,
  OwnerSkillPackage,
  OwnerSkillRevision,
  OwnerSkillValidation
} from "../api";
import { AppIconButton } from "../components/AppIconButton";
import { OwnerSkillDetail } from "../components/ownerSkills/OwnerSkillDetail";
import { OwnerSkillList } from "../components/ownerSkills/OwnerSkillList";
import {
  OwnerApprovalOperation,
  SkillApprovalDialog
} from "../components/ownerSkills/SkillApprovalDialog";
import { OwnerPolicy, canManageOwnerSkills, ownerRequest } from "../ownerBridge";

type OwnerSkillsProps = {
  onBack: () => void;
  policy: OwnerPolicy;
};

type PendingApproval = {
  approval: OwnerSkillApproval;
  operation: OwnerApprovalOperation;
  revision: OwnerSkillRevision;
};

const pendingValidation: OwnerSkillValidation = {
  ok: false,
  errors: ["Validation has not run"],
  warnings: []
};

export function OwnerSkills({ onBack, policy }: OwnerSkillsProps): JSX.Element {
  const [packages, setPackages] = useState<OwnerSkillPackage[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null);
  const [approvalError, setApprovalError] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [draftInstructions, setDraftInstructions] = useState("");
  const [draftRequiredTools, setDraftRequiredTools] = useState("");
  const [draftValidation, setDraftValidation] = useState<OwnerSkillValidation>(pendingValidation);
  const [mobileDetail, setMobileDetail] = useState(false);
  const isMobile = useIsMobile();

  const selected = packages.find((skillPackage) => skillPackage.package_id === selectedId) ?? null;
  const selectedRevision = selected ? getSelectedRevision(selected) : null;

  const loadInventory = async (retainExisting = false): Promise<void> => {
    if (!retainExisting) setLoadError(null);
    try {
      const inventory = await ownerRequest<OwnerSkillInventory>("/owner/skills", { method: "GET" });
      const nextPackages = normalizeInventory(inventory);
      setPackages(nextPackages);
      setSelectedId((current) =>
        current && nextPackages.some((item) => item.package_id === current)
          ? current
          : nextPackages[0]?.package_id ?? null
      );
      setLoadError(null);
    } catch (error) {
      setLoadError(errorMessage(error, "Unable to load owner skill inventory"));
    }
  };

  useEffect(() => {
    void loadInventory();
  }, []);

  useEffect(() => {
    if (!selectedRevision) return;
    setDraftInstructions(selectedRevision.instructions);
    setDraftRequiredTools(selectedRevision.required_tools.join(", "));
    setDraftValidation(selectedRevision.validation);
  }, [selectedId]);

  useEffect(() => {
    if (
      !selected ||
      selected.source_layer !== "managed" ||
      (selected.revisions?.length ?? 0) > 1 ||
      selected.revisions?.[0]?.created_by !== "Unknown"
    ) {
      return;
    }
    let active = true;
    ownerRequest<OwnerSkillAuditRecord[]>(
      `/owner/skills/${encodeURIComponent(selected.package_id)}/audit`,
      { method: "GET" }
    )
      .then((audit) => {
        if (!active) return;
        const revisions = revisionsFromAudit(selected, audit);
        if (revisions.length > 1) {
          updatePackage(selected.package_id, { revisions });
        }
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, [selectedId, selected?.revisions?.length]);

  const selectPackage = (skillPackage: OwnerSkillPackage) => {
    setSelectedId(skillPackage.package_id);
    if (isMobile) setMobileDetail(true);
    setStatus(null);
  };

  const mutate = async (name: string, operation: () => Promise<void>): Promise<void> => {
    if (busy) return;
    setBusy(name);
    setStatus(null);
    try {
      await operation();
    } finally {
      setBusy(null);
    }
  };

  const requestActivation = () => {
    if (!selected || !selectedRevision || !draftValidation.ok) return;
    void mutate("activation", async () => {
      try {
        const approval = await ownerRequest<OwnerSkillApproval>(
          `/owner/skills/drafts/${encodeURIComponent(selectedRevision.revision_id)}/activation`,
          { body: JSON.stringify({}), method: "POST" }
        );
        setApprovalError(null);
        setPendingApproval({ approval, operation: "activation", revision: selectedRevision });
      } catch (error) {
        setStatus(errorMessage(error, "Activation request failed"));
      }
    });
  };

  const approvePending = () => {
    if (!pendingApproval) return;
    void mutate("approval", async () => {
      try {
        const report = await ownerRequest<OwnerSkillMutationReport>(
          `/owner/skills/approvals/${encodeURIComponent(pendingApproval.approval.approval_id)}`,
          { body: JSON.stringify({ decision: "approve" }), method: "POST" }
        );
        if (pendingApproval.operation === "removal") {
          setPackages((current) => current.filter(
            (item) => item.package_id !== pendingApproval.approval.package_id
          ));
          setSelectedId(null);
          setMobileDetail(false);
          setStatus("Skill removed");
        } else {
          const generation = report.active_generation ?? report.generation;
          setStatus(generation ? `Active snapshot ${generation}` : "Skill operation approved");
        }
        setPendingApproval(null);
        setApprovalError(null);
      } catch (error) {
        setApprovalError(errorMessage(error, "Approval failed"));
      }
    });
  };

  const rollback = (revision: OwnerSkillRevision) => {
    if (!selected) return;
    void mutate("rollback", async () => {
      try {
        const result = await ownerRequest<OwnerSkillMutationReport & Partial<OwnerSkillApproval>>(
          `/owner/skills/${encodeURIComponent(selected.package_id)}/rollback`,
          { body: JSON.stringify({ revision_id: revision.revision_id }), method: "POST" }
        );
        if (result.approval_id) {
          setPendingApproval({
            approval: result as OwnerSkillApproval,
            operation: "rollback",
            revision
          });
          return;
        }
        updatePackage(selected.package_id, {
          active_revision_id: revision.revision_id,
          version: revision.version,
          status: "active"
        });
        setStatus(`Rolled back to ${revision.version}`);
      } catch {
        setStatus("Rollback failed. The current revision remains active.");
      }
    });
  };

  const disable = () => {
    if (!selected) return;
    void mutate("disable", async () => {
      try {
        await ownerRequest(`/owner/skills/${encodeURIComponent(selected.package_id)}/disable`, {
          body: JSON.stringify({}),
          method: "POST"
        });
        updatePackage(selected.package_id, { status: "disabled" });
        setStatus("Skill disabled");
      } catch (error) {
        setStatus(errorMessage(error, "Disable failed. The current revision remains active."));
      }
    });
  };

  const requestRemoval = () => {
    if (!selected || !selectedRevision || !draftValidation.ok) return;
    void mutate("removal", async () => {
      try {
        const approval = await ownerRequest<OwnerSkillApproval>(
          `/owner/skills/${encodeURIComponent(selected.package_id)}`,
          { method: "DELETE" }
        );
        setPendingApproval({ approval, operation: "removal", revision: selectedRevision });
      } catch (error) {
        setStatus(errorMessage(error, "Removal request failed"));
      }
    });
  };

  const saveDraft = () => {
    if (!selected || !selectedRevision) return;
    void mutate("save", async () => {
      try {
        await ownerRequest<OwnerSkillDraftSummary>(
          `/owner/skills/drafts/${encodeURIComponent(selectedRevision.revision_id)}`,
          {
            body: JSON.stringify({ files: draftFiles(selected, selectedRevision, draftInstructions, draftRequiredTools) }),
            method: "PUT"
          }
        );
        setDraftValidation(pendingValidation);
        updateRevision(selected.package_id, selectedRevision.revision_id, {
          instructions: draftInstructions,
          required_tools: splitValues(draftRequiredTools),
          validation: pendingValidation
        });
        setStatus("Draft saved");
      } catch (error) {
        setStatus(errorMessage(error, "Draft save failed"));
      }
    });
  };

  const validateDraft = () => {
    if (!selected || !selectedRevision) return;
    void mutate("validate", async () => {
      try {
        const validation = await ownerRequest<OwnerSkillValidation>(
          `/owner/skills/drafts/${encodeURIComponent(selectedRevision.revision_id)}/validate`,
          { body: JSON.stringify({}), method: "POST" }
        );
        const normalized = normalizeValidation(validation);
        setDraftValidation(normalized);
        updateRevision(selected.package_id, selectedRevision.revision_id, { validation: normalized });
      } catch (error) {
        const retained = {
          ...draftValidation,
          ok: false,
          errors: [errorMessage(error, "Draft validation failed")]
        };
        setDraftValidation(retained);
      }
    });
  };

  const updatePackage = (packageId: string, values: Partial<OwnerSkillPackage>) => {
    setPackages((current) => current.map((item) =>
      item.package_id === packageId ? { ...item, ...values } : item
    ));
  };

  const updateRevision = (
    packageId: string,
    revisionId: string,
    values: Partial<OwnerSkillRevision>
  ) => {
    setPackages((current) => current.map((item) => item.package_id === packageId
      ? {
          ...item,
          revisions: item.revisions?.map((revision) =>
            revision.revision_id === revisionId ? { ...revision, ...values } : revision
          )
        }
      : item));
  };

  const showList = !isMobile || !mobileDetail;
  const showDetail = !isMobile || mobileDetail;
  const back = isMobile && mobileDetail ? () => setMobileDetail(false) : onBack;

  return (
    <main
      aria-label="Owner Skills"
      style={{ display: "flex", height: "100%", minHeight: 0, flexDirection: "column", background: "var(--color-background)" }}
    >
      <header className="top-bar" style={{ justifyContent: "space-between" }}>
        <AppIconButton label={isMobile && mobileDetail ? "Back to skills list" : "Back to settings"} onClick={back}>
          <ArrowLeft size={18} aria-hidden="true" />
        </AppIconButton>
        <div className="top-bar-title" style={{ marginRight: "auto", textAlign: "left" }}>
          <h1>Owner Skills</h1>
          <p>{policy.mode.replaceAll("_", " ")} · {policy.actorId}</p>
        </div>
        <AppIconButton label="Refresh skills" onClick={() => void loadInventory(true)}>
          <RefreshCw size={17} aria-hidden="true" />
        </AppIconButton>
      </header>
      {status ? (
        <Box px="4" py="2" style={{ borderBottom: "1px solid var(--gray-a5)", background: "var(--accent-a2)" }}>
          <Text size="2">{status}</Text>
        </Box>
      ) : null}
      <Box
        style={{
          display: "grid",
          flex: 1,
          minHeight: 0,
          gridTemplateColumns: isMobile ? "minmax(0, 1fr)" : "320px minmax(0, 1fr)",
          background: "var(--gray-a2)"
        }}
      >
        {showList ? (
          <Box style={{ minHeight: 0, borderRight: isMobile ? 0 : "1px solid var(--gray-a5)", background: "var(--color-panel-solid)" }}>
            <OwnerSkillList
              canCreate={canManageOwnerSkills(policy, "create_draft")}
              onCreate={() => setCreateOpen(true)}
              onSearchChange={setSearch}
              onSelect={selectPackage}
              packages={packages}
              search={search}
              selectedId={selectedId}
            />
          </Box>
        ) : null}
        {showDetail ? (
          <Box style={{ minHeight: 0, minWidth: 0 }}>
            {selected && selectedRevision ? (
              <OwnerSkillDetail
                busy={busy !== null}
                draftInstructions={draftInstructions}
                draftRequiredTools={draftRequiredTools}
                draftValidation={draftValidation}
                isMobile={isMobile}
                onActivate={requestActivation}
                onDisable={disable}
                onDraftInstructionsChange={setDraftInstructions}
                onDraftRequiredToolsChange={setDraftRequiredTools}
                onRemove={requestRemoval}
                onRollback={rollback}
                onSaveDraft={saveDraft}
                onValidateDraft={validateDraft}
                ownerPolicy={policy}
                selected={selected}
                selectedRevision={selectedRevision}
              />
            ) : (
              <Flex align="center" height="100%" justify="center" p="5">
                <Text color={loadError ? "red" : "gray"} size="2">
                  {loadError ?? (packages.length === 0 ? "No skill packages" : "Select a package")}
                </Text>
              </Flex>
            )}
          </Box>
        ) : null}
      </Box>
      <CreateDraftDialog
        busy={busy !== null}
        onCreate={(form) => void mutate("create", async () => {
          try {
            const draft = await ownerRequest<OwnerSkillDraftSummary>("/owner/skills/drafts", {
              body: JSON.stringify({
                package_id: form.packageId,
                display_name: form.displayName,
                description: form.description,
                kind: form.kind,
                required_tools: splitValues(form.requiredTools)
              }),
              method: "POST"
            });
            const created = draftPackage(draft, form);
            setPackages((current) => [created, ...current]);
            setSelectedId(created.package_id);
            setDraftInstructions(`# ${form.displayName}\n\n${form.description}`);
            setDraftRequiredTools(form.requiredTools);
            setDraftValidation(pendingValidation);
            setCreateOpen(false);
            setMobileDetail(true);
          } catch (error) {
            setStatus(errorMessage(error, "Draft creation failed"));
          }
        })}
        onOpenChange={setCreateOpen}
        open={createOpen}
      />
      <SkillApprovalDialog
        approval={pendingApproval?.approval ?? null}
        busy={busy === "approval"}
        error={approvalError}
        onApprove={approvePending}
        onOpenChange={(open) => {
          if (!open && busy !== "approval") setPendingApproval(null);
        }}
        operation={pendingApproval?.operation ?? "activation"}
        revision={pendingApproval?.revision ?? null}
      />
    </main>
  );
}

type DraftForm = {
  packageId: string;
  displayName: string;
  description: string;
  kind: string;
  requiredTools: string;
};

function CreateDraftDialog({
  busy,
  open,
  onCreate,
  onOpenChange
}: {
  busy: boolean;
  open: boolean;
  onCreate: (form: DraftForm) => void;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const [form, setForm] = useState<DraftForm>({
    packageId: "",
    displayName: "",
    description: "",
    kind: "instruction_only",
    requiredTools: ""
  });
  const valid = form.packageId.trim() && form.displayName.trim() && form.description.trim();
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Theme
          accentColor="teal"
          appearance={preferredAppearance()}
          grayColor="gray"
          hasBackground={false}
          radius="small"
          scaling="100%"
        >
          <Dialog.Overlay style={{ position: "fixed", inset: 0, zIndex: 50, background: "rgba(17,24,39,.48)" }} />
          <Dialog.Content
          aria-label="Create skill draft"
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            zIndex: 51,
            width: "min(560px, calc(100vw - 32px))",
            transform: "translate(-50%, -50%)",
            border: "1px solid var(--gray-a6)",
            borderRadius: "var(--radius-3)",
            background: "var(--color-panel-solid)",
            padding: 20,
            boxShadow: "var(--shadow-6)"
          }}
        >
          <Flex align="center" justify="between" mb="4">
            <Dialog.Title asChild><Heading as="h2" size="4">New skill draft</Heading></Dialog.Title>
            <Dialog.Close asChild><Button aria-label="Close draft form" variant="ghost"><X size={17} /></Button></Dialog.Close>
          </Flex>
          <Flex direction="column" gap="3">
            <Field label="Package ID"><TextField.Root aria-label="Package ID" onChange={(event) => setForm({ ...form, packageId: event.currentTarget.value })} value={form.packageId} /></Field>
            <Field label="Display name"><TextField.Root aria-label="Display name" onChange={(event) => setForm({ ...form, displayName: event.currentTarget.value })} value={form.displayName} /></Field>
            <Field label="Description"><TextArea aria-label="Description" onChange={(event) => setForm({ ...form, description: event.currentTarget.value })} value={form.description} /></Field>
            <Field label="Package kind">
              <select
                aria-label="Package kind"
                onChange={(event) => setForm({ ...form, kind: event.currentTarget.value })}
                style={{
                  width: "100%",
                  minHeight: 38,
                  border: "1px solid var(--gray-a7)",
                  borderRadius: "var(--radius-2)",
                  background: "var(--color-panel-solid)",
                  color: "inherit",
                  padding: "0 10px"
                }}
                value={form.kind}
              >
                <option value="instruction_only">Instruction only</option>
                <option value="host_tools_only">Host tools only</option>
              </select>
            </Field>
            <Field label="Required host tools"><TextField.Root aria-label="Draft required host tools" onChange={(event) => setForm({ ...form, requiredTools: event.currentTarget.value })} value={form.requiredTools} /></Field>
          </Flex>
          <Flex justify="end" gap="2" mt="5">
            <Dialog.Close asChild><Button disabled={busy} variant="soft">Cancel</Button></Dialog.Close>
            <Button disabled={busy || !valid} onClick={() => onCreate(form)}>
              {busy ? <LoaderCircle size={15} aria-hidden="true" /> : null} Create draft
            </Button>
          </Flex>
          </Dialog.Content>
        </Theme>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Field({ children, label }: { children: React.ReactNode; label: string }): JSX.Element {
  return <label><Text as="div" mb="1" size="2" weight="medium">{label}</Text>{children}</label>;
}

function useIsMobile(): boolean {
  const [mobile, setMobile] = useState(() => typeof window !== "undefined" && window.innerWidth <= 760);
  useEffect(() => {
    const update = () => setMobile(window.innerWidth <= 760);
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  return mobile;
}

function normalizeInventory(inventory: OwnerSkillInventory): OwnerSkillPackage[] {
  const byId = new Map<string, OwnerSkillPackage>();
  for (const item of [...(inventory.effective ?? []), ...(inventory.managed ?? [])]) {
    const current = byId.get(item.package_id);
    byId.set(item.package_id, normalizePackage(current ? { ...current, ...item } : item));
  }
  return [...byId.values()].sort((left, right) => left.package_id.localeCompare(right.package_id));
}

function normalizePackage(item: OwnerSkillPackage): OwnerSkillPackage {
  const validation = normalizeValidation(item.validation ?? {
    ok: item.status === "active",
    errors: item.status === "active" ? [] : ["Validation has not run"],
    warnings: []
  });
  const fallback = normalizeRevision({
    revision_id: item.active_revision_id ?? `draft-${item.package_id}`,
    version: item.version || "0.1.0",
    status: item.status,
    created_by: "Unknown",
    created_at: "",
    kind: item.kind ?? "instruction_only",
    instructions: item.instructions ?? "",
    validation,
    required_tools: item.requirements?.runtime_tools ?? [],
    required_capabilities: item.requirements?.capabilities ?? [],
    required_connectors: item.requirements?.connectors ?? [],
    dependencies: item.requirements?.packages ?? [],
    permission_diff: {}
  });
  return {
    ...item,
    display_name: item.display_name ?? displayName(item.package_id),
    kind: item.kind ?? fallback.kind,
    validation,
    revisions: item.revisions?.map(normalizeRevision) ?? [fallback]
  };
}

function normalizeRevision(revision: OwnerSkillRevision): OwnerSkillRevision {
  return {
    ...revision,
    validation: normalizeValidation(revision.validation),
    required_tools: revision.required_tools ?? [],
    required_capabilities: revision.required_capabilities ?? [],
    required_connectors: revision.required_connectors ?? [],
    dependencies: revision.dependencies ?? []
  };
}

function normalizeValidation(validation: OwnerSkillValidation): OwnerSkillValidation {
  return {
    ...validation,
    ok: Boolean(validation.ok),
    errors: validation.errors ?? [],
    warnings: validation.warnings ?? []
  };
}

function getSelectedRevision(selected: OwnerSkillPackage): OwnerSkillRevision {
  const revisions = selected.revisions ?? [];
  return revisions.find((revision) => revision.revision_id === selected.active_revision_id)
    ?? revisions[0]
    ?? normalizePackage(selected).revisions![0];
}

function revisionsFromAudit(
  selected: OwnerSkillPackage,
  audit: OwnerSkillAuditRecord[]
): OwnerSkillRevision[] {
  const active = getSelectedRevision(selected);
  const byId = new Map<string, OwnerSkillRevision>([[active.revision_id, active]]);
  for (const record of audit) {
    if (!record.revision_id || byId.has(record.revision_id)) continue;
    const metadataVersion = record.metadata_json.version;
    byId.set(record.revision_id, {
      ...active,
      revision_id: record.revision_id,
      version:
        typeof metadataVersion === "string"
          ? metadataVersion
          : `Revision ${record.revision_id.slice(0, 8)}`,
      status: record.operation.includes("rollback") ? "rollback" : "managed",
      created_by: record.actor_id,
      created_at: record.created_at,
      instructions: "",
      validation: {
        ok: record.result === "ok",
        errors: record.result === "ok" ? [] : ["Revision audit recorded an error"],
        warnings: []
      }
    });
  }
  return [...byId.values()];
}

function draftPackage(draft: OwnerSkillDraftSummary, form: DraftForm): OwnerSkillPackage {
  return normalizePackage({
    package_id: draft.package_id,
    display_name: form.displayName,
    version: draft.version,
    source_layer: "managed",
    status: "draft",
    reason: "",
    active_revision_id: draft.revision_id,
    kind: draft.kind,
    instructions: `# ${form.displayName}\n\n${form.description}`,
    validation: pendingValidation,
    requirements: {
      runtime_tools: splitValues(form.requiredTools),
      capabilities: [],
      connectors: [],
      packages: []
    },
    revisions: [{
      revision_id: draft.revision_id,
      version: draft.version,
      status: "draft",
      created_by: "Current actor",
      created_at: new Date().toISOString(),
      kind: draft.kind,
      instructions: `# ${form.displayName}\n\n${form.description}`,
      validation: pendingValidation,
      required_tools: splitValues(form.requiredTools),
      required_capabilities: [],
      permission_diff: {}
    }]
  });
}

function draftFiles(
  skillPackage: OwnerSkillPackage,
  revision: OwnerSkillRevision,
  instructions: string,
  requiredTools: string
): Array<{ path: string; content: string }> {
  const tools = splitValues(requiredTools);
  const descriptor = {
    schemaVersion: 1,
    id: skillPackage.package_id,
    version: revision.version,
    displayName: skillPackage.display_name ?? skillPackage.package_id,
    kind: revision.kind,
    package: { includeInstructions: true, includeRuntime: false },
    compatibility: { minimumRuntimeVersion: null, platforms: [] },
    requires: {
      packages: revision.dependencies ?? [],
      capabilities: revision.required_capabilities,
      runtimeTools: tools,
      connectors: revision.required_connectors ?? []
    }
  };
  return [
    { path: "SKILL.md", content: instructions },
    { path: "general-agent.json", content: `${JSON.stringify(descriptor, null, 2)}\n` }
  ];
}

function splitValues(value: string): string[] {
  return value.split(",").map((item) => item.trim()).filter(Boolean);
}

function displayName(packageId: string): string {
  const leaf = packageId.split(".").at(-1) ?? packageId;
  return leaf.replaceAll("-", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

function preferredAppearance(): "dark" | "light" {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
