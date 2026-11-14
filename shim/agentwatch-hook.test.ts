import { afterAll, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SHIM = join(import.meta.dir, "agentwatch-hook.ts");
const tempDirs: string[] = [];
afterAll(() => {
  for (const d of tempDirs) rmSync(d, { recursive: true, force: true });
});

function tempDir(endpoint?: unknown): string {
  const dir = mkdtempSync(join(tmpdir(), "agentwatch-shim-"));
  tempDirs.push(dir);
  if (endpoint !== undefined) {
    writeFileSync(join(dir, "endpoint.json"), JSON.stringify(endpoint));
  }
  return dir;
}

// The shim reads these three variables; drop them so a test run inside Claude
// Code itself does not leak real values into the forwarded body.
function cleanEnv(): Record<string, string | undefined> {
  const env = { ...process.env };
  delete env.CLAUDE_CODE_ENTRYPOINT;
  delete env.CLAUDE_PID;
  delete env.VSCODE_PID;
  return env;
}

/** Body as the server saw it, with the `agentwatch` meta key removed. */
function stripMeta(body: string): string {
  const v = JSON.parse(body) as Record<string, unknown>;
  delete v.agentwatch;
  return JSON.stringify(v);
}

async function runShim(
  dir: string,
  stdin: string,
  extraEnv: Record<string, string> = {},
) {
  const start = performance.now();
  const proc = Bun.spawn([process.execPath, SHIM], {
    env: { ...cleanEnv(), AGENTWATCH_DIR: dir, ...extraEnv },
    stdin: new TextEncoder().encode(stdin),
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  const ms = Math.round(performance.now() - start);
  console.log(`  [timing] ${ms} ms`);
  return { stdout, stderr, code, ms };
}

function expectSilentSuccess(r: {
  stdout: string;
  stderr: string;
  code: number;
}) {
  expect(r.code).toBe(0);
  expect(r.stdout).toBe("");
  expect(r.stderr).toBe("");
}

const EVENT = JSON.stringify({
  hook_event_name: "PreToolUse",
  session_id: "abc",
});

describe("agentwatch-hook shim", () => {
  test("no endpoint file", async () => {
    expectSilentSuccess(await runShim(tempDir(), EVENT));
  });

  test("invalid endpoint file", async () => {
    const dir = tempDir();
    writeFileSync(join(dir, "endpoint.json"), "not json{");
    expectSilentSuccess(await runShim(dir, EVENT));
  });

  test("endpoint points at a closed port", async () => {
    // Bind then close a server to get a port that is very likely unused.
    const s = Bun.serve({ port: 0, fetch: () => new Response("{}") });
    const port = s.port;
    await s.stop(true);
    const dir = tempDir({ port, token: "t", pid: 1 });
    expectSilentSuccess(await runShim(dir, EVENT));
  });

  test("wrong token gets 401", async () => {
    const s = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch: () => new Response("nope", { status: 401 }),
    });
    try {
      const dir = tempDir({ port: s.port, token: "bad", pid: 1 });
      expectSilentSuccess(await runShim(dir, EVENT));
    } finally {
      await s.stop(true);
    }
  });

  test("hung server: exits within the deadline", async () => {
    const s = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch: () => new Promise<Response>(() => {}), // never responds
    });
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, EVENT);
      expectSilentSuccess(r);
      expect(r.ms).toBeLessThan(1200); // includes Bun process startup
    } finally {
      await s.stop(true);
    }
  });

  test("invalid stdin", async () => {
    let hits = 0;
    const s = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      fetch: () => {
        hits++;
        return new Response("bad", { status: 400 });
      },
    });
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      expectSilentSuccess(await runShim(dir, "this is not json"));
      expectSilentSuccess(await runShim(dir, ""));
      expect(hits).toBeLessThanOrEqual(1); // empty stdin is never sent
    } finally {
      await s.stop(true);
    }
  });

  test("happy path delivers exact body and bearer token", async () => {
    type Got = {
      body: string;
      auth: string | null;
      method: string;
      path: string;
    };
    let got = null as Got | null; // assigned inside the server callback
    const s = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      async fetch(req) {
        got = {
          body: await req.text(),
          auth: req.headers.get("authorization"),
          method: req.method,
          path: new URL(req.url).pathname,
        };
        return new Response("{}");
      },
    });
    try {
      const dir = tempDir({ port: s.port, token: "secret-token", pid: 1 });
      const body = JSON.stringify({
        hook_event_name: "PreToolUse",
        note: "ünï\n",
      });
      expectSilentSuccess(await runShim(dir, body));
      expect(got).not.toBeNull();
      expect(stripMeta((got as unknown as Got).body)).toBe(body);
      expect(got).toEqual({
        body: (got as unknown as Got).body,
        auth: "Bearer secret-token",
        method: "POST",
        path: "/event",
      });
    } finally {
      await s.stop(true);
    }
  });
});

