// agentwatch hook shim (ADR-5). Claude Code runs this as an async command hook:
// event JSON arrives on stdin and we forward it to the widget over localhost.
//
// FAIL OPEN: the widget being closed (or anything else going wrong) must be
// invisible to Claude Code. So this script
//  - never writes to stdout or stderr (stdout can be parsed as a hook decision),
//  - always exits 0 (exit 2 blocks on some events; other codes show hook errors),
//  - never prints a decision in Phase 1 (the widget is read-only),
//  - gives up after a hard deadline, because async hooks have no enforced timeout.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const MAX_BODY_BYTES = 1024 * 1024; // matches the server's 1 MB limit
const FETCH_TIMEOUT_MS = 400;
const HARD_DEADLINE_MS = 500;

const done = (): never => process.exit(0);

// Hard deadline: if stdin never closes or fetch stalls, leave anyway.
// unref'd so it never keeps the process alive on its own.
setTimeout(done, HARD_DEADLINE_MS).unref();

// Last-resort handlers: swallow anything unexpected and still exit 0.
process.on("uncaughtException", done);
process.on("unhandledRejection", done);

async function readStdin(): Promise<Uint8Array | null> {
  const chunks: Uint8Array[] = [];
  let total = 0;
  for await (const chunk of Bun.stdin.stream()) {
    total += chunk.byteLength;
    if (total > MAX_BODY_BYTES) return null; // too big: the server would 413 it
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

function readEndpoint(): { port: number; token: string } | null {
  const dir = process.env.AGENTWATCH_DIR || join(homedir(), ".agentwatch");
  try {
    const ep: unknown = JSON.parse(
      readFileSync(join(dir, "endpoint.json"), "utf8"),
    );
    if (typeof ep !== "object" || ep === null) return null;
    const { port, token } = ep as { port?: unknown; token?: unknown };
    if (typeof port !== "number" || !Number.isInteger(port)) return null;
    if (port < 1 || port > 65535) return null;
    if (typeof token !== "string" || token === "") return null;
    return { port, token };
  } catch {
    return null; // missing or invalid endpoint file: widget is not running
  }
}

async function main(): Promise<void> {
  // Check the endpoint first so a closed widget costs no stdin work or network.
  const endpoint = readEndpoint();
  if (!endpoint) return;
  const body = await readStdin();
  if (!body || body.byteLength === 0) return;
  // The response is deliberately ignored: Phase 1 never relays a decision.
  await fetch(`http://127.0.0.1:${endpoint.port}/event`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${endpoint.token}`,
      "Content-Type": "application/json",
    },
    body: new TextDecoder().decode(body),
    signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
  });
}

try {
  await main();
} catch {
  // Fail open: refused connection, timeout, bad stdin... all silently ignored.
}
done();
