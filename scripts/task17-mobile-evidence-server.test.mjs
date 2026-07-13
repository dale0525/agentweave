import assert from "node:assert/strict";
import test from "node:test";

import { buildNextTurnEvidence } from "./task17-mobile-evidence-server.mjs";

test("binds one provider request to authoritative mobile revision evidence", () => {
  const marker = "TASK17_UI_ACTIVE_SKILL_EVIDENCE";
  const requestBody = {
    input: [
      { role: "developer", content: `available: ${marker}` },
      { role: "user", content: "prove_active_skill" },
    ],
  };

  const evidence = buildNextTurnEvidence(
    {
      "x-task17-request-id": "request-1",
      "x-task17-revision-id": "revision-1",
      "x-task17-content-hash": "hash-1",
      "x-task17-marker": marker,
      "x-task17-user-text": "prove_active_skill",
      authorization: "Bearer must-not-leak",
    },
    requestBody,
  );

  assert.deepEqual(evidence, {
    request_id: "request-1",
    user_text: "prove_active_skill",
    active_revision_id: "revision-1",
    content_hash: "hash-1",
    marker,
    request_body: requestBody,
  });
  assert.doesNotMatch(JSON.stringify(evidence), /must-not-leak/);
});

test("rejects a request body without the active marker", () => {
  assert.throws(
    () => buildNextTurnEvidence(
      {
        "x-task17-request-id": "request-1",
        "x-task17-revision-id": "revision-1",
        "x-task17-content-hash": "hash-1",
        "x-task17-marker": "TASK17_UI_ACTIVE_SKILL_EVIDENCE",
        "x-task17-user-text": "prove_active_skill",
      },
      { input: [{ role: "user", content: "prove_active_skill" }] },
    ),
    /active marker/,
  );
});

test("accepts Responses input_text user content from the real gateway", () => {
  const marker = "TASK17_UI_ACTIVE_SKILL_EVIDENCE";
  const requestBody = {
    input: [
      { role: "developer", content: marker },
      { role: "user", content: [{ type: "input_text", text: "prove_active_skill" }] },
    ],
  };

  const evidence = buildNextTurnEvidence(
    {
      "x-task17-request-id": "request-structured",
      "x-task17-revision-id": "revision-structured",
      "x-task17-content-hash": "hash-structured",
      "x-task17-marker": marker,
      "x-task17-user-text": "prove_active_skill",
    },
    requestBody,
  );

  assert.equal(evidence.user_text, "prove_active_skill");
});
