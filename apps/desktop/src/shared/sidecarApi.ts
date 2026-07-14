export const SIDECAR_API_REQUEST_CHANNEL = "agentweave:sidecar-api:request";

export type SidecarApiOperation =
  | "actions.list"
  | "actions.resolve"
  | "devSkills.delete"
  | "devSkills.list"
  | "devSkills.reload"
  | "devSkills.validate"
  | "mail.connect"
  | "mail.disconnect"
  | "mail.list"
  | "mail.status"
  | "memory.export"
  | "memory.forget"
  | "memory.get"
  | "memory.list"
  | "sessions.create"
  | "sessions.delete"
  | "sessions.list"
  | "sessions.load"
  | "sessions.update"
  | "tasks.create"
  | "tasks.delete"
  | "tasks.get"
  | "tasks.list"
  | "tasks.setStatus"
  | "tasks.update"
  | "turns.cancel"
  | "turns.events";

export type FoundationTaskStatus = "open" | "completed" | "cancelled";

export type FoundationTaskPriority = "low" | "normal" | "high" | "urgent";

export type FoundationTaskContent = Readonly<{
  title: string;
  notes?: string | null;
  dueAt?: string | null;
  timezone?: string | null;
  recurrence?: string | null;
  priority: FoundationTaskPriority;
  tags: readonly string[];
}>;

export type FoundationTaskRecord = Readonly<{
  id: string;
  content: FoundationTaskContent;
  status: FoundationTaskStatus;
  version: number;
  createdAt: string;
  updatedAt: string;
  completedAt?: string | null;
}>;

export type FoundationTaskPage = Readonly<{
  tasks: FoundationTaskRecord[];
  nextCursor: string | null;
}>;

export type FoundationTaskListInput = Readonly<{
  status?: FoundationTaskStatus;
  dueAfter?: string;
  dueBefore?: string;
  tag?: string;
  text?: string;
  limit?: number;
  cursor?: string;
}>;

export type SidecarApiRequest = Readonly<{
  input?: unknown;
  operation: SidecarApiOperation;
}>;
