import { Badge, Box, Button, Flex, Heading, IconButton, Separator, Text } from "@radix-ui/themes";
import { Check, LoaderCircle, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { OwnerSkillApproval, OwnerSkillPackage, OwnerSkillRevision } from "../api";

type ApprovalReview = {
  approval: OwnerSkillApproval & { operation: "activation" | "removal" | "rollback" };
  package: OwnerSkillPackage;
};

type ApprovalPrincipal = { actorId: string; role: string };

export function ApprovalSurface(): JSX.Element {
  const approvalId = useMemo(readApprovalId, []);
  const [review, setReview] = useState<ApprovalReview | null>(null);
  const [principal, setPrincipal] = useState<ApprovalPrincipal | null>(null);
  const [busy, setBusy] = useState<"approve" | "reject" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const api = window.agentWeaveApproval;
    if (!api || !approvalId) {
      setError("Independent approval service is unavailable");
      return;
    }
    let current = true;
    Promise.all([api.principal(), api.approval(approvalId)])
      .then(([actor, value]) => {
        if (!current) return;
        setPrincipal(requirePrincipal(actor));
        setReview(requireReview(value, approvalId));
      })
      .catch((reason) => { if (current) setError(errorMessage(reason)); });
    return () => { current = false; };
  }, [approvalId]);

  const decide = async (decision: "approve" | "reject") => {
    const api = window.agentWeaveApproval;
    if (!api || !approvalId || !review) return;
    setBusy(decision);
    setError(null);
    try {
      const resolution = await api.resolve(approvalId, decision);
      await api.complete({ approvalId, decision, resolution });
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(null);
    }
  };

  const closeObservation = async () => {
    const api = window.agentWeaveApproval;
    if (!api || !approvalId) return;
    setError(null);
    try {
      await api.close(approvalId);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };

  const target = review ? targetRevision(review) : null;
  const baseline = review ? baselineRevision(review) : null;
  const operation = review?.approval.operation ?? "activation";
  return (
    <main aria-label="Independent skill approval" style={{ minHeight: "100vh", background: "var(--color-background)" }}>
      <Box mx="auto" style={{ width: "min(100%, 760px)" }}>
        <Flex align="center" justify="between" px="5" py="4" wrap="wrap" gap="3">
          <Box>
            <Text color="gray" size="1" weight="bold">INDEPENDENT APPROVAL</Text>
            <Heading as="h1" mt="1" size="5">Review skill {operation}</Heading>
          </Box>
          <Flex align="center" gap="3">
            <Box style={{ textAlign: "right" }}>
              <Text as="div" color="gray" size="1">Authenticated approver</Text>
              <Text as="div" size="2" weight="medium">{principal?.actorId ?? "Loading"}</Text>
            </Box>
            <IconButton
              aria-label="Close approval window"
              color="gray"
              disabled={busy !== null}
              onClick={() => void closeObservation()}
              variant="ghost"
            >
              <X aria-hidden="true" size={17} />
            </IconButton>
          </Flex>
        </Flex>
        <Separator size="4" />
        {error ? <Box aria-live="assertive" p="5"><Text color="red" role="alert">{error}</Text></Box> : null}
        {!review && !error ? (
          <Flex align="center" gap="2" p="5"><LoaderCircle aria-hidden="true" size={17} /><Text color="gray">Loading approval</Text></Flex>
        ) : null}
        {review ? (
          <>
            <Flex direction="column" gap="5" p="5">
              <Flex align="start" justify="between" gap="4" wrap="wrap">
                <Box>
                  <Heading as="h2" size="4">{review.package.display_name}</Heading>
                  <Text color="gray" size="2">{review.approval.package_id}</Text>
                </Box>
                <Flex gap="2" wrap="wrap">
                  <Badge color="gray">{target?.version ?? "No revision"}</Badge>
                  <Badge color={target?.validation.ok ? "teal" : "red"}>
                    {target?.validation.ok ? "Validated" : "Validation required"}
                  </Badge>
                </Flex>
              </Flex>
              <ReviewRow label="Requested by" value={review.approval.requested_by} />
              <ReviewRow label="Revision" value={review.approval.revision_id} />
              <ReviewSection title="Instruction change">
                <InstructionChange before={baseline?.instructions ?? ""} after={target?.instructions ?? ""} />
              </ReviewSection>
              <ReviewSection title="Required tools">
                <BadgeList values={target?.requirements.runtime_tools ?? []} />
              </ReviewSection>
              <ReviewSection title="Capabilities">
                <BadgeList values={target?.requirements.capabilities ?? []} />
              </ReviewSection>
              <ReviewSection title="Connectors and packages">
                <BadgeList values={[
                  ...(target?.requirements.connectors ?? []),
                  ...(target?.requirements.packages ?? [])
                ]} />
              </ReviewSection>
            </Flex>
            <Separator size="4" />
            <Flex justify="end" gap="3" p="5">
              <Button color="red" disabled={busy !== null} onClick={() => void decide("reject")} variant="soft">
                {busy === "reject" ? <LoaderCircle aria-hidden="true" size={15} /> : <X aria-hidden="true" size={15} />}
                Reject
              </Button>
              <Button disabled={busy !== null || target?.validation.ok === false} onClick={() => void decide("approve")}>
                {busy === "approve" ? <LoaderCircle aria-hidden="true" size={15} /> : <Check aria-hidden="true" size={15} />}
                Approve
              </Button>
            </Flex>
          </>
        ) : null}
      </Box>
    </main>
  );
}

