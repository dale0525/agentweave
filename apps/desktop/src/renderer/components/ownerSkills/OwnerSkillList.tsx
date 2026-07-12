import {
  Badge,
  Box,
  Button,
  Flex,
  Heading,
  ScrollArea,
  Text,
  TextField
} from "@radix-ui/themes";
import { Plus, Search } from "lucide-react";

import { OwnerSkillPackage } from "../../api";

type OwnerSkillListProps = {
  canCreate: boolean;
  packages: OwnerSkillPackage[];
  search: string;
  selectedId: string | null;
  onCreate: () => void;
  onSearchChange: (value: string) => void;
  onSelect: (skillPackage: OwnerSkillPackage) => void;
};

export function OwnerSkillList({
  canCreate,
  packages,
  search,
  selectedId,
  onCreate,
  onSearchChange,
  onSelect
}: OwnerSkillListProps): JSX.Element {
  const normalizedSearch = search.trim().toLowerCase();
  const visiblePackages = packages.filter((skillPackage) =>
    `${skillPackage.display_name ?? ""} ${skillPackage.package_id}`
      .toLowerCase()
      .includes(normalizedSearch)
  );

  return (
    <Flex direction="column" height="100%" minHeight="0" gap="3" p="4">
      <Flex align="center" justify="between" gap="3" style={{ minHeight: 40 }}>
        <Box>
          <Heading as="h2" size="3">Packages</Heading>
          <Text color="gray" size="1">{packages.length} installed</Text>
        </Box>
        {canCreate ? (
          <Button onClick={onCreate} size="2" variant="solid">
            <Plus size={15} aria-hidden="true" /> New draft
          </Button>
        ) : null}
      </Flex>
      <TextField.Root
        aria-label="Search skills"
        onChange={(event) => onSearchChange(event.currentTarget.value)}
        placeholder="Search packages"
        size="2"
        value={search}
      >
        <TextField.Slot>
          <Search size={15} aria-hidden="true" />
        </TextField.Slot>
      </TextField.Root>
      <ScrollArea scrollbars="vertical" style={{ flex: 1, minHeight: 0 }}>
        <Flex aria-label="Skill packages" direction="column" role="list">
          {visiblePackages.map((skillPackage) => {
            const selected = skillPackage.package_id === selectedId;
            return (
              <button
                aria-pressed={selected}
                key={skillPackage.package_id}
                onClick={() => onSelect(skillPackage)}
                role="listitem"
                style={{
                  width: "100%",
                  minHeight: 68,
                  padding: "11px 12px",
                  border: 0,
                  borderBottom: "1px solid var(--gray-a5)",
                  borderLeft: selected
                    ? "2px solid var(--accent-9)"
                    : "2px solid transparent",
                  background: selected ? "var(--accent-a3)" : "transparent",
                  color: "inherit",
                  cursor: "pointer",
                  textAlign: "left"
                }}
                type="button"
              >
                <Flex align="start" justify="between" gap="2">
                  <Box style={{ minWidth: 0 }}>
                    <Text as="div" size="2" weight="medium" truncate>
                      {skillPackage.display_name ?? skillPackage.package_id}
                    </Text>
                    <Text as="div" color="gray" size="1" truncate>
                      {skillPackage.version || "No active revision"} {titleCase(skillPackage.status)}
                    </Text>
                  </Box>
                  <Badge color={skillPackage.source_layer === "managed" ? "teal" : "gray"}>
                    {skillPackage.source_layer === "managed" ? "Managed" : "Built-in"}
                  </Badge>
                </Flex>
              </button>
            );
          })}
          {visiblePackages.length === 0 ? (
            <Box p="3"><Text color="gray" size="2">No matching packages</Text></Box>
          ) : null}
        </Flex>
      </ScrollArea>
    </Flex>
  );
}

function titleCase(value: string): string {
  return value.replaceAll("_", " ").replace(/^./, (letter) => letter.toUpperCase());
}
