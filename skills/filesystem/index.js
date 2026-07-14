const fs = require("fs");
const path = require("path");

const MAX_SEARCH_RESULTS = 1000;
const MAX_MATCH_TEXT_BYTES = 4096;

function main() {
  const input = readInput();
  const tool = process.env.AGENTWEAVE_TOOL_NAME;
  const workspaceRoot = requiredEnvPath("AGENTWEAVE_WORKSPACE_ROOT");
  const realRoot = fs.realpathSync(workspaceRoot);

  let result;
  switch (tool) {
    case "create_directory":
      result = createDirectory(realRoot, input);
      break;
    case "list_directory":
      result = listDirectory(realRoot, input);
      break;
    case "file_metadata":
      result = fileMetadata(realRoot, input);
      break;
    case "read_text_file":
      result = readTextFile(realRoot, input);
      break;
    case "write_text_file":
      result = writeTextFile(realRoot, input);
      break;
    case "search_files":
      result = searchFiles(realRoot, input);
      break;
    case "apply_patch":
      result = applyPatch(realRoot, input);
      break;
    default:
      throw new Error(`unknown filesystem tool: ${tool || "<missing>"}`);
  }

  process.stdout.write(JSON.stringify(result));
}

function readInput() {
  const chunks = [];
  const buffer = fs.readFileSync(0);
  if (buffer.length === 0) {
    return {};
  }
  chunks.push(buffer);
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function requiredEnvPath(name) {
  const value = process.env[name];
  if (!value || value.trim() === "") {
    throw new Error(`${name} is required`);
  }
  return value;
}

function createDirectory(root, input) {
  const target = resolveOutputPath(root, requiredString(input, "path"));
  const existed = fs.existsSync(target.absolute) && fs.statSync(target.absolute).isDirectory();
  fs.mkdirSync(target.absolute, { recursive: true });
  return {
    path: target.relative,
    created: !existed,
  };
}

function listDirectory(root, input) {
  const target = resolveExistingPath(root, requiredString(input, "path"));
  const limit = optionalLimit(input, 100);
  const entries = fs
    .readdirSync(target.absolute, { withFileTypes: true })
    .map((entry) => {
      const absolute = path.join(target.absolute, entry.name);
      const metadata = fs.lstatSync(absolute);
      return {
        name: entry.name,
        path: toWorkspacePath(root, path.join(target.relative, entry.name)),
        type: metadataType(metadata),
        size: metadata.size,
      };
    })
    .sort((left, right) => left.path.localeCompare(right.path));

  const truncated = entries.length > limit;
  return {
    path: target.relative,
    entries: entries.slice(0, limit),
    truncated,
  };
}

function fileMetadata(root, input) {
  const requested = requiredString(input, "path");
  const target = resolveWorkspacePath(root, requested);
  if (!fs.existsSync(target.absolute)) {
    return {
      path: target.relative,
      exists: false,
    };
  }

  ensureExistingPathInsideRoot(root, target.absolute);
  const metadata = fs.lstatSync(target.absolute);
  return {
    path: target.relative,
    exists: true,
    type: metadataType(metadata),
    size: metadata.size,
  };
}

function readTextFile(root, input) {
  const target = resolveExistingPath(root, requiredString(input, "path"));
  const bytes = fs.readFileSync(target.absolute);
  const text = bytes.toString("utf8");
  if (Buffer.from(text, "utf8").compare(bytes) !== 0) {
    throw new Error("workspace path is not valid UTF-8 text");
  }
  return {
    path: target.relative,
    text,
  };
}

function writeTextFile(root, input) {
  const requested = requiredString(input, "path");
  const text = requiredString(input, "text");
  const overwrite = optionalBoolean(input, "overwrite", false);
  const target = resolveOutputPath(root, requested);

  if (fs.existsSync(target.absolute) && !overwrite) {
    throw new Error("refusing to overwrite existing path without overwrite=true");
  }
  fs.mkdirSync(path.dirname(target.absolute), { recursive: true });
  fs.writeFileSync(target.absolute, text, "utf8");
  return {
    path: target.relative,
    bytes: Buffer.byteLength(text, "utf8"),
  };
}

function searchFiles(root, input) {
  const pattern = requiredString(input, "pattern");
  const searchRoot = resolveExistingPath(root, optionalString(input, "path", "."));
  const limit = optionalLimit(input, 100);
  const matches = [];
  let truncated = false;

  for (const filePath of walkTextFiles(searchRoot.absolute)) {
    const relative = relativePath(root, filePath);
    const text = fs.readFileSync(filePath, "utf8");
    const lines = text.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const column = lines[index].indexOf(pattern);
      if (column === -1) {
        continue;
      }
      if (matches.length >= limit) {
        truncated = true;
        break;
      }
      matches.push({
        path: relative,
        line: index + 1,
        column: column + 1,
        text: truncateUtf8(lines[index], MAX_MATCH_TEXT_BYTES),
      });
    }
    if (truncated) {
      break;
    }
  }

  return {
    path: searchRoot.relative,
    pattern,
    matches,
    truncated,
    engine: "node",
  };
}

