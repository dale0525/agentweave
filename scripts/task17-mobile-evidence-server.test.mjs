import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { buildNextTurnCapture, startEvidenceServer } from "./task17-mobile-evidence-server.mjs";

const marker = "TASK17_UI_ACTIVE_SKILL_EVIDENCE";
const nonce = "nonce-absolute-1";
const userText = `task17-mobile prove_active_skill nonce:${nonce}`;

function bodyWithDeveloper(developer) {
  return JSON.stringify({
    input: [
      { role: "developer", content: developer },
      { role: "user", content: userText },
    ],
    model: "task17-model",
  });
}

test("captures raw request digest and instruction-only marker location", () => {
  const raw = Buffer.from(bodyWithDeveloper(
    `<available_skills count="1">\n- name: Task17 mobile lifecycle\n  description: summary without marker\n</available_skills>\n\n` +
    `<skill_instructions name="Task17 mobile lifecycle" source="SKILL.md">\n${marker}\n</skill_instructions>`,
  ));

  const capture = buildNextTurnCapture(raw, { requestId: "server-request-1" });

  assert.equal(capture.request_id, "server-request-1");
  assert.equal(capture.capture_nonce, nonce);
  assert.equal(capture.user_text, userText);
  assert.equal(capture.marker_location, "skill_instructions");
  assert.equal(capture.raw_request_sha256, createHash("sha256").update(raw).digest("hex"));
  assert.equal(capture.request_body.input[1].content, userText);
  assert.equal("active_revision_id" in capture, false);
  assert.equal("content_hash" in capture, false);
});

test("rejects marker present only in available skill summary", () => {
  const raw = Buffer.from(bodyWithDeveloper(
    `<available_skills count="1">\n- name: Task17 mobile lifecycle\n  description: ${marker}\n</available_skills>`,
  ));
  assert.throws(() => buildNextTurnCapture(raw), /selected skill instructions/);
});

test("arbitrary authoritative-looking headers cannot satisfy capture", () => {
  const raw = Buffer.from(bodyWithDeveloper("summary without selected instructions"));
  assert.throws(
    () => buildNextTurnCapture(raw, {
      headers: {
        "x-task17-revision-id": "forged-revision",
        "x-task17-content-hash": "forged-hash",
        "x-task17-marker": marker,
      },
    }),
    /selected skill instructions/,
  );
});

test("rejects a marker outside the selected lifecycle instruction block", () => {
  const raw = Buffer.from(bodyWithDeveloper(
    `<skill_instructions name="another-skill" source="SKILL.md">${marker}</skill_instructions>`,
  ));
  assert.throws(() => buildNextTurnCapture(raw), /selected skill instructions/);
});

test("rejects reuse of a nonce already accepted by this server run", () => {
  const seenNonces = new Set();
  const raw = Buffer.from(bodyWithDeveloper(
    `<skill_instructions name="Task17 mobile lifecycle">${marker}</skill_instructions>`,
  ));
  buildNextTurnCapture(raw, { seenNonces });
  assert.throws(() => buildNextTurnCapture(raw, { seenNonces }), /nonce was already used/);
});

test("clears stale capture and serves only the same-run request", async () => {
  const root = await mkdtemp(join(tmpdir(), "task17-evidence-"));
  const capturePath = join(root, "capture.json");
  await writeFile(capturePath, '{"request_id":"stale"}');
  const server = startEvidenceServer({ port: 0, capturePath, quiet: true });
  try {
    await once(server, "listening");
    await assert.rejects(readFile(capturePath), /ENOENT/);
    const { port } = server.address();
    const base = `http://127.0.0.1:${port}`;
    assert.equal((await fetch(`${base}/task17-capture?nonce=${nonce}`)).status, 409);

    const raw = bodyWithDeveloper(
      `<skill_instructions name="Task17 mobile lifecycle">${marker}</skill_instructions>`,
    );
    const response = await fetch(`${base}/v1/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: raw,
    });
    assert.equal(response.status, 200);
    assert.equal((await fetch(`${base}/task17-capture?nonce=wrong-request`)).status, 409);
    const host = await (await fetch(`${base}/task17-capture?nonce=${nonce}`)).json();
    const persisted = JSON.parse(await readFile(capturePath, "utf8"));
    assert.deepEqual(host, persisted);
    assert.notEqual(host.request_id, "stale");
    assert.equal(host.raw_request_sha256, createHash("sha256").update(raw).digest("hex"));
  } finally {
    server.close();
    await rm(root, { recursive: true, force: true });
  }
});
