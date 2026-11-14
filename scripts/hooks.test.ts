import { afterAll, describe, expect, test } from "bun:test";
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EVENTS, installHooks, statusOf, uninstallHooks } from "./hooks-config";
import { readSettings, runInstall, runUninstall, UserError } from "./hooks";

const CMD = '"C:/bun/bun.exe" "C:/x/shim/agentwatch-hook.ts"';
const dirs: string[] = [];
afterAll(() =>
  dirs.forEach((d) => rmSync(d, { recursive: true, force: true })),
);
function tmp(): string {
  const d = mkdtempSync(join(tmpdir(), "agentwatch-hooks-"));
  dirs.push(d);
  return d;
}
const read = (p: string) => JSON.parse(readFileSync(p, "utf8"));

const FOREIGN = {
  theme: "dark",
  permissions: { allow: ["Bash(ls)"] },
  hooks: {
    PreToolUse: [
      { matcher: "Bash", hooks: [{ type: "command", command: "lint.sh" }] },
    ],
    Stop: [{ hooks: [{ type: "command", command: "notify.sh" }] }],
    CwdChanged: [{ hooks: [{ type: "command", command: "cwd.sh" }] }],
  },
};

describe("hooks-config", () => {
  test("install into empty settings", () => {
    const out = installHooks({}, CMD) as { hooks: Record<string, unknown[]> };
    expect(Object.keys(out.hooks)).toEqual([...EVENTS]);
    expect(out.hooks.PreToolUse).toEqual([
      {
        matcher: "*",
        hooks: [{ type: "command", command: CMD, async: true, timeout: 5 }],
      },
    ]);
    expect(out.hooks.Stop).toEqual([
      { hooks: [{ type: "command", command: CMD, asyncRewake: true }] },
    ]);
    expect(out.hooks.PermissionRequest).toEqual([
      {
        matcher: "*",
        hooks: [{ type: "command", command: CMD, timeout: 130 }],
      },
    ]);
    for (const e of EVENTS) {
      if (e === "Stop" || e === "PermissionRequest") continue;
      expect(out.hooks[e][0]).toHaveProperty("hooks", [
        { type: "command", command: CMD, async: true, timeout: 5 },
      ]);
    }
    expect(statusOf(out).kinds).toMatchObject({
      PermissionRequest: "sync 130s",
      Stop: "asyncRewake",
      PreToolUse: "async",
    });
    expect(out.hooks.UserPromptSubmit[0]).not.toHaveProperty("matcher");
  });

  test("merge keeps foreign hooks and top-level keys", () => {
    const out = installHooks(structuredClone(FOREIGN), CMD) as typeof FOREIGN;
    expect(out.theme).toBe("dark");
    expect(out.permissions).toEqual(FOREIGN.permissions);
    expect(out.hooks.CwdChanged).toEqual(FOREIGN.hooks.CwdChanged);
    expect(out.hooks.PreToolUse).toHaveLength(2);
    expect(out.hooks.PreToolUse[0]).toEqual(FOREIGN.hooks.PreToolUse[0]);
    expect(out.hooks.Stop).toHaveLength(2);
    expect(out.hooks.Stop[0]).toEqual(FOREIGN.hooks.Stop[0]);
  });

  test("migrates a Phase 1 install without duplicates", () => {
    const old: Record<string, unknown[]> = {};
    for (const e of EVENTS) {
      old[e] = [
        ...(e === "Stop" ? FOREIGN.hooks.Stop : []),
        {
          ...(e === "UserPromptSubmit" || e === "Stop" ? {} : { matcher: "*" }),
          hooks: [{ type: "command", command: CMD, async: true, timeout: 5 }],
        },
      ];
    }
    const out = installHooks({ hooks: old }, CMD) as {
      hooks: Record<string, { hooks: Record<string, unknown>[] }[]>;
    };
    expect(statusOf(out).commands).toEqual([CMD]);
    expect(out.hooks.PermissionRequest).toHaveLength(1);
    expect(out.hooks.PermissionRequest[0].hooks[0]).toEqual({
      type: "command",
      command: CMD,
      timeout: 130,
    });
    expect(out.hooks.Stop).toHaveLength(2); // foreign + ours
    expect(out.hooks.Stop[0]).toEqual(FOREIGN.hooks.Stop[0]);
    expect(out.hooks.Stop[1].hooks[0]).toEqual({
      type: "command",
      command: CMD,
      asyncRewake: true,
    });
    expect(installHooks(structuredClone(out), CMD)).toEqual(out);
  });

  test("install twice is idempotent", () => {
    const once = installHooks(structuredClone(FOREIGN), CMD);
    const twice = installHooks(structuredClone(once), CMD);
    expect(twice).toEqual(once);
    // Even with a changed command (e.g. moved repo), still one entry per event.
    const moved = installHooks(once, '"D:/bun" "D:/agentwatch-hook.ts"');
    expect(statusOf(moved).commands).toHaveLength(1);
    expect((moved.hooks as Record<string, unknown[]>).PreToolUse).toHaveLength(
      2,
    );
  });

  test("uninstall removes only ours, round-trips to the original", () => {
    const installed = installHooks(structuredClone(FOREIGN), CMD);
    const { settings, removed } = uninstallHooks(installed);
    expect(removed).toBe(EVENTS.length);
    expect(settings).toEqual(FOREIGN);
  });

  test("uninstall drops emptied hooks but keeps a pre-existing empty one", () => {
    const installed = installHooks({ a: 1 }, CMD);
    expect(uninstallHooks(installed).settings).toEqual({ a: 1 });
    const untouched = { hooks: {} };
    expect(uninstallHooks(untouched)).toEqual({
      settings: untouched,
      removed: 0,
    });
  });

  test("a mixed group only loses our hook", () => {
    const s = {
      hooks: {
        Stop: [
          {
            hooks: [
              { type: "command", command: "mine.sh" },
              { type: "command", command: CMD },
            ],
          },
        ],
      },
    };
    expect(uninstallHooks(s).settings).toEqual({
      hooks: { Stop: [{ hooks: [{ type: "command", command: "mine.sh" }] }] },
    });
  });
});

