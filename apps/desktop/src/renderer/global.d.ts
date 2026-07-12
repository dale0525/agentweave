/// <reference types="vite/client" />

export {};

declare global {
  interface Window {
    generalAgent?: {
      owner: {
        principal(): Promise<unknown>;
        approverPrincipal(): Promise<unknown>;
        listSkills(): Promise<unknown>;
        skillDetail(packageId: string): Promise<unknown>;
        createDraft(request: unknown): Promise<unknown>;
        updateDraft(revisionId: string, files: unknown): Promise<unknown>;
        validateDraft(revisionId: string): Promise<unknown>;
        requestActivation(revisionId: string): Promise<unknown>;
        resolveApproval(approvalId: string, decision: "approve" | "reject"): Promise<unknown>;
        rollback(packageId: string, revisionId: string): Promise<unknown>;
        disable(packageId: string): Promise<unknown>;
        requestRemoval(packageId: string): Promise<unknown>;
      };
    };
  }
}
