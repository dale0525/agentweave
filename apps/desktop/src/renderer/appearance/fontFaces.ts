import type { PackagedFontFace } from "./types";

const STYLE_ELEMENT_ID = "agentweave-packaged-fonts";
const FONT_FAMILIES = {
  display: "AgentWeave Display",
  mono: "AgentWeave Mono",
  ui: "AgentWeave UI"
} as const;

export function installPackagedFonts(fontFaces: PackagedFontFace[]): void {
  if (typeof document === "undefined" || fontFaces.length === 0) return;
  const existing = document.getElementById(STYLE_ELEMENT_ID);
  existing?.remove();
  const style = document.createElement("style");
  style.id = STYLE_ELEMENT_ID;
  style.textContent = fontFaces.map(fontFaceRule).join("\n");
  document.head.append(style);

  const slots = new Set(fontFaces.map((font) => font.slot));
  const root = document.documentElement.style;
  if (slots.has("ui")) root.setProperty("--font-ui", `"${FONT_FAMILIES.ui}", sans-serif`);
  if (slots.has("display")) {
    root.setProperty("--font-display", `"${FONT_FAMILIES.display}", var(--font-ui)`);
  }
  if (slots.has("mono")) root.setProperty("--font-mono", `"${FONT_FAMILIES.mono}", monospace`);
}

function fontFaceRule(font: PackagedFontFace): string {
  const family = FONT_FAMILIES[font.slot];
  return [
    "@font-face {",
    `  font-family: "${family}";`,
    `  src: url("${font.dataUrl}") format("${font.format}");`,
    `  font-style: ${font.style};`,
    `  font-weight: ${font.weight};`,
    "  font-display: swap;",
    "}"
  ].join("\n");
}
