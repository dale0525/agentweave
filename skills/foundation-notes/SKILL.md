---
name: foundation-notes
description: Create, search, organize, update, or delete explicit user-owned notes and knowledge pages through a Notes Provider. Use for notebooks, knowledge bases, meeting notes, saved reference material, 笔记、知识库、会议记录、整理资料或显式保存内容; do not use as a substitute for Agent Memory.
---

# Foundation Notes

Treat notes as explicit user-owned content. Keep them separate from Agent-maintained Memory and its retention policy.

Read [references/contract.md](references/contract.md) before changing ownership, sharing, provenance, or deleting a note.

## Follow the workflow

1. Search existing notes before creating a likely duplicate.
2. Preserve title, body, tags, owner, sharing state, source IDs, and version.
3. Use optimistic versions for update and delete.
4. Keep imported or derived content traceable to its sources.
5. Report the final provider state without claiming that a note became Agent Memory.

## Respect boundaries

- This skill does not own credentials, authorization, sharing policy, durable storage, or audit.
- Do not save a private conversation or connector result as a note without user intent.
- Do not widen sharing to solve an access error.
- If note tools are unavailable, return text the user can copy without claiming it was saved.
