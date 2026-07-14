# Notes Provider v1

Every note preserves a stable ID, title, body, tags, owner, sharing state, source IDs, provider reference, version, and timestamps.

Create, search, get, update, organize, and delete are scoped to the active App, tenant, user, and provider account. Update and delete require the version that was read.

Notes are explicit user-owned artifacts. Memory is Agent-maintained contextual state. Providers must not silently mirror one into the other.

The host owns credentials, authorization, encryption, sharing enforcement, synchronization, retention, and audit.
