---
name: foundation-documents
description: Inspect, extract, create, convert, render, and verify document artifacts through host-provided document tools while preserving source provenance. Use for PDF, Word, spreadsheet, presentation, Markdown, or text document work; format conversion; layout checks; 文档读取、生成、转换、排版或视觉验收.
---

# Foundation Documents

Use host document and artifact tools. Preserve the source file unless the user explicitly requests replacement.

Read [references/contract.md](references/contract.md) before conversion, creation, or layout-sensitive delivery.

## Follow the artifact workflow

1. Inspect the source format, size, revision, and requested output before editing.
2. Keep extraction, semantic edits, conversion, rendering, and visual verification as distinct steps.
3. Create derived artifacts with source IDs and content hashes.
4. Render PDF, DOCX, XLSX, and PPTX outputs when layout matters.
5. Record verification notes and iterate until blocking issues are resolved.
6. Deliver the verified artifact ID or path and disclose any accepted limitation.

## Respect boundaries

- This skill does not own filesystem permission, sandboxing, temporary storage, authorization, or artifact retention.
- Never execute macros or active document content.
- Never overwrite an input merely because an output has the same extension.
- Do not describe a layout-sensitive artifact as verified without a host verification result.
- Use text-only fallback to summarize planned operations when document tools are unavailable.
