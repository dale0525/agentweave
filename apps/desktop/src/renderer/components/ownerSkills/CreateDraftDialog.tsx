import * as Dialog from "@radix-ui/react-dialog";
import {
  Button,
  Flex,
  Heading,
  IconButton,
  Text,
  TextArea,
  TextField,
  Theme,
  Tooltip
} from "@radix-ui/themes";
import { LoaderCircle, X } from "lucide-react";
import { useState } from "react";

export type DraftForm = {
  packageId: string;
  displayName: string;
  description: string;
  kind: string;
  requiredTools: string;
};

export function CreateDraftDialog({
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
        <Theme accentColor="teal" appearance={preferredAppearance()} grayColor="gray" hasBackground={false} radius="small">
          <Dialog.Overlay style={{ position: "fixed", inset: 0, zIndex: 50, background: "rgba(17,24,39,.48)" }} />
          <Dialog.Content aria-label="Create skill draft" style={contentStyle}>
            <Flex align="center" justify="between" mb="4">
              <Dialog.Title asChild><Heading as="h2" size="4">New skill draft</Heading></Dialog.Title>
              <Tooltip content="Close draft form">
                <Dialog.Close asChild>
                  <IconButton aria-label="Close draft form" disabled={busy} variant="ghost"><X size={17} /></IconButton>
                </Dialog.Close>
              </Tooltip>
            </Flex>
            <Flex direction="column" gap="3">
              <Field label="Package ID"><TextField.Root aria-label="Package ID" onChange={(event) => setForm({ ...form, packageId: event.currentTarget.value })} value={form.packageId} /></Field>
              <Field label="Display name"><TextField.Root aria-label="Display name" onChange={(event) => setForm({ ...form, displayName: event.currentTarget.value })} value={form.displayName} /></Field>
              <Field label="Description"><TextArea aria-label="Description" onChange={(event) => setForm({ ...form, description: event.currentTarget.value })} value={form.description} /></Field>
              <Field label="Package kind">
                <select aria-label="Package kind" onChange={(event) => setForm({ ...form, kind: event.currentTarget.value })} style={selectStyle} value={form.kind}>
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

const contentStyle: React.CSSProperties = {
  position: "fixed", top: "50%", left: "50%", zIndex: 51,
  width: "min(560px, calc(100vw - 32px))", transform: "translate(-50%, -50%)",
  border: "1px solid var(--gray-a6)", borderRadius: "var(--radius-3)",
  background: "var(--color-panel-solid)", padding: 20, boxShadow: "var(--shadow-6)"
};

const selectStyle: React.CSSProperties = {
  width: "100%", minHeight: 38, border: "1px solid var(--gray-a7)",
  borderRadius: "var(--radius-2)", background: "var(--color-panel-solid)",
  color: "inherit", padding: "0 10px"
};

function preferredAppearance(): "dark" | "light" {
  const configured = document.documentElement.dataset.appearance;
  if (configured === "dark" || configured === "light") return configured;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
