---
name: foundation-memory
description: Safely recall, propose, confirm, correct, export, or forget durable user and application memory through host-provided memory operations. Use when a user asks to remember, recall, list, update, correct, forget, delete, or export memory; mentions preferences or long-term memory; or says 记住、回忆、偏好、忘记、删除记忆、长期记忆.
---

# Foundation Memory

Use durable memory only through the host's scoped Memory Provider. Keep proposals separate from committed records, preserve provenance, and make destructive outcomes explicit.

Read [references/contract.md](references/contract.md) before changing durable state, resolving a conflict, exporting memory, deleting a scope, or mapping host tools to the contract.

## Respect the boundary

- This is a host-tools-only skill. It does not own credentials, authorization, approvals, persistence, encryption, audit, or a database.
- Accept `app_id`, `tenant_id`, and `user_id` only from trusted host context. Never invent, widen, or cross a scope.
- Use only host-provided memory operations. Never simulate durable memory with a workspace file, hidden prompt text, notes, or unsupported tool calls.
- If the required operation is unavailable or denied, say so plainly and do not claim that memory changed.

## Route the request

- Recall or “what do you remember?”: run a narrow recall or an authorized export. Do not expose another scope or tombstoned data.
- “Remember this”: create a proposal with evidence, confidence, sensitivity, retention, and an optional conflict key.
- Confirm a proposal: commit only after the host's approval rule is satisfied. A direct “remember X” command counts as confirmation only when host policy explicitly permits it.
- Correct or update: use the current record ID and expected version. Prefer explicit supersession when preserving replacement history matters.
- Forget or delete: target the exact record when possible. Require the host's confirmation or approval boundary for ambiguous or scope-wide deletion.
- Export: return only the requested scope and honor proposal/tombstone inclusion flags.

## Work safely

1. Identify the user's intent and the minimum necessary scope, query, kinds, and result limit.
2. Recall only information relevant to the current task. Do not fetch the entire memory store for convenience.
3. For new memory, record a short normalized value and minimal evidence. Do not store a full conversation when a concise fact is sufficient.
4. Keep inferred candidates proposed. Report whether the result is `proposed`, `committed`, `updated`, or `tombstoned`.
5. On version or semantic conflict, show a concise, non-sensitive explanation and ask the user which value should remain. Never resolve a conflict silently.
6. After update, forget, export, or scope deletion, trust only the provider result. Do not infer success from an attempted call.

## Minimize sensitive data

- Never place passwords, session cookies, API keys, private keys, recovery codes, or authentication tokens in memory. Direct the user to the host credential vault instead.
- Use the least-sensitive classification that is accurate, the shortest useful evidence excerpt, and the narrowest viable retention.
- Treat relationship context, health, finance, identity, location, and private communications as sensitive. Persist them only when host policy and user intent allow it.
- A forget operation must scrub the stored value and evidence and remove derived search data. A tombstone may retain only non-secret deletion metadata.

## Respond with state, not implementation detail

Briefly state what was recalled or changed, its durable state, and any action still required. Do not reveal internal tenant identifiers, raw evidence, SQL details, provider credentials, or unrelated memories.
