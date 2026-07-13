export type ApprovalDecision = "approve" | "reject";

export type ApprovalObservationResult =
  | {
    approvalId: string;
    decision: ApprovalDecision;
    resolution?: unknown;
    status: "completed";
  }
  | { approvalId: string; status: "closed" }
  | { approvalId: string; status: "disposed" }
  | { approvalId: string; status: "load_failed" };