function applyPatch(root, input) {
  const patchText = requiredString(input, "patch");
  const operations = parsePatch(patchText);
  const changedFiles = [];

  for (const operation of operations) {
    if (operation.type === "add") {
      const target = resolveOutputPath(root, operation.path);
      if (fs.existsSync(target.absolute)) {
        throw new Error(`path already exists: ${operation.path}`);
      }
      fs.mkdirSync(path.dirname(target.absolute), { recursive: true });
      fs.writeFileSync(target.absolute, operation.lines.join("\n") + "\n", "utf8");
      changedFiles.push({
        path: target.relative,
        action: "add",
        added_lines: operation.lines.length,
        removed_lines: 0,
      });
    } else if (operation.type === "delete") {
      const target = resolveExistingPath(root, operation.path);
      fs.unlinkSync(target.absolute);
      changedFiles.push({
        path: target.relative,
        action: "delete",
        added_lines: 0,
        removed_lines: 0,
      });
    } else if (operation.type === "update") {
      const target = resolveExistingPath(root, operation.path);
      const original = fs.readFileSync(target.absolute, "utf8").split(/\r?\n/);
      if (original[original.length - 1] === "") {
        original.pop();
      }
      const updated = applyUpdateHunks(original, operation.hunks);
      fs.writeFileSync(target.absolute, updated.join("\n") + "\n", "utf8");
      changedFiles.push({
        path: target.relative,
        action: "update",
        added_lines: operation.hunks.flat().filter((line) => line.kind === "add").length,
        removed_lines: operation.hunks.flat().filter((line) => line.kind === "remove").length,
      });
    }
  }

  return { changed_files: changedFiles };
}

function parsePatch(patchText) {
  const lines = patchText.split(/\r?\n/);
  if (lines[0] !== "*** Begin Patch") {
    throw new Error("patch must start with *** Begin Patch");
  }
  const operations = [];
  let index = 1;

  while (index < lines.length) {
    const line = lines[index];
    if (line === "*** End Patch") {
      return operations;
    }
    if (line.startsWith("*** Add File: ")) {
      const filePath = line.slice("*** Add File: ".length);
      index += 1;
      const body = [];
      while (index < lines.length && !lines[index].startsWith("*** ")) {
        if (!lines[index].startsWith("+")) {
          throw new Error("add file lines must start with +");
        }
        body.push(lines[index].slice(1));
        index += 1;
      }
      operations.push({ type: "add", path: filePath, lines: body });
      continue;
    }
    if (line.startsWith("*** Delete File: ")) {
      operations.push({
        type: "delete",
        path: line.slice("*** Delete File: ".length),
      });
      index += 1;
      continue;
    }
    if (line.startsWith("*** Update File: ")) {
      const filePath = line.slice("*** Update File: ".length);
      index += 1;
      const hunks = [];
      let current = [];
      while (index < lines.length && !lines[index].startsWith("*** ")) {
        const hunkLine = lines[index];
        if (hunkLine.startsWith("@@")) {
          if (current.length > 0) {
            hunks.push(current);
            current = [];
          }
        } else if (hunkLine.startsWith("+")) {
          current.push({ kind: "add", text: hunkLine.slice(1) });
        } else if (hunkLine.startsWith("-")) {
          current.push({ kind: "remove", text: hunkLine.slice(1) });
        } else if (hunkLine.startsWith(" ")) {
          current.push({ kind: "context", text: hunkLine.slice(1) });
        } else if (hunkLine !== "") {
          throw new Error(`invalid update hunk line: ${hunkLine}`);
        }
        index += 1;
      }
      if (current.length > 0) {
        hunks.push(current);
      }
      operations.push({ type: "update", path: filePath, hunks });
      continue;
    }
    if (line === "") {
      index += 1;
      continue;
    }
    throw new Error(`unknown patch operation: ${line}`);
  }

  throw new Error("patch must end with *** End Patch");
}

