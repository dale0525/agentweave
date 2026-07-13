import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { Box, Flex, IconButton, Text, Tooltip } from "@radix-ui/themes";
import { ArrowLeft, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";

import { CreateDraftDialog } from "../components/ownerSkills/CreateDraftDialog";
import { OwnerSkillDetail } from "../components/ownerSkills/OwnerSkillDetail";
import { OwnerSkillList } from "../components/ownerSkills/OwnerSkillList";
import { SkillApprovalDialog } from "../components/ownerSkills/SkillApprovalDialog";
import { useOwnerSkillsWorkflow } from "../hooks/useOwnerSkillsWorkflow";
import { OwnerPolicy, canManageOwnerSkills } from "../ownerBridge";

type OwnerSkillsProps = { onBack: () => void; policy: OwnerPolicy };

export function OwnerSkills({ onBack, policy }: OwnerSkillsProps): JSX.Element {
  const workflow = useOwnerSkillsWorkflow(policy);
  const [mobileDetail, setMobileDetail] = useState(false);
  const isMobile = useIsMobile();
  const showList = !isMobile || !mobileDetail;
  const showDetail = !isMobile || mobileDetail;
  const back = isMobile && mobileDetail ? () => setMobileDetail(false) : onBack;
  useEffect(() => {
    if (isMobile && workflow.createdPackageId) setMobileDetail(true);
  }, [isMobile, workflow.createdPackageId]);
  const selectPackage = (item: Parameters<typeof workflow.selectPackage>[0]) => {
    workflow.selectPackage(item);
    if (isMobile) setMobileDetail(true);
  };

  return (
    <TooltipPrimitive.Provider delayDuration={350}>
    <main aria-label="Owner Skills" style={{ display: "flex", height: "100%", minHeight: 0, flexDirection: "column", background: "var(--color-background)" }}>
      <header className="top-bar" style={{ justifyContent: "space-between" }}>
        <Tooltip content={isMobile && mobileDetail ? "Back to skills list" : "Back to settings"}>
          <IconButton aria-label={isMobile && mobileDetail ? "Back to skills list" : "Back to settings"} onClick={back} size="2" variant="ghost">
            <ArrowLeft size={18} aria-hidden="true" />
          </IconButton>
        </Tooltip>
        <div className="top-bar-title" style={{ marginRight: "auto", textAlign: "left" }}>
          <h1>Owner Skills</h1>
          <p>{policy.mode.replaceAll("_", " ")} · {policy.actorId}</p>
        </div>
        <Tooltip content="Refresh skills">
          <IconButton aria-label="Refresh skills" disabled={workflow.busy !== null} onClick={() => void workflow.refresh()} size="2" variant="ghost">
            <RefreshCw size={17} aria-hidden="true" />
          </IconButton>
        </Tooltip>
      </header>
      {workflow.status || workflow.loadError ? (
        <Box aria-live="polite" px="4" py="2" style={{ borderBottom: "1px solid var(--gray-a5)", background: workflow.loadError ? "var(--red-a2)" : "var(--accent-a2)" }}>
          <Text color={workflow.loadError ? "red" : undefined} size="2">{workflow.loadError ?? workflow.status}</Text>
        </Box>
      ) : null}
      <Box style={{ display: "grid", flex: 1, minHeight: 0, gridTemplateColumns: isMobile ? "minmax(0, 1fr)" : "320px minmax(0, 1fr)", background: "var(--gray-a2)" }}>
        {showList ? (
          <Box style={{ minHeight: 0, borderRight: isMobile ? 0 : "1px solid var(--gray-a5)", background: "var(--color-panel-solid)" }}>
            <OwnerSkillList
              canCreate={canManageOwnerSkills(policy, "create_draft")}
              onCreate={() => workflow.setCreateOpen(true)}
              onSearchChange={workflow.setSearch}
              onSelect={selectPackage}
              packages={workflow.packages}
              search={workflow.search}
              selectedId={workflow.selectedId}
            />
          </Box>
        ) : null}
        {showDetail ? (
          <Box style={{ minHeight: 0, minWidth: 0 }}>
            {workflow.selected && workflow.selectedRevision ? (
              <OwnerSkillDetail
                busy={workflow.busy !== null}
                draftInstructions={workflow.draftInstructions}
                draftRequiredTools={workflow.draftRequiredTools}
                draftValidation={workflow.draftValidation}
                isMobile={isMobile}
                onActivate={workflow.requestActivation}
                onDisable={workflow.disable}
                onDraftInstructionsChange={workflow.changeInstructions}
                onDraftRequiredToolsChange={workflow.changeRequiredTools}
                onRemove={workflow.requestRemoval}
                onRollback={workflow.rollback}
                onSaveDraft={workflow.saveDraft}
                onValidateDraft={workflow.validateDraft}
                selected={workflow.selected}
                selectedRevision={workflow.selectedRevision}
              />
            ) : (
              <Flex align="center" height="100%" justify="center" p="5"><Text color="gray" size="2">{workflow.packages.length === 0 ? "No skill packages" : "Loading package detail"}</Text></Flex>
            )}
          </Box>
        ) : null}
      </Box>
      <CreateDraftDialog busy={workflow.busy !== null} onCreate={workflow.createDraft} onOpenChange={workflow.setCreateOpen} open={workflow.createOpen} />
      <SkillApprovalDialog
        approval={workflow.pendingApproval?.approval ?? null}
        baselineRevision={workflow.pendingApproval?.baselineRevision ?? null}
        busy={workflow.busy === "approval"}
        error={workflow.approvalError}
        onApprove={workflow.approvePending}
        onOpenChange={(open) => { if (!open) workflow.closeApproval(); }}
        operation={workflow.pendingApproval?.operation ?? "activation"}
        revision={workflow.pendingApproval?.revision ?? null}
      />
    </main>
    </TooltipPrimitive.Provider>
  );
}

function useIsMobile(): boolean {
  const [mobile, setMobile] = useState(() => window.innerWidth <= 760);
  useEffect(() => {
    const update = () => setMobile(window.innerWidth <= 760);
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  return mobile;
}
