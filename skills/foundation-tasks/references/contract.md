# Task Provider v1

The provider exposes bounded list, get, create, update, status, and delete operations.

Every task preserves a stable ID, title, optional notes, priority, tags, due instant, source timezone, optional recurrence, status, version, and timestamps.

Create is idempotent. Update, completion, cancellation, and deletion use optimistic versions. A stale version must fail and require a refetch.

Task state is not a scheduler claim. The host may associate a task with a separate scheduled job, but each object retains its own ID, lifecycle, audit, and failure state.

Text-only fallback may describe a proposed task. It must never say the task was saved, completed, or deleted.
