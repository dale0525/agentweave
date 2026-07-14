# Scheduler Store v1

Supported schedules are one-shot instants, anchored intervals, timezone-aware cron expressions, and bounded RRULE subsets.

Every job binds App, tenant, user, name, schedule, misfire policy, bounded payload, status, next occurrence, and version.

Claims bind job ID, deterministic run ID, due time, worker, lease, and payload. An expired claim is recovered with the same run identity.

Misfire policy is explicit: skip with grace, fire once, or bounded catch-up. The host owns persistence, worker lifecycle, concurrency, and audit.
