/// <reference types="vite/client" />

import type { ApprovalObservationResult } from "../shared/approvalObservation";

export {};

declare global {
  interface Window {
    generalAgent?: {
      owner: {
        principal(): Promise<unknown>;
        listSkills(): Promise<unknown>;
        skillDetail(packageId: string): Promise<unknown>;
        createDraft(request: unknown): Promise<unknown>;
        updateDraft(revisionId: string, files: unknown): Promise<unknown>;
        validateDraft(revisionId: string): Promise<unknown>;
        requestActivation(revisionId: string): Promise<unknown>;
        rollback(packageId: string, revisionId: string): Promise<unknown>;
        disable(packageId: string): Promise<unknown>;
        requestRemoval(packageId: string): Promise<unknown>;
      };
      approval: {
        open(approvalId: string): Promise<ApprovalObservationResult>;
      };
    };
    generalAgentApproval?: {
      principal(): Promise<unknown>;
      approval(approvalId: string): Promise<unknown>;
      resolve(approvalId: string, decision: "approve" | "reject"): Promise<unknown>;
      complete(result: unknown): Promise<unknown>;
      close(approvalId: string): Promise<unknown>;
    };
  }
}