const PERM = JSON.stringify({
  hook_event_name: "PermissionRequest",
  session_id: "abc",
  tool_name: "Bash",
  tool_input: { command: "ls" },
});
const STOP = JSON.stringify({
  hook_event_name: "Stop",
  session_id: "abc",
});
const ALLOW_JSON =
  '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}';
const DENY_JSON =
  '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from agentwatch"}}}';
const SHORT = {
  AGENTWATCH_PERMISSION_TIMEOUT_MS: "300",
  AGENTWATCH_REPLY_TIMEOUT_MS: "300",
};

function serve(handler: (req: Request) => Response | Promise<Response>) {
  return Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: handler });
}

async function closedPortDir(): Promise<string> {
  const s = serve(() => new Response("{}"));
  const port = s.port;
  await s.stop(true);
  return tempDir({ port, token: "t", pid: 1 });
}

type Seen = { body: string; auth: string | null; path: string };

describe("PermissionRequest route", () => {
  test("allow prints exact decision JSON", async () => {
    const s = serve(() => Response.json({ decision: "allow" }));
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, PERM);
      expect(r.code).toBe(0);
      expect(r.stdout).toBe(ALLOW_JSON);
      expect(r.stderr).toBe("");
    } finally {
      await s.stop(true);
    }
  });

  test("deny prints exact decision JSON", async () => {
    const s = serve(() => Response.json({ decision: "deny" }));
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, PERM);
      expect(r.code).toBe(0);
      expect(r.stdout).toBe(DENY_JSON);
      expect(r.stderr).toBe("");
    } finally {
      await s.stop(true);
    }
  });

  test("none, non-200, bad JSON and unknown values print nothing", async () => {
    const makers: (() => Response)[] = [
      () => Response.json({ decision: "none" }),
      () => new Response("nope", { status: 500 }),
      () => new Response("not json"),
      () => Response.json({ decision: "maybe" }),
    ];
    for (const make of makers) {
      const s = serve(make);
      try {
        const dir = tempDir({ port: s.port, token: "t", pid: 1 });
        expectSilentSuccess(await runShim(dir, PERM));
      } finally {
        await s.stop(true);
      }
    }
  });

  test("hung server: silent exit 0 at the fetch timeout", async () => {
    const s = serve(() => new Promise<Response>(() => {}));
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, PERM, SHORT);
      expectSilentSuccess(r);
      expect(r.ms).toBeGreaterThanOrEqual(250); // waited, not fail-fast
      expect(r.ms).toBeLessThan(2500);
    } finally {
      await s.stop(true);
    }
  });

  test("long poll is not cut off by the 500 ms event deadline", async () => {
    const s = serve(async () => {
      await Bun.sleep(900);
      return Response.json({ decision: "allow" });
    });
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, PERM, {
        AGENTWATCH_PERMISSION_TIMEOUT_MS: "5000",
      });
      expect(r.stdout).toBe(ALLOW_JSON);
      expect(r.code).toBe(0);
    } finally {
      await s.stop(true);
    }
  });

  test("closed port and missing endpoint: silent and quick", async () => {
    const a = await runShim(await closedPortDir(), PERM);
    expectSilentSuccess(a);
    expect(a.ms).toBeLessThan(1500);
    const b = await runShim(tempDir(), PERM);
    expectSilentSuccess(b);
    expect(b.ms).toBeLessThan(1500);
  });

  test("forwards body, bearer token, method and path", async () => {
    let got = null as Seen | null;
    const s = serve(async (req) => {
      expect(req.method).toBe("POST");
      got = {
        body: await req.text(),
        auth: req.headers.get("authorization"),
        path: new URL(req.url).pathname,
      };
      return Response.json({ decision: "none" });
    });
    try {
      const dir = tempDir({ port: s.port, token: "tok-p", pid: 1 });
      expectSilentSuccess(await runShim(dir, PERM));
      expect(stripMeta((got as unknown as Seen).body)).toBe(PERM);
      expect(got).toEqual({
        body: (got as unknown as Seen).body,
        auth: "Bearer tok-p",
        path: "/permission",
      });
    } finally {
      await s.stop(true);
    }
  });
});

