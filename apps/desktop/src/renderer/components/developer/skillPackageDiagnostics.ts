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

export function packageStateLabel(
  skillPackage: DevSkillPackage,
  translate: (key: string) => string = defaultTranslation
): string {
  if (packageHasBlockingDiagnostics(skillPackage)) {
    return translate("developer.stateIssues");
  }

  if (!skillPackage.hasSkillMd && skillPackage.packageKind === "runtime") {
    return translate("developer.stateRuntimeOnly");
  }

  return translate("developer.stateReady");
}

export function packageValidationHeading(
  skillPackage: DevSkillPackage,
  translate: (key: string) => string = defaultTranslation
): string {
  if (packageHasBlockingDiagnostics(skillPackage)) {
    return translate("developer.needsAttention");
  }

  if (!skillPackage.hasSkillMd && skillPackage.packageKind === "runtime") {
    return translate("developer.stateRuntimeOnly");
  }

  return translate("developer.pass");
}

function defaultTranslation(key: string): string {
  return {
    "developer.stateIssues": "Validation issues",
    "developer.stateRuntimeOnly": "Runtime only",
    "developer.stateReady": "Ready",
    "developer.needsAttention": "Needs attention",
    "developer.pass": "PASS"
  }[key] ?? key;
}
