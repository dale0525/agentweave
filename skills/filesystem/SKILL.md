---
name: filesystem
description: Use when AgentWeave needs to create, inspect, read, write, search, or patch files in the workspace through the packaged filesystem runtime tools.
---

# Filesystem

Use the packaged runtime tools for concrete workspace filesystem work instead of explaining commands to the user.

Available tools:

- `create_directory`: create workspace directories.
- `list_directory`: list workspace directory entries.
- `file_metadata`: inspect path existence, type, and size.
- `read_text_file`: read UTF-8 text files.
- `write_text_file`: write UTF-8 text files, using `overwrite: true` when replacing existing content.
- `search_files`: search text inside workspace files.
- `apply_patch`: apply minimal patch blocks inside the workspace.

All paths are resolved inside `AGENTWEAVE_WORKSPACE_ROOT`. Do not use these tools for paths outside the active workspace.
