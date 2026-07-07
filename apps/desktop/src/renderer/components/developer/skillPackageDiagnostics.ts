import { DevSkillPackage } from "../../api";

function isMissingSkillMdDiagnostic(message: string): boolean {
  return /skill\.md/i.test(message) && /missing/i.test(message);
}

function isRuntimeOnlyPackage(skillPackage: DevSkillPackage): boolean {
  return skillPackage.packageKind === "runtime" && !skillPackage.hasSkillMd;
}

export function packageHasBlockingDiagnostics(skillPackage: DevSkillPackage): boolean {
  if (skillPackage.validation.ok) {
    return false;
  }

  const diagnostics = [
    ...skillPackage.validation.errors,
    ...skillPackage.validation.warnings
  ];
  if (diagnostics.length === 0) {
    return true;
  }

  if (!isRuntimeOnlyPackage(skillPackage)) {
    return true;
  }

  return diagnostics.some((item) => !isMissingSkillMdDiagnostic(item));
}

export function packageStateLabel(skillPackage: DevSkillPackage): string {
  if (packageHasBlockingDiagnostics(skillPackage)) {
    return "Validation issues";
  }

  if (!skillPackage.hasSkillMd && skillPackage.packageKind === "runtime") {
    return "Runtime only";
  }

  return "Ready";
}

export function packageValidationHeading(skillPackage: DevSkillPackage): string {
  if (packageHasBlockingDiagnostics(skillPackage)) {
    return "Needs attention";
  }

  if (!skillPackage.hasSkillMd && skillPackage.packageKind === "runtime") {
    return "Runtime only";
  }

  return "PASS";
}
