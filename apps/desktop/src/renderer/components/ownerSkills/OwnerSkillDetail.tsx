import * as Tabs from "@radix-ui/react-tabs";
import { useEffect, useState } from "react";
import {
  Badge,
  Box,
  Button,
  Flex,
  Grid,
  Heading,
  ScrollArea,
  Separator,
  Text
} from "@radix-ui/themes";
import { Power, RotateCcw, Trash2 } from "lucide-react";

import { OwnerSkillPackage, OwnerSkillRevision, OwnerSkillValidation } from "../../api";
import { RevisionHistory } from "./RevisionHistory";
import { SkillDraftEditor } from "./SkillDraftEditor";

type OwnerSkillDetailProps = {
  busy: boolean;
  draftInstructions: string;
  draftRequiredTools: string;
  draftValidation: OwnerSkillValidation;
  isMobile: boolean;
  selected: OwnerSkillPackage;
  selectedRevision: OwnerSkillRevision;
  onActivate: () => void;
  onDisable: () => void;
  onDraftInstructionsChange: (value: string) => void;
  onDraftRequiredToolsChange: (value: string) => void;
  onRemove: () => void;
  onRollback: (revision: OwnerSkillRevision) => void;
  onSaveDraft: () => void;
  onValidateDraft: () => void;
};

