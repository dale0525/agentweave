# Document Artifact Host v1

Every artifact preserves an ID, file name, format, byte size, content hash, revision, creation time, source artifact IDs, verification state, and verification notes.

Inspect and extract operations are bounded reads. Create and derive operations produce a new artifact by default. Explicit replacement must retain the prior revision or another reversible history mechanism.

Layout-sensitive formats require rendering and visual verification. A failed check leaves the artifact unverified and records concise issues such as clipping, overflow, missing fonts, broken pagination, or unreadable charts.

The host owns path confinement, process sandboxing, malware handling, macro disabling, temporary-file cleanup, and final artifact retention.
