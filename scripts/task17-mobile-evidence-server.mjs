import http from "node:http";

const MAX_REQUEST_BYTES = 1024 * 1024;
const REQUIRED_HEADERS = [
  "x-task17-request-id",
  "x-task17-revision-id",
  "x-task17-content-hash",
  "x-task17-marker",
  "x-task17-user-text",
];

export function buildNextTurnEvidence(headers, requestBody) {
  const values = Object.fromEntries(
    REQUIRED_HEADERS.map((name) => [name, headerValue(headers, name)]),
  );
  for (const [name, value] of Object.entries(values)) {
    if (!value) throw new Error(`missing ${name}`);
  }
  const marker = values["x-task17-marker"];
  if (!JSON.stringify(requestBody).includes(marker)) {
    throw new Error("provider request does not contain the active marker");
  }
  const userText = values["x-task17-user-text"];
  const userBound = Array.isArray(requestBody.input) && requestBody.input.some(
    (item) => item?.role === "user" && contentIncludesText(item.content, userText),
  );
  if (!userBound) throw new Error("provider request does not contain the bound user text");
  return {
    request_id: values["x-task17-request-id"],
    user_text: userText,
    active_revision_id: values["x-task17-revision-id"],
    content_hash: values["x-task17-content-hash"],
    marker,
    request_body: requestBody,
  };
}

function contentIncludesText(content, expected) {
  if (content === expected) return true;
  return Array.isArray(content) && content.some(
    (part) => part?.type === "input_text" && part?.text === expected,
  );
}

export function startEvidenceServer({ port = 18717, host = "127.0.0.1" } = {}) {
  const server = http.createServer(async (request, response) => {
    try {
      if (request.method !== "POST" || !request.url?.endsWith("/responses")) {
        sendJson(response, 404, { error: "not found" });
        return;
      }
      const requestBody = JSON.parse(await readBoundedBody(request));
      const evidence = buildNextTurnEvidence(request.headers, requestBody);
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

function headerValue(headers, name) {
  const value = headers[name];
  return Array.isArray(value) ? value[0] ?? "" : value ?? "";
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
    request.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    request.on("error", reject);
  });
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
