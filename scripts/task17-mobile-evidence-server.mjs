import http from "node:http";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, open, rename } from "node:fs/promises";
import { dirname, join } from "node:path";

const MAX_REQUEST_BYTES = 1024 * 1024;
const ACTIVE_MARKER = "TASK17_UI_ACTIVE_SKILL_EVIDENCE";
const SELECTED_SKILL = "Task17 mobile lifecycle";
const NONCE_PATTERN = /\bnonce:([a-zA-Z0-9][a-zA-Z0-9._-]{7,127})\b/;

export function buildNextTurnCapture(rawBytes, { requestId = randomUUID() } = {}) {
  const raw = Buffer.isBuffer(rawBytes) ? rawBytes : Buffer.from(rawBytes);
  if (raw.length === 0 || raw.length > MAX_REQUEST_BYTES) {
    throw new Error("provider request exceeds evidence limit");
  }
  const requestBody = JSON.parse(raw.toString("utf8"));
  const developerText = requestContentForRole(requestBody, "developer").join("\n");
  const selected = selectedInstructionBodies(developerText, SELECTED_SKILL);
  if (!selected.some((body) => body.includes(ACTIVE_MARKER))) {
    throw new Error("provider request does not contain the marker in selected skill instructions");
  }
  const userText = requestContentForRole(requestBody, "user").at(-1) ?? "";
  const nonce = userText.match(NONCE_PATTERN)?.[1];
  if (!nonce || !userText.includes("task17-mobile")) {
    throw new Error("provider request does not contain the bound UI nonce");
  }
  return {
    request_id: requestId,
    capture_nonce: nonce,
    user_text: userText,
    marker: ACTIVE_MARKER,
    marker_location: "skill_instructions",
    raw_request_sha256: createHash("sha256").update(raw).digest("hex"),
    request_body: requestBody,
  };
}

function requestContentForRole(requestBody, role) {
  if (!Array.isArray(requestBody.input)) return [];
  return requestBody.input
    .filter((item) => item?.role === role)
    .flatMap((item) => contentText(item.content));
}

function contentText(content) {
  if (typeof content === "string") return [content];
  if (!Array.isArray(content)) return [];
  return content
    .filter((part) => part?.type === "input_text" && typeof part.text === "string")
    .map((part) => part.text);
}

function selectedInstructionBodies(text, expectedName) {
  const bodies = [];
  const blocks = text.matchAll(/<skill_instructions\s+([^>]*)>([\s\S]*?)<\/skill_instructions>/g);
  for (const block of blocks) {
    const name = block[1].match(/(?:^|\s)name="([^"]+)"(?:\s|$)/)?.[1];
    if (name === expectedName) bodies.push(block[2]);
  }
  return bodies;
}

export function startEvidenceServer({
  port = 18717,
  host = "127.0.0.1",
  capturePath = process.env.TASK17_EVIDENCE_CAPTURE_PATH,
} = {}) {
  const server = http.createServer(async (request, response) => {
    try {
      if (request.method !== "POST" || !request.url?.endsWith("/responses")) {
        sendJson(response, 404, { error: "not found" });
        return;
      }
      const rawBytes = await readBoundedBody(request);
      const evidence = buildNextTurnCapture(rawBytes);
      if (capturePath) await writeCaptureAtomically(capturePath, evidence);
      sendJson(response, 200, {
        output: [{
          type: "message",
          content: [{ type: "output_text", text: JSON.stringify(evidence) }],
        }],
      });
      server.close();
    } catch (error) {
      sendJson(response, 400, { error: error instanceof Error ? error.message : "invalid request" });
    }
  });
  server.requestTimeout = 30_000;
  server.listen(port, host, () => {
    const address = server.address();
    const activePort = typeof address === "object" && address ? address.port : port;
    process.stdout.write(`task17 mobile evidence server listening on http://${host}:${activePort}/v1\n`);
  });
  return server;
}

function readBoundedBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    request.on("data", (chunk) => {
      size += chunk.length;
      if (size > MAX_REQUEST_BYTES) {
        reject(new Error("provider request exceeds evidence limit"));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => resolve(Buffer.concat(chunks)));
    request.on("error", reject);
  });
}

async function writeCaptureAtomically(target, capture) {
  const parent = dirname(target);
  await mkdir(parent, { recursive: true });
  const temporary = join(parent, `.${randomUUID()}.capture.tmp`);
  const file = await open(temporary, "wx", 0o600);
  try {
    await file.writeFile(`${JSON.stringify(capture, null, 2)}\n`, "utf8");
    await file.sync();
  } finally {
    await file.close();
  }
  await rename(temporary, target);
  const directory = await open(parent, "r");
  try {
    await directory.sync();
  } finally {
    await directory.close();
  }
}

function sendJson(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    connection: "close",
  });
  response.end(body);
}

if (process.argv[1]?.endsWith("task17-mobile-evidence-server.mjs")) {
  const port = Number.parseInt(process.argv[2] ?? process.env.TASK17_EVIDENCE_PORT ?? "18717", 10);
  startEvidenceServer({ port });
}