describe("hooks CLI file handling", () => {
  test("install creates a missing file; no backup needed", () => {
    const d = tmp();
    const p = join(d, "nested", "settings.json");
    runInstall(p, CMD);
    expect(statusOf(read(p)).events).toEqual([...EVENTS]);
    expect(readdirSync(join(d, "nested"))).toEqual(["settings.json"]);
  });

  test("install backs up first and uninstall restores foreign content", () => {
    const d = tmp();
    const p = join(d, "settings.json");
    writeFileSync(p, JSON.stringify(FOREIGN));
    runInstall(p, CMD);
    const backups = readdirSync(d).filter((f) =>
      /^settings\.json\.agentwatch-backup-\d{14}$/.test(f),
    );
    expect(backups).toHaveLength(1);
    expect(read(join(d, backups[0]))).toEqual(FOREIGN);
    expect(readdirSync(d).some((f) => f.includes("tmp"))).toBe(false);

    runUninstall(p);
    expect(read(p)).toEqual(FOREIGN);
  });

  test("invalid JSON aborts without writing anything", () => {
    const d = tmp();
    const p = join(d, "settings.json");
    writeFileSync(p, "{ not json");
    expect(() => runInstall(p, CMD)).toThrow(UserError);
    expect(() => readSettings(p)).toThrow(UserError);
    expect(readFileSync(p, "utf8")).toBe("{ not json");
    expect(readdirSync(d)).toEqual(["settings.json"]);
  });

  test("uninstall with nothing installed changes and backs up nothing", () => {
    const d = tmp();
    const p = join(d, "settings.json");
    writeFileSync(p, JSON.stringify(FOREIGN));
    runUninstall(p);
    expect(readdirSync(d)).toEqual(["settings.json"]);
    expect(existsSync(join(d, "missing.json"))).toBe(false);
  });

  test("CLI end to end via --settings", async () => {
    const p = join(tmp(), "settings.json");
    const env = { ...process.env, AGENTWATCH_DIR: tmp() };
    const run = async (cmd: string) => {
      const proc = Bun.spawn(
        [
          process.execPath,
          join(import.meta.dir, "hooks.ts"),
          cmd,
          "--settings",
          p,
        ],
        { stdout: "pipe", stderr: "pipe", env },
      );
      const out = await new Response(proc.stdout).text();
      return { out, code: await proc.exited };
    };
    expect((await run("install")).code).toBe(0);
    const st = await run("status");
    expect(st.out).toContain("Installed: yes");
    expect(st.out).toContain("agentwatch-hook");
    expect(st.out).toContain("widget is not running");
    expect((await run("uninstall")).code).toBe(0);
    expect((await run("status")).out).toContain("Installed: no");
  });
});
