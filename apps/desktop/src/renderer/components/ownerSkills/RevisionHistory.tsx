import { Badge, Box, Button, Flex, Table, Text } from "@radix-ui/themes";
import { RotateCcw } from "lucide-react";

import { OwnerSkillRevision } from "../../api";

type RevisionHistoryProps = {
  activeRevisionId: string | null;
  canRollback: boolean;
  busy: boolean;
  revisions: OwnerSkillRevision[];
  isMobile: boolean;
  onRollback: (revision: OwnerSkillRevision) => void;
};

export function RevisionHistory({
  activeRevisionId,
  canRollback,
  busy,
  revisions,
  isMobile,
  onRollback
}: RevisionHistoryProps): JSX.Element {
  if (revisions.length === 0) {
    return <Text color="gray" size="2">No revision history is available.</Text>;
  }

  if (isMobile) {
    return (
      <Flex aria-label="Revision history" direction="column" gap="2" role="list">
        {revisions.map((revision) => {
          const active = revision.revision_id === activeRevisionId;
          return (
            <Box key={revision.revision_id} role="listitem" style={{ borderBottom: "1px solid var(--gray-a6)", padding: "10px 0" }}>
              <Flex align="center" justify="between" gap="2">
                <Text weight="medium">{revision.version}{active ? " Active" : ""}</Text>
                <Badge color={active ? "teal" : "gray"}>{active ? "Active" : revision.status}</Badge>
              </Flex>
              <Text as="div" color="gray" size="1" mt="1" style={{ overflowWrap: "anywhere" }}>{revision.revision_id}</Text>
              <Flex align="center" justify="between" gap="2" mt="2">
                <Text color="gray" size="1">{revision.created_by || "Unknown"}</Text>
                {!active && canRollback ? <Button disabled={busy || !revision.validation.ok} onClick={() => onRollback(revision)} size="1" variant="soft"><RotateCcw size={13} aria-hidden="true" /> Rollback to {revision.version}</Button> : null}
              </Flex>
            </Box>
          );
        })}
      </Flex>
    );
  }

  return (
    <Box>
      <Table.Root aria-label="Revision history" variant="surface">
        <Table.Header>
          <Table.Row>
            <Table.ColumnHeaderCell>Revision</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>Status</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>Actor</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell justify="end">Action</Table.ColumnHeaderCell>
          </Table.Row>
        </Table.Header>
        <Table.Body>
          {revisions.map((revision) => {
            const active = revision.revision_id === activeRevisionId;
            return (
              <Table.Row key={revision.revision_id}>
                <Table.RowHeaderCell>
                  <Flex direction="column" gap="1">
                    <Text weight="medium">{revision.version}{active ? " Active" : ""}</Text>
                    <Text color="gray" size="1" style={{ fontFamily: "var(--font-mono)" }}>
                      {revision.revision_id}
                    </Text>
                  </Flex>
                </Table.RowHeaderCell>
                <Table.Cell>
                  <Badge color={active ? "teal" : "gray"}>{active ? "Active" : revision.status}</Badge>
                </Table.Cell>
                <Table.Cell>{revision.created_by || "Unknown"}</Table.Cell>
                <Table.Cell justify="end">
                  {!active && canRollback ? (
                    <Button
                      disabled={busy || !revision.validation.ok}
                      onClick={() => onRollback(revision)}
                      size="1"
                      variant="soft"
                    >
                      <RotateCcw size={13} aria-hidden="true" /> Rollback to {revision.version}
                    </Button>
                  ) : null}
                </Table.Cell>
              </Table.Row>
            );
          })}
        </Table.Body>
      </Table.Root>
    </Box>
  );
}
