// agentwatch hook shim (ADR-5). Claude Code runs this as a command hook: event
// JSON arrives on stdin and we forward it to the widget over localhost.
//
// FAIL OPEN: the widget being closed (or anything else going wrong) must be
// invisible to Claude Code. So this script
//  - always drains stdin (exiting early could give Claude Code a broken pipe),
//  - writes to stdout ONLY the PermissionRequest decision JSON (stdout can be
//    parsed as a hook decision, so nothing else may ever appear there),
//  - writes to stderr ONLY the Stop reply text,
//  - exits 0 in every case except a Stop reply. Exit code 2 is how an
//    asyncRewake Stop hook wakes Claude with the stderr text, so it is the one
//    and only non-zero exit; any other non-zero code would show a hook error,
//  - gives up after a hard deadline, because hooks without a timeout never end.
//
// Routes (by hook_event_name):
//  - PermissionRequest -> POST /permission (long poll, up to ~2 min)
//  - Stop              -> POST /stop       (long poll, up to ~30 min)
//  - everything else   -> POST /event      (fire and forget, ~0.5 s)
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const MAX_BODY_BYTES = 1024 * 1024; // matches the server's 1 MB limit

// Env overrides exist for tests only; production uses the defaults.
function envMs(name: string, fallback: number): number {
  const n = Number(process.env[name]);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

const EVENT_FETCH_MS = 400;
const EVENT_DEADLINE_MS = 500;
// Server holds /permission up to 120 s; the hook config timeout is 130 s.
const PERMISSION_FETCH_MS = envMs("AGENTWATCH_PERMISSION_TIMEOUT_MS", 125_000);
// Server holds /stop up to 1800 s (reply_max_wait_secs) plus a small margin.
const STOP_FETCH_MS = envMs("AGENTWATCH_REPLY_TIMEOUT_MS", 1_805_000);
// The hard deadline must never fire before the fetch timeout, so it is longer.
const DEADLINE_MARGIN_MS = 1_000;

const done = (code = 0): never => process.exit(code);

// Initial hard deadline covers a stdin that never closes. It is replaced once
// we know which route we are on. unref'd so it never keeps the process alive.
let deadline = setTimeout(() => done(0), EVENT_DEADLINE_MS);
deadline.unref();
function setDeadline(ms: number): void {
  clearTimeout(deadline);
  deadline = setTimeout(() => done(0), ms);
  deadline.unref();
}

// Last-resort handlers: swallow anything unexpected and still exit 0.
process.on("uncaughtException", () => done(0));
process.on("unhandledRejection", () => done(0));

// Always read stdin to the end, even when we will discard it. `keep: false`
// drains without buffering.
async function readStdin(keep: boolean): Promise<Uint8Array | null> {
  const chunks: Uint8Array[] = [];
  let total = 0;
  for await (const chunk of Bun.stdin.stream()) {
    total += chunk.byteLength;
    if (keep && total <= MAX_BODY_BYTES) chunks.push(chunk);
  }
  if (!keep || total > MAX_BODY_BYTES) return null; // too big: server would 413
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

/** Event name from the hook JSON; "" when it cannot be parsed. */
function eventName(text: string): string {
  try {
    const v: unknown = JSON.parse(text);
    if (typeof v === "object" && v !== null) {
      const n = (v as { hook_event_name?: unknown }).hook_event_name;
      if (typeof n === "string") return n;
    }
  } catch {
    // not JSON: treated as a generic event
  }
  return "";
}

/**
 * Adds a top-level `agentwatch` key with facts only the hook's environment
 * knows (the payload itself has none of them). Bodies that are not a JSON
 * object are forwarded unchanged. Never throws.
 */
function withMeta(text: string): string {
  try {
    const v: unknown = JSON.parse(text);
    if (typeof v !== "object" || v === null || Array.isArray(v)) return text;
    const meta = {
      entrypoint: process.env.CLAUDE_CODE_ENTRYPOINT || null,
      claude_pid: Number(process.env.CLAUDE_PID) || null,
      vscode_pid: Number(process.env.VSCODE_PID) || null,
    };
    return JSON.stringify({
      ...(v as Record<string, unknown>),
      agentwatch: meta,
    });
  } catch {
    return text;
  }
}

async function post(
  endpoint: { port: number; token: string },
  path: string,
  text: string,
  timeoutMs: number,
): Promise<Response> {
  return fetch(`http://127.0.0.1:${endpoint.port}${path}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${endpoint.token}`,
      "Content-Type": "application/json",
    },
    body: text,
    signal: AbortSignal.timeout(timeoutMs),
  });
}

/** Parsed JSON object of a 200 response, else null. */
async function jsonOf(res: Response): Promise<Record<string, unknown> | null> {
  if (res.status !== 200) return null;
  try {
    const v: unknown = await res.json();
    return typeof v === "object" && v !== null
      ? (v as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/** Write and wait for the flush, so exiting right after cannot lose it. */
function write(stream: NodeJS.WriteStream, text: string): Promise<void> {
  return new Promise((resolve) => stream.write(text, () => resolve()));
}

function permissionOutput(decision: Record<string, unknown>): string | null {
  if (decision.decision === "allow") {
    return JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PermissionRequest",
        decision: { behavior: "allow" },
      },
    });
  }
  if (decision.decision === "deny") {
    return JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PermissionRequest",
        decision: { behavior: "deny", message: "Denied from agentwatch" },
      },
    });
  }
  return null; // "none" or anything else: let the terminal prompt decide
}

async function main(): Promise<number> {
  // Check the endpoint first so a closed widget costs no network; stdin is
  // still drained (see readStdin).
  const endpoint = readEndpoint();
  const body = await readStdin(endpoint !== null);
  if (!endpoint) return 0;
  if (!body || body.byteLength === 0) return 0;
  const text = withMeta(new TextDecoder().decode(body));
  // eventName() reads only hook_event_name, which withMeta leaves untouched.

  switch (eventName(text)) {
    case "PermissionRequest": {
      setDeadline(PERMISSION_FETCH_MS + DEADLINE_MARGIN_MS);
      const res = await post(
        endpoint,
        "/permission",
        text,
        PERMISSION_FETCH_MS,
      );
      const json = await jsonOf(res);
      const out = json ? permissionOutput(json) : null;
      if (out) await write(process.stdout, out);
      return 0;
    }
    case "Stop": {
      setDeadline(STOP_FETCH_MS + DEADLINE_MARGIN_MS);
      const res = await post(endpoint, "/stop", text, STOP_FETCH_MS);
      const json = await jsonOf(res);
      const reply = json?.reply;
      if (typeof reply === "string" && reply.trim() !== "") {
        await write(
          process.stderr,
          `The user replied from the agentwatch widget:\n${reply}`,
        );
        return 2; // the only non-zero exit: wakes Claude (asyncRewake)
      }
      return 0;
    }
    default:
      // The response is deliberately ignored for plain events.
      await post(endpoint, "/event", text, EVENT_FETCH_MS);
      return 0;
  }
}

let code = 0;
try {
  code = await main();
} catch {
  // Fail open: refused connection, timeout, bad stdin... all silently ignored.
}
done(code);