export function OwnerSkillDetail({
  busy,
  draftInstructions,
  draftRequiredTools,
  draftValidation,
  isMobile,
  selected,
  selectedRevision,
  onActivate,
  onDisable,
  onDraftInstructionsChange,
  onDraftRequiredToolsChange,
  onRemove,
  onRollback,
  onSaveDraft,
  onValidateDraft
}: OwnerSkillDetailProps): JSX.Element {
  const managed = selected.managed !== null;
  const revisions = selected.revisions;
  const hasDraft = selected.editable_draft !== null;
  const validated = hasDraft ? draftValidation.ok : selectedRevision.validation.ok;
  const [tab, setTab] = useState(selected.status === "draft" && hasDraft ? "draft" : "overview");
  useEffect(() => {
    setTab(selected.status === "draft" && hasDraft ? "draft" : "overview");
  }, [selected.package_id]);
  useEffect(() => {
    if (tab === "draft" && !hasDraft) setTab("overview");
  }, [hasDraft, tab]);
  const canDisable = selected.actions.can_disable;
  const canRemove = selected.actions.can_request_removal;
  const rollbackRevision = revisions.find(
    (revision) => !revision.editable
      && revision.status === "managed"
      && revision.revision_id !== selected.active_revision_id
      && revision.validation.ok
  );

  return (
    <Flex direction="column" height="100%" minHeight="0">
      <ScrollArea scrollbars="vertical" style={{ flex: 1, minHeight: 0 }}>
      <Flex direction="column" gap="5" p={{ initial: "4", md: "5" }} pb="8">
        <Box>
          <Text color="gray" size="1" style={{ fontFamily: "var(--font-mono)", overflowWrap: "anywhere" }}>
            {selected.package_id}
          </Text>
          <Flex align="start" justify="between" gap="4" mt="2" wrap="wrap">
            <Box>
              <Heading as="h2" size="7">{selected.display_name ?? selected.package_id}</Heading>
              <Flex gap="2" mt="2" wrap="wrap">
                <Badge>{selected.version || selectedRevision.version}</Badge>
                <Badge color={selected.source_layer === "managed" ? "teal" : "gray"}>
                  {selected.source_layer === "managed" ? "Managed" : "Built-in"}
                </Badge>
                <Badge color={selected.status === "active" ? "teal" : "amber"}>{selected.status}</Badge>
                {selected.built_in_collision ? <Badge color="amber">Collision</Badge> : null}
                {selected.managed ? <Badge color="gray">Managed {selected.managed.status}</Badge> : null}
              </Flex>
            </Box>
            {!isMobile ? <Flex gap="2" wrap="wrap">
              {selected.actions.can_rollback && rollbackRevision ? (
                <Button disabled={busy} onClick={() => onRollback(rollbackRevision)} variant="soft">
                  <RotateCcw size={15} aria-hidden="true" /> Rollback to {rollbackRevision.version}
                </Button>
              ) : null}
            </Flex> : null}
          </Flex>
        </Box>

        <Box
          style={{
            border: `1px solid ${validated ? "var(--accent-a6)" : "var(--red-a6)"}`,
            borderRadius: "var(--radius-2)",
            padding: 14
          }}
        >
          <Flex
            align={isMobile ? "stretch" : "center"}
            direction={isMobile ? "column" : "row"}
            justify="between"
            gap="3"
          >
            <Box>
              <Text as="div" weight="medium">Validation</Text>
              <Text color="gray" size="2">
                {validated ? "Latest revision passed validation" : "Validation is required before activation or removal"}
              </Text>
            </Box>
            <Badge color={validated ? "teal" : "red"}>{validated ? "Passed" : "Required"}</Badge>
          </Flex>
        </Box>

        <Tabs.Root
          onValueChange={setTab}
          style={{ minWidth: 0, maxWidth: "100%" }}
          value={tab}
        >
          <Tabs.List
            aria-label="Skill details"
            className="owner-tabs-list"
            style={{
              display: "flex",
              gap: 4,
              width: "100%",
              maxWidth: isMobile ? "calc(100vw - 32px)" : "100%",
              minHeight: 40,
              overflowX: "auto",
              borderBottom: "1px solid var(--gray-a6)"
            }}
          >
            {[
              ["overview", "Overview"],
              ["revisions", "Revisions"],
              ...(managed && hasDraft ? [["draft", "Draft"]] : []),
              ["requirements", "Requirements"]
            ].map(([value, label]) => (
              <Tabs.Trigger
                className="owner-tab-trigger"
                key={value}
                value={value}
                style={{
                  minWidth: isMobile ? 0 : 88,
                  minHeight: 40,
                  padding: isMobile ? "0 6px" : "0 12px",
                  border: 0,
                  borderBottom: undefined,
                  background: "transparent",
                  color: "inherit",
                  cursor: "pointer",
                  flex: isMobile ? "1 1 auto" : "0 0 auto",
                  whiteSpace: "nowrap"
                }}
              >{label}</Tabs.Trigger>
            ))}
          </Tabs.List>
          <Tabs.Content style={{ minWidth: 0 }} value="overview">
            <Grid columns={{ initial: "1", md: "2" }} gap="3" pt="4" style={{ minWidth: 0 }}>
              <Fact label="Package kind" value={selectedRevision.kind} />
              <Fact label="Effective source" value={selected.effective?.source_layer ?? selected.source_layer} />
              <Fact label="Effective revision" value={selected.effective?.active_revision_id ?? selected.active_revision_id ?? "None"} />
              <Fact label="Managed status" value={selected.managed?.status ?? "Not installed"} />
              <Fact label="Managed revision" value={selected.managed?.active_revision_id ?? "None"} />
              <Fact label="Validation" value={validated ? "Passed" : "Required"} />
            </Grid>
          </Tabs.Content>
          <Tabs.Content style={{ minWidth: 0 }} value="revisions">
            <Box pt="4">
              <RevisionHistory
                activeRevisionId={selected.active_revision_id}
                busy={busy}
                canRollback={selected.actions.can_rollback}
                onRollback={onRollback}
                revisions={revisions}
                isMobile={isMobile}
              />
            </Box>
          </Tabs.Content>
          {managed && hasDraft ? (
            <Tabs.Content style={{ minWidth: 0 }} value="draft">
              <Box pt="4">
                <SkillDraftEditor
                  busy={busy}
                  canActivate={selected.actions.can_request_activation}
                  canEdit={selected.actions.can_edit_draft}
                  canValidate={selected.actions.can_validate_draft}
                  instructions={draftInstructions}
                  onActivate={onActivate}
                  onInstructionsChange={onDraftInstructionsChange}
                  onRequiredToolsChange={onDraftRequiredToolsChange}
                  onSave={onSaveDraft}
                  onValidate={onValidateDraft}
                  requiredTools={draftRequiredTools}
                  validation={draftValidation}
                />
              </Box>
            </Tabs.Content>
          ) : null}
          <Tabs.Content style={{ minWidth: 0 }} value="requirements">
            <Grid columns={{ initial: "1", md: "2" }} gap="3" pt="4" style={{ minWidth: 0 }}>
              <Requirement label="Runtime tools" values={selectedRevision.requirements.runtime_tools} />
              <Requirement label="Capabilities" values={selectedRevision.requirements.capabilities} />
              <Requirement label="Connectors" values={selectedRevision.requirements.connectors} />
              <Requirement label="Dependencies" values={selectedRevision.requirements.packages} />
            </Grid>
          </Tabs.Content>
        </Tabs.Root>

        {canDisable || canRemove ? (
          <Box>
            <Separator size="4" mb="4" />
            <Flex align="center" justify="between" gap="4" wrap="wrap">
              <Box>
                <Heading as="h3" size="3">Lifecycle controls</Heading>
                <Text color="gray" size="2">Disable or remove this managed package.</Text>
              </Box>
              <Flex gap="2" wrap="wrap">
                {canDisable ? (
                  <Button color="red" disabled={busy} onClick={onDisable} variant="soft">
                    <Power size={15} aria-hidden="true" /> Disable skill
                  </Button>
                ) : null}
                {canRemove ? (
                  <Button color="red" disabled={busy || !validated} onClick={onRemove} variant="outline">
                    <Trash2 size={15} aria-hidden="true" /> Remove skill
                  </Button>
                ) : null}
              </Flex>
            </Flex>
          </Box>
        ) : null}
      </Flex>
      </ScrollArea>
      {isMobile && selected.actions.can_rollback && rollbackRevision ? (
        <Flex
          gap="2"
          p="3"
          style={{
            flex: "0 0 auto",
            borderTop: "1px solid var(--gray-a6)",
            background: "var(--color-panel-solid)"
          }}
        >
          {rollbackRevision ? (
            <Button disabled={busy} onClick={() => onRollback(rollbackRevision)} variant="soft" style={{ flex: 1 }}>
              <RotateCcw size={15} aria-hidden="true" /> Rollback
            </Button>
          ) : null}
        </Flex>
      ) : null}
    </Flex>
  );
}

function Fact({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <Box style={{ minWidth: 0, border: "1px solid var(--gray-a6)", borderRadius: "var(--radius-2)", padding: 14 }}>
      <Text as="div" color="gray" size="1">{label}</Text>
      <Text as="div" mt="1" size="2" weight="medium" style={{ overflowWrap: "anywhere" }}>{value}</Text>
    </Box>
  );
}

function Requirement({ label, values }: { label: string; values: string[] }): JSX.Element {
  return (
    <Box style={{ borderTop: "1px solid var(--gray-a6)", paddingTop: 12 }}>
      <Heading as="h3" mb="2" size="2">{label}</Heading>
      <Flex gap="2" wrap="wrap">
        {values.length > 0
          ? values.map((value) => <Badge key={value} style={{ maxWidth: "100%", overflowWrap: "anywhere", whiteSpace: "normal" }}>{value}</Badge>)
          : <Text color="gray" size="2">None</Text>}
      </Flex>
    </Box>
  );
}
