import { DevSkillPackage } from "./api";

export function buildCreateSkillPrompt(root: string): string {
  return [
    "Use the existing skill-creator skill to create a new GeneralAgent skill package.",
    "",
    `Target skills root: ${root}`,
    "",
    "Requirements:",
    "- Create the package under the target skills root.",
    "- SKILL.md is a development authoring asset for Codex guidance.",
    "- skill.json is the GeneralAgent runtime contract for packaged tools.",
    "- Add or update skill.json only when the package needs runtime tools.",
    "- Keep generated source files focused and under 1000 physical lines.",
    "",
    "After creating the package, run the GeneralAgent development skill validation."
  ].join("\n");
}

export function buildModifySkillPrompt(
  root: string,
  skillPackage: DevSkillPackage
): string {
  const runtimeTools =
    skillPackage.runtimeTools.length > 0
      ? skillPackage.runtimeTools.join(", ")
      : "none";
  const errors =
    skillPackage.validation.errors.length > 0
      ? skillPackage.validation.errors.map((error) => `- ${error}`).join("\n")
      : "- none";
  const warnings =
    skillPackage.validation.warnings.length > 0
      ? skillPackage.validation.warnings
          .map((warning) => `- ${warning}`)
          .join("\n")
      : "- none";

  return [
    "Use the existing skill-creator skill to modify this GeneralAgent skill package.",
    "",
    `Package path: ${root}/${skillPackage.path}`,
    `Package name: ${skillPackage.name}`,
    `Description: ${skillPackage.description}`,
    `Package kind: ${skillPackage.packageKind}`,
    `Files present: SKILL.md=${skillPackage.hasSkillMd}, skill.json=${skillPackage.hasRuntimeManifest}`,
    `runtime tools: ${runtimeTools}`,
    `Bundle ready: ${skillPackage.bundleReady}`,
    "",
    "Validation errors:",
    errors,
    "",
    "Validation warnings:",
    warnings,
    "",
    "Remember: SKILL.md is a development authoring asset; skill.json is the GeneralAgent runtime contract."
  ].join("\n");
}
