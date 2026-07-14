import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const PROJECT_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const CATALOG_PATH = join(PROJECT_ROOT, "catalog", "vscode-themes.json");

export const VSCODE_THEME_CATALOG = Object.freeze(
  JSON.parse(readFileSync(CATALOG_PATH, "utf8")),
);

export const VSCODE_BUILTIN_THEME_IDS = Object.freeze(
  VSCODE_THEME_CATALOG.themes.map((theme) => theme.id),
);

export function parseJsonc(text, label = "JSONC document") {
  try {
    return JSON.parse(stripJsonCommentsAndTrailingCommas(text));
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`${label} is not valid JSON or JSONC: ${detail}`);
  }
}

export function mergeVsCodeTheme(base, override) {
  return {
    ...base,
    ...override,
    colors: {
      ...(isPlainObject(base.colors) ? base.colors : {}),
      ...(isPlainObject(override.colors) ? override.colors : {}),
    },
  };
}

export function validateVsCodeThemeDocument(value, label = "VS Code theme") {
  if (!isPlainObject(value)) throw new Error(`${label} must be an object`);
  if (value.colors !== undefined && !isPlainObject(value.colors)) {
    throw new Error(`${label}.colors must be an object`);
  }
  if (value.include !== undefined && typeof value.include !== "string") {
    throw new Error(`${label}.include must be a string`);
  }
  if (value.name !== undefined && typeof value.name !== "string") {
    throw new Error(`${label}.name must be a string`);
  }
  return value;
}

function stripJsonCommentsAndTrailingCommas(text) {
  let output = "";
  let index = 0;
  let inString = false;
  let escaped = false;

  while (index < text.length) {
    const current = text[index];
    const next = text[index + 1];
    if (inString) {
      output += current;
      if (escaped) escaped = false;
      else if (current === "\\") escaped = true;
      else if (current === '"') inString = false;
      index += 1;
      continue;
    }
    if (current === '"') {
      inString = true;
      output += current;
      index += 1;
      continue;
    }
    if (current === "/" && next === "/") {
      output += "  ";
      index += 2;
      while (index < text.length && text[index] !== "\n") {
        output += " ";
        index += 1;
      }
      continue;
    }
    if (current === "/" && next === "*") {
      output += "  ";
      index += 2;
      while (index < text.length && !(text[index] === "*" && text[index + 1] === "/")) {
        output += text[index] === "\n" ? "\n" : " ";
        index += 1;
      }
      if (index < text.length) {
        output += "  ";
        index += 2;
      }
      continue;
    }
    output += current;
    index += 1;
  }

  return stripTrailingCommas(output);
}

function stripTrailingCommas(text) {
  let output = "";
  let index = 0;
  let inString = false;
  let escaped = false;
  while (index < text.length) {
    const current = text[index];
    if (inString) {
      output += current;
      if (escaped) escaped = false;
      else if (current === "\\") escaped = true;
      else if (current === '"') inString = false;
      index += 1;
      continue;
    }
    if (current === '"') {
      inString = true;
      output += current;
      index += 1;
      continue;
    }
    if (current === ",") {
      let lookahead = index + 1;
      while (lookahead < text.length && /\s/.test(text[lookahead])) lookahead += 1;
      if (text[lookahead] === "}" || text[lookahead] === "]") {
        index += 1;
        continue;
      }
    }
    output += current;
    index += 1;
  }
  return output;
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
