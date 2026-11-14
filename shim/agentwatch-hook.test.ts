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

async function runShim(dir: string, stdin: string) {
  const start = performance.now();
  const proc = Bun.spawn([process.execPath, SHIM], {
    env: { ...process.env, AGENTWATCH_DIR: dir },
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

const EVENT = JSON.stringify({ hook_event_name: "Stop", session_id: "abc" });

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
      const body = JSON.stringify({ hook_event_name: "Stop", note: "ünï\n" });
      expectSilentSuccess(await runShim(dir, body));
      expect(got).toEqual({
        body,
        auth: "Bearer secret-token",
        method: "POST",
        path: "/event",
      });
    } finally {
      await s.stop(true);
    }
  });
});
