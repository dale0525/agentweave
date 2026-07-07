import { describe, expect, it } from "vitest";

import {
  buildCreateSkillPrompt,
  buildModifySkillPrompt
} from "../src/renderer/devSkillPrompts";
import { DevSkillPackage } from "../src/renderer/api";

describe("developer skill prompts", () => {
  it("builds a create prompt for Codex skill-creator", () => {
    const prompt = buildCreateSkillPrompt("/repo/skills");

    expect(prompt).toContain("Use the existing skill-creator skill");
    expect(prompt).toContain("/repo/skills");
    expect(prompt).toContain("SKILL.md is a development authoring asset");
    expect(prompt).toContain("skill.json is the GeneralAgent runtime contract");
  });

  it("builds a modify prompt with package diagnostics", () => {
    const skillPackage: DevSkillPackage = {
      id: "echo",
      path: "echo",
      name: "echo",
      description: "Echo a text payload.",
      hasSkillMd: false,
      hasRuntimeManifest: true,
      runtimeTools: ["echo"],
      packageKind: "runtime",
      bundleReady: true,
      validation: {
        ok: false,
        errors: ["missing SKILL.md is informational only"],
        warnings: []
      }
    };

    const prompt = buildModifySkillPrompt("/repo/skills", skillPackage);

    expect(prompt).toContain("Use the existing skill-creator skill");
    expect(prompt).toContain("/repo/skills/echo");
    expect(prompt).toContain("runtime tools: echo");
    expect(prompt).toContain("missing SKILL.md is informational only");
  });
});
