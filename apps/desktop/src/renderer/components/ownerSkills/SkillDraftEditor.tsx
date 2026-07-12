import { Badge, Box, Button, Flex, Heading, Text, TextArea, TextField } from "@radix-ui/themes";
import { CheckCircle2, Save, ShieldCheck } from "lucide-react";

import { OwnerSkillValidation } from "../../api";

type SkillDraftEditorProps = {
  busy: boolean;
  canActivate: boolean;
  canEdit: boolean;
  canValidate: boolean;
  instructions: string;
  requiredTools: string;
  validation: OwnerSkillValidation;
  onActivate: () => void;
  onInstructionsChange: (value: string) => void;
  onRequiredToolsChange: (value: string) => void;
  onSave: () => void;
  onValidate: () => void;
};

export function SkillDraftEditor({
  busy,
  canActivate,
  canEdit,
  canValidate,
  instructions,
  requiredTools,
  validation,
  onActivate,
  onInstructionsChange,
  onRequiredToolsChange,
  onSave,
  onValidate
}: SkillDraftEditorProps): JSX.Element {
  return (
    <Flex direction="column" gap="4">
      <label>
        <Text as="div" mb="2" size="2" weight="medium">Instructions</Text>
        <TextArea
          aria-label="Instructions"
          disabled={!canEdit || busy}
          onChange={(event) => onInstructionsChange(event.currentTarget.value)}
          resize="vertical"
          rows={12}
          style={{ fontFamily: "var(--font-mono)", minHeight: 220 }}
          value={instructions}
        />
      </label>
      <label>
        <Text as="div" mb="2" size="2" weight="medium">Required host tools</Text>
        <TextField.Root
          aria-label="Required host tools"
          disabled={!canEdit || busy}
          onChange={(event) => onRequiredToolsChange(event.currentTarget.value)}
          placeholder="tool.one, tool.two"
          value={requiredTools}
        />
      </label>
      <Box
        style={{
          borderTop: `1px solid ${validation.ok ? "var(--accent-a7)" : "var(--red-a7)"}`,
          borderRight: `1px solid ${validation.ok ? "var(--accent-a7)" : "var(--red-a7)"}`,
          borderBottom: `1px solid ${validation.ok ? "var(--accent-a7)" : "var(--red-a7)"}`,
          borderLeft: `3px solid ${validation.ok ? "var(--accent-a7)" : "var(--red-a7)"}`,
          borderRadius: "var(--radius-2)",
          padding: 14
        }}
      >
        <Flex align="center" gap="2" mb="2">
          {validation.ok ? <CheckCircle2 size={16} aria-hidden="true" /> : <ShieldCheck size={16} aria-hidden="true" />}
          <Heading as="h3" color={validation.ok ? "teal" : "red"} size="3">
            {validation.ok ? "Validation passed" : "Validation failed"}
          </Heading>
          <Badge color={validation.ok ? "teal" : "red"}>
            {validation.errors.length} {validation.errors.length === 1 ? "error" : "errors"}
          </Badge>
        </Flex>
        {validation.errors.length > 0 ? (
          <ul style={{ margin: 0, paddingLeft: 20 }}>
            {validation.errors.map((error) => <li key={error}><Text size="2">{error}</Text></li>)}
          </ul>
        ) : (
          <Text color="gray" size="2">The latest draft is ready for approval.</Text>
        )}
      </Box>
      <Flex gap="2" wrap="wrap">
        {canEdit ? (
          <Button disabled={busy} onClick={onSave} variant="soft">
            <Save size={15} aria-hidden="true" /> Save draft
          </Button>
        ) : null}
        {canValidate ? (
          <Button disabled={busy} onClick={onValidate}>
            <ShieldCheck size={15} aria-hidden="true" /> Validate draft
          </Button>
        ) : null}
        {canActivate ? (
          <Button disabled={busy || !validation.ok} onClick={onActivate} variant="solid">
            Request activation
          </Button>
        ) : null}
      </Flex>
    </Flex>
  );
}