describe("Stop route", () => {
  test("reply exits 2 with exact stderr and empty stdout", async () => {
    const s = serve(() => Response.json({ reply: "line one\nline two" }));
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, STOP);
      expect(r.code).toBe(2);
      expect(r.stdout).toBe("");
      expect(r.stderr).toBe(
        "The user replied from the agentwatch widget:\nline one\nline two",
      );
    } finally {
      await s.stop(true);
    }
  });

  test("null, empty and malformed replies exit 0 silently", async () => {
    const makers: (() => Response)[] = [
      () => Response.json({ reply: null }),
      () => Response.json({ reply: "" }),
      () => Response.json({}),
      () => new Response("bad json"),
      () => new Response("err", { status: 500 }),
    ];
    for (const make of makers) {
      const s = serve(make);
      try {
        const dir = tempDir({ port: s.port, token: "t", pid: 1 });
        expectSilentSuccess(await runShim(dir, STOP));
      } finally {
        await s.stop(true);
      }
    }
  });

  test("hung server: silent exit 0 at the fetch timeout", async () => {
    const s = serve(() => new Promise<Response>(() => {}));
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const r = await runShim(dir, STOP, SHORT);
      expectSilentSuccess(r);
      expect(r.ms).toBeGreaterThanOrEqual(250);
      expect(r.ms).toBeLessThan(2500);
    } finally {
      await s.stop(true);
    }
  });

  test("closed port and missing endpoint: silent and quick", async () => {
    const a = await runShim(await closedPortDir(), STOP);
    expectSilentSuccess(a);
    expect(a.ms).toBeLessThan(1500);
    const b = await runShim(tempDir(), STOP);
    expectSilentSuccess(b);
    expect(b.ms).toBeLessThan(1500);
  });

  test("forwards body, bearer token, method and path", async () => {
    let got = null as Seen | null;
    const s = serve(async (req) => {
      expect(req.method).toBe("POST");
      got = {
        body: await req.text(),
        auth: req.headers.get("authorization"),
        path: new URL(req.url).pathname,
      };
      return Response.json({ reply: null });
    });
    try {
      const dir = tempDir({ port: s.port, token: "tok-s", pid: 1 });
      expectSilentSuccess(await runShim(dir, STOP));
      expect(stripMeta((got as unknown as Seen).body)).toBe(STOP);
      expect(got).toEqual({
        body: (got as unknown as Seen).body,
        auth: "Bearer tok-s",
        path: "/stop",
      });
    } finally {
      await s.stop(true);
    }
  });
});

describe("generic routing", () => {
  test("other events go to /event and any response is ignored", async () => {
    let path = "";
    const s = serve((req) => {
      path = new URL(req.url).pathname;
      return Response.json({ decision: "allow", reply: "x" });
    });
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      const body = JSON.stringify({ hook_event_name: "PreToolUse" });
      expectSilentSuccess(await runShim(dir, body));
      expect(path).toBe("/event");
    } finally {
      await s.stop(true);
    }
  });

  test("invalid JSON stdin is a generic event: silent exit 0", async () => {
    const s = serve(() => new Response("{}"));
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      expectSilentSuccess(await runShim(dir, "{ nope", SHORT));
    } finally {
      await s.stop(true);
    }
  });
});

describe("agentwatch meta", () => {
  async function bodyFor(stdin: string, env: Record<string, string>) {
    let body = "";
    const s = serve(async (req) => {
      body = await req.text();
      return new Response("{}");
    });
    try {
      const dir = tempDir({ port: s.port, token: "t", pid: 1 });
      expectSilentSuccess(await runShim(dir, stdin, env));
    } finally {
      await s.stop(true);
    }
    return body;
  }

  test("env vars are forwarded under the agentwatch key", async () => {
    const body = await bodyFor(EVENT, {
      CLAUDE_CODE_ENTRYPOINT: "claude-vscode",
      CLAUDE_PID: "1234",
      VSCODE_PID: "5678",
    });
    const v = JSON.parse(body);
    expect(v.hook_event_name).toBe("PreToolUse");
    expect(v.session_id).toBe("abc");
    expect(v.agentwatch).toEqual({
      entrypoint: "claude-vscode",
      claude_pid: 1234,
      vscode_pid: 5678,
    });
  });

  test("unset or invalid env vars become null", async () => {
    const v = JSON.parse(await bodyFor(EVENT, { CLAUDE_PID: "abc" }));
    expect(v.agentwatch).toEqual({
      entrypoint: null,
      claude_pid: null,
      vscode_pid: null,
    });
  });

  test("a body that is not a JSON object is forwarded unchanged", async () => {
    expect(await bodyFor("[1,2]", {})).toBe("[1,2]");
    expect(await bodyFor("not json", {})).toBe("not json");
  });
});
