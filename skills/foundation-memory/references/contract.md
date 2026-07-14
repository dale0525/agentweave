# Foundation Memory Contract

Use this reference to map available host operations to the provider-neutral memory workflow. The names below describe capabilities; a host may expose different tool names.

## Ownership boundary

The host owns identity, scope injection, authorization, approval, credentials, persistence, encryption, audit, availability, and tool execution. This skill owns only model behavior and safe operation selection.

Fail closed when the host does not provide the required operation. Do not create an alternative memory store.

## Required host operations

| Contract operation | Purpose | Expected result |
| --- | --- | --- |
| `pre_turn_recall` | Search live committed records before a turn | Scoped records only |
| `post_turn_candidates` | Submit inferred or summarized candidates | Proposed records |
| `on_session_end` | Submit final candidates and expire session retention | Proposals plus provider lifecycle result |
| `on_compaction` | Preserve selected candidates across compaction | Proposed records |
| `mutate.propose` | Stage a durable-memory candidate | `proposed` record |
| `mutate.confirm` | Commit an approved proposal | `committed` record |
| `mutate.update` | Compare-and-swap a live record | New version or version conflict |
| `mutate.forget` | Scrub a record and its derived index | Scrubbed tombstone |
| `export` | Export records in one scope | Versioned scoped export |
| `delete_scope` | Physically delete one authorized scope | Deleted record and index counts |

The host may combine these under one memory tool. Preserve the same state transitions and safety properties when mapping them.

## Stable record model

Every record has:

- a UUID memory ID;
- an exact `app_id`, `tenant_id`, and `user_id` scope;
- an extensible dotted kind such as `user.preference` or `work.project`;
- a concise value with optional structured attributes;
- one or more evidence items with source and observation time;
- confidence in basis points from 0 to 10,000;
- sensitivity and retention metadata;
- state: `proposed`, `committed`, or `tombstoned`;
- a positive version for optimistic compare-and-swap;
- optional conflict key, supersession links, and tombstone metadata.

Never expose raw scope principals in normal user-facing output.

## Recall

Build the smallest request that answers the task:

- exact trusted scope;
- a short query, which may be empty only for an intentional scoped listing;
- optional kinds;
- a bounded result limit.

Recall returns only committed, unexpired, non-superseded records. Proposals and tombstones are not personalization context.

Do not broaden a failed query into an unrestricted export without user intent and authorization.

## Propose and confirm

A proposal contains the candidate value, evidence, confidence, sensitivity, retention, optional conflict key, and optional `supersedes` ID.

Use evidence that explains why the candidate exists without copying unnecessary private text. Distinguish explicit user statements from tool observations, session summaries, compaction summaries, and imports.

Inferred facts remain proposed. Confirm only when the user and host policy authorize durable commitment. Never describe a proposal as remembered permanently.

## Update and conflict handling

Update requires the record ID and the version that was read. A stale version must fail with a version conflict; refetch before offering another update.

Records with the same scope, kind, and conflict key may conflict. Return the competing IDs and ask for a resolution. Do not pick the newest, highest-confidence, or most convenient value silently.

Use explicit supersession to replace an older committed record while keeping auditable history. Once superseded, the old record must leave recall and search results.

## Retention and deletion

Supported retention modes are:

- persistent;
- expiration at a specific instant;
- session retention tied to a host session ID.

Expired and ended-session records become scrubbed tombstones. Forget also creates a scrubbed tombstone: value empty, evidence empty, and derived search entries deleted.

Scope deletion is stronger than forget. It physically removes every record and derived index entry for exactly one authorized scope. Confirm ambiguous or broad scope deletion through the host approval boundary.

## Export

Export is exact-scope only. The caller must explicitly choose whether proposals and tombstones are included. Preserve the contract schema version and record state in structured output.

Do not treat export as recall. Export can reveal more information and requires separate user intent and authorization.

## Sensitive-data rules

Credential material never belongs in memory. Refuse to persist secrets used for authentication and direct them to the host credential vault.

For other sensitive material, minimize the value, attributes, evidence excerpt, and retention. Do not log or echo memory payloads in provider errors. Provider failures should identify the operation, not SQL text or bound values.

## Completion language

Use precise state language:

- “I found…” only after recall returns a record.
- “I proposed…” only after proposal succeeds.
- “I remembered…” only after confirmation returns committed state.
- “I updated…” only after compare-and-swap succeeds.
- “I forgot…” only after the provider returns a scrubbed tombstone.
- “I deleted this memory scope…” only after scope deletion returns success counts.

If execution is unavailable, denied, conflicted, or incomplete, state that outcome and the next required user or host action.
