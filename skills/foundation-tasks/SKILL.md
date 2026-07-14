---
name: foundation-tasks
description: Capture, inspect, prioritize, update, complete, cancel, or delete user tasks through a provider-neutral Task Provider, including due times and recurrence. Use for todo lists, follow-ups, deadlines, reminders linked to tasks, 待办、任务、截止日期、跟进、完成任务或重复任务.
---

# Foundation Tasks

Use the host Task Provider as the source of truth. Keep task state separate from scheduler triggers and notification delivery.

Read [references/contract.md](references/contract.md) before changing recurrence, completion, or deletion state.

## Follow the workflow

1. Search or read the current task before updating it.
2. Keep title, notes, priority, tags, due instant, source timezone, and recurrence explicit.
3. Use the stable task ID and expected version for every mutation.
4. Create with an idempotency key when the host supports retries.
5. Treat complete, cancel, and delete as different outcomes.
6. Report the final provider state and any scheduler or notification work still pending.

## Respect boundaries

- This skill does not own credentials, authorization, storage, scheduling, notifications, or audit.
- A due date does not create a background trigger by itself.
- A reminder does not imply that a task was completed.
- Do not invent recurrence behavior when a provider cannot represent it.
- If task tools are unavailable, return a text checklist without claiming durable state.
