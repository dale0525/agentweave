import * as Dialog from "@radix-ui/react-dialog";
import { Badge, Box, Button, Flex, Heading, IconButton, ScrollArea, Separator, Text, Theme, Tooltip } from "@radix-ui/themes";
import { LoaderCircle, X } from "lucide-react";

import { OwnerSkillApproval, OwnerSkillRevision } from "../../api";

export type OwnerApprovalOperation = "activation" | "removal" | "rollback";

type SkillApprovalDialogProps = {
  approval: OwnerSkillApproval | null;
  approverActor: string | null;
  approverAvailable: boolean;
  busy: boolean;
  error: string | null;
  operation: OwnerApprovalOperation;
  revision: OwnerSkillRevision | null;
  onApprove: () => void;
  onOpenChange: (open: boolean) => void;
};

export function SkillApprovalDialog({
  approval,
  approverActor,
  approverAvailable,
  busy,
  error,
  operation,
  revision,
  onApprove,
  onOpenChange
}: SkillApprovalDialogProps): JSX.Element {
  const title = `Approve skill ${operation}`;
  const capabilities = getAddedCapabilities(approval?.permission_diff, revision);
  return (
    <Dialog.Root open={approval !== null} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Theme
          accentColor="teal"
          appearance={preferredAppearance()}
          grayColor="gray"
          hasBackground={false}
          radius="small"
          scaling="100%"
        >
          <Dialog.Overlay
          style={{ position: "fixed", inset: 0, zIndex: 50, background: "rgba(17, 24, 39, 0.48)" }}
          />
          <Dialog.Content
          aria-label={title}
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            zIndex: 51,
            width: "min(680px, calc(100vw - 32px))",
            maxHeight: "calc(100vh - 96px)",
            transform: "translate(-50%, -50%)",
            overflow: "hidden",
            border: "1px solid var(--gray-a6)",
            borderRadius: "var(--radius-3)",
            background: "var(--color-panel-solid)",
            boxShadow: "var(--shadow-6)"
          }}
        >
          <Flex align="start" justify="between" gap="3" p="4">
            <Box>
              <Dialog.Title asChild>
                <Heading as="h2" size="4">{title}</Heading>
              </Dialog.Title>
              <Dialog.Description asChild>
                <Text color="gray" size="2">
                  {approval?.package_id} {revision?.version ?? ""}
                </Text>
              </Dialog.Description>
              <Flex gap="2" mt="2">
                <Badge color="gray">{revision?.kind ?? "instruction_only"}</Badge>
                <Badge color={revision?.validation.ok ? "teal" : "red"}>
                  {revision?.validation.ok ? "Validated" : "Validation required"}
                </Badge>
              </Flex>
            </Box>
            <Tooltip content="Close approval"><Dialog.Close asChild><IconButton aria-label="Close approval" disabled={busy} size="1" variant="ghost"><X size={17} aria-hidden="true" /></IconButton></Dialog.Close></Tooltip>
          </Flex>
          <Separator size="4" />
          <ScrollArea scrollbars="vertical" style={{ maxHeight: "calc(100vh - 300px)" }}>
            <Flex direction="column" gap="4" p="4">
              <ApprovalSection title="Instruction diff">
                <Box
                  style={{
                    maxHeight: 180,
                    overflow: "auto",
                    padding: 12,
                    border: "1px solid var(--gray-a6)",
                    borderRadius: "var(--radius-2)",
                    background: "var(--gray-a2)",
                    fontFamily: "var(--font-mono)",
                    whiteSpace: "pre-wrap"
                  }}
                >
                  <DiffText value={revision?.instructions || "No instruction changes"} />
                </Box>
              </ApprovalSection>
              <ApprovalSection title="Required tools">
                <ValueList empty="No runtime tools" values={revision?.requirements.runtime_tools ?? []} />
              </ApprovalSection>
              <ApprovalSection title="Capability diff">
                {capabilities.length === 0 ? (
                  <Text color="gray" size="2">No new capabilities</Text>
                ) : <ValueList values={capabilities} />}
              </ApprovalSection>
              <ApprovalSection title="Connectors and dependencies">
                <ValueList
                  empty="No connector or dependency changes"
                  values={[
                    ...(revision?.requirements.connectors ?? []),
                    ...(revision?.requirements.packages ?? [])
                  ]}
                />
              </ApprovalSection>
              <Flex justify="between" gap="3" wrap="wrap">
                <Text color="gray" size="2">Requested by</Text>
                <Text size="2" weight="medium">{approval?.requested_by ?? "Unknown"}</Text>
              </Flex>
              <Flex justify="between" gap="3" wrap="wrap">
                <Text color="gray" size="2">Approving actor</Text>
                <Text size="2" weight="medium">{approverActor ?? "Independent approver unavailable"}</Text>
              </Flex>
              <Text color="gray" size="2">
                {operation === "removal"
                  ? "Approval removes the managed package from the active inventory."
                  : "Approval publishes a new immutable active snapshot."}
              </Text>
              {error ? <Text color="red" size="2">{error}</Text> : null}
            </Flex>
          </ScrollArea>
          <Separator size="4" />
          <Flex justify="end" gap="2" p="4">
            <Dialog.Close asChild>
              <Button disabled={busy} variant="soft">Cancel</Button>
            </Dialog.Close>
            <Button disabled={busy || !revision?.validation.ok || !approverAvailable} onClick={onApprove}>
              {busy ? <LoaderCircle size={15} aria-hidden="true" /> : null}
              {busy ? "Approving..." : `Approve ${operation}`}
            </Button>
          </Flex>
          </Dialog.Content>
        </Theme>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ApprovalSection({ children, title }: { children: React.ReactNode; title: string }): JSX.Element {
  return (
    <Box>
      <Heading as="h3" mb="2" size="2">{title}</Heading>
      {children}
    </Box>
  );
}

function ValueList({ empty, values }: { empty?: string; values: string[] }): JSX.Element {
  if (values.length === 0) return <Text color="gray" size="2">{empty}</Text>;
  return <Flex gap="2" wrap="wrap">{values.map((value) => <Badge key={value}>{value}</Badge>)}</Flex>;
}

function getAddedCapabilities(
  permissionDiff: unknown,
  revision: OwnerSkillRevision | null
): string[] {
  if (isRecord(permissionDiff) && isRecord(permissionDiff.capabilities)) {
    const added = permissionDiff.capabilities.added;
    if (Array.isArray(added)) return added.filter((value): value is string => typeof value === "string");
  }
  return revision?.requirements.capabilities ?? [];
}

function DiffText({ value }: { value: string }): JSX.Element {
  return <>{value.split("\n").map((line, index) => <Text as="div" color={line.startsWith("+") ? "green" : line.startsWith("-") ? "red" : undefined} key={`${index}-${line}`} size="1">{line || " "}</Text>)}</>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function preferredAppearance(): "dark" | "light" {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