function ReviewRow({ label, value }: { label: string; value: string }): JSX.Element {
  return <Flex justify="between" gap="4" wrap="wrap"><Text color="gray" size="2">{label}</Text><Text size="2" weight="medium">{value}</Text></Flex>;
}

function ReviewSection({ children, title }: { children: React.ReactNode; title: string }): JSX.Element {
  return <Box><Heading as="h3" mb="2" size="2">{title}</Heading>{children}</Box>;
}

function BadgeList({ values }: { values: string[] }): JSX.Element {
  if (values.length === 0) return <Text color="gray" size="2">None</Text>;
  return <Flex gap="2" wrap="wrap">{values.map((value) => <Badge key={value}>{value}</Badge>)}</Flex>;
}

function InstructionChange({ after, before }: { after: string; before: string }): JSX.Element {
  return (
    <Box style={{ maxHeight: 220, overflow: "auto", border: "1px solid var(--gray-a6)", borderRadius: 6, background: "var(--gray-a2)", padding: 12, whiteSpace: "pre-wrap" }}>
      {before ? <Text as="div" color="red" size="1">- {before}</Text> : null}
      {after ? <Text as="div" color="green" size="1">+ {after}</Text> : <Text color="gray" size="1">No instruction content</Text>}
    </Box>
  );
}

function targetRevision(review: ApprovalReview): OwnerSkillRevision | null {
  return review.package.revisions.find((revision) => revision.revision_id === review.approval.revision_id) ?? null;
}

function baselineRevision(review: ApprovalReview): OwnerSkillRevision | null {
  return review.package.revisions.find((revision) => revision.revision_id === review.package.active_revision_id) ?? null;
}

function readApprovalId(): string | null {
  const value = new URLSearchParams(window.location.search).get("approvalId");
  return value && UUID_V4.test(value) ? value : null;
}

function requirePrincipal(value: unknown): ApprovalPrincipal {
  if (!isRecord(value) || typeof value.actorId !== "string" || typeof value.role !== "string") {
    throw new Error("Authenticated approver response is invalid");
  }
  return { actorId: value.actorId, role: value.role };
}

function requireReview(value: unknown, approvalId: string): ApprovalReview {
  if (!isRecord(value) || !isRecord(value.approval) || !isRecord(value.package)) {
    throw new Error("Approval review response is invalid");
  }
  const approval = value.approval;
  if (approval.approval_id !== approvalId || approval.status !== "pending") {
    throw new Error("Approval review response is not pending");
  }
  return value as ApprovalReview;
}

function errorMessage(value: unknown): string {
  return value instanceof Error ? value.message : "Approval failed";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