function applyUpdateHunks(original, hunks) {
  let result = original.slice();
  for (const hunk of hunks) {
    const before = hunk.filter((line) => line.kind !== "add").map((line) => line.text);
    const after = hunk.filter((line) => line.kind !== "remove").map((line) => line.text);
    const position = findSubsequence(result, before);
    if (position === -1) {
      throw new Error("patch hunk did not match file content");
    }
    result = result.slice(0, position).concat(after, result.slice(position + before.length));
  }
  return result;
}

function findSubsequence(haystack, needle) {
  if (needle.length === 0) {
    return 0;
  }
  for (let index = 0; index <= haystack.length - needle.length; index += 1) {
    let matched = true;
    for (let offset = 0; offset < needle.length; offset += 1) {
      if (haystack[index + offset] !== needle[offset]) {
        matched = false;
        break;
      }
    }
    if (matched) {
      return index;
    }
  }
  return -1;
}

function walkTextFiles(root) {
  const metadata = fs.lstatSync(root);
  if (metadata.isFile()) {
    return isLikelyTextFile(root) ? [root] : [];
  }
  if (!metadata.isDirectory()) {
    return [];
  }

  const files = [];
  const entries = fs.readdirSync(root, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    if (entry.name === ".git" || entry.name === "node_modules") {
      continue;
    }
    files.push(...walkTextFiles(path.join(root, entry.name)));
  }
  return files;
}

function isLikelyTextFile(filePath) {
  const buffer = fs.readFileSync(filePath);
  return !buffer.includes(0);
}

function resolveWorkspacePath(root, requested) {
  const absolute = path.resolve(root, requested);
  ensureInsideRoot(root, absolute);
  return {
    absolute,
    relative: relativePath(root, absolute),
  };
}

function resolveExistingPath(root, requested) {
  const target = resolveWorkspacePath(root, requested);
  const real = fs.realpathSync(target.absolute);
  ensureInsideRoot(root, real);
  return {
    absolute: real,
    relative: relativePath(root, target.absolute),
  };
}

function resolveOutputPath(root, requested) {
  const target = resolveWorkspacePath(root, requested);
  ensureOutputParentInsideRoot(root, target.absolute);
  if (fs.existsSync(target.absolute)) {
    ensureExistingPathInsideRoot(root, target.absolute);
  }
  return target;
}

function ensureExistingPathInsideRoot(root, absolute) {
  const real = fs.realpathSync(absolute);
  ensureInsideRoot(root, real);
}

function ensureOutputParentInsideRoot(root, absolute) {
  let cursor = path.dirname(absolute);
  while (!fs.existsSync(cursor)) {
    const parent = path.dirname(cursor);
    if (parent === cursor) {
      break;
    }
    cursor = parent;
  }
  ensureExistingPathInsideRoot(root, cursor);
}

function ensureInsideRoot(root, absolute) {
  const relative = path.relative(root, absolute);
  if (relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))) {
    return;
  }
  throw new Error("path escapes workspace root");
}

function relativePath(root, absolute) {
  return toWorkspacePath(root, path.relative(root, absolute));
}

function toWorkspacePath(root, value) {
  const normalized = value.split(path.sep).join("/");
  return normalized === "" ? "." : normalized;
}

function metadataType(metadata) {
  if (metadata.isDirectory()) {
    return "directory";
  }
  if (metadata.isFile()) {
    return "file";
  }
  if (metadata.isSymbolicLink()) {
    return "symlink";
  }
  return "other";
}

function requiredString(input, field) {
  const value = input[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`missing string field: ${field}`);
  }
  return value;
}

function optionalString(input, field, fallback) {
  const value = input[field];
  if (value === undefined) {
    return fallback;
  }
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`field must be a non-empty string: ${field}`);
  }
  return value;
}

function optionalBoolean(input, field, fallback) {
  const value = input[field];
  if (value === undefined) {
    return fallback;
  }
  if (typeof value !== "boolean") {
    throw new Error(`field must be boolean: ${field}`);
  }
  return value;
}

function optionalLimit(input, fallback) {
  const value = input.limit;
  if (value === undefined) {
    return fallback;
  }
  if (!Number.isInteger(value) || value < 1 || value > MAX_SEARCH_RESULTS) {
    throw new Error(`limit must be an integer between 1 and ${MAX_SEARCH_RESULTS}`);
  }
  return value;
}

function truncateUtf8(value, limit) {
  const bytes = Buffer.from(value, "utf8");
  if (bytes.length <= limit) {
    return value;
  }
  return bytes.subarray(0, limit).toString("utf8");
}

try {
  main();
} catch (error) {
  process.stderr.write(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
