import {
  OwnerSkillApproval,
  OwnerSkillDraftSummary,
  OwnerSkillInventory,
  OwnerSkillMutationReport,
  OwnerSkillPackage,
  OwnerSkillValidation
} from "./api";
import { getOwnerApi } from "./ownerBridge";

export const ownerClient = {
  listSkills: () => getOwnerApi().listSkills() as Promise<OwnerSkillInventory>,
  skillDetail: (packageId: string) =>
    getOwnerApi().skillDetail(packageId) as Promise<OwnerSkillPackage>,
  createDraft: (request: unknown) =>
    getOwnerApi().createDraft(request) as Promise<OwnerSkillDraftSummary>,
  updateDraft: (revisionId: string, files: unknown) =>
    getOwnerApi().updateDraft(revisionId, files) as Promise<OwnerSkillDraftSummary>,
  validateDraft: (revisionId: string) =>
    getOwnerApi().validateDraft(revisionId) as Promise<OwnerSkillValidation>,
  requestActivation: (revisionId: string) =>
    getOwnerApi().requestActivation(revisionId) as Promise<OwnerSkillApproval>,
  rollback: (packageId: string, revisionId: string) =>
    getOwnerApi().rollback(packageId, revisionId) as Promise<OwnerSkillMutationReport & Partial<OwnerSkillApproval>>,
  disable: (packageId: string) => getOwnerApi().disable(packageId),
  requestRemoval: (packageId: string) =>
    getOwnerApi().requestRemoval(packageId) as Promise<OwnerSkillApproval>
};
