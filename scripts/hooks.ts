// CLI: bun run hooks:install | hooks:uninstall | hooks:status [--settings <path>]
// Target file: --settings, else $CLAUDE_SETTINGS_PATH, else ~/.claude/settings.json.
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  buildCommand,
  installHooks,
  statusOf,
  uninstallHooks,
} from "./hooks-config";

export class UserError extends Error {}

export function defaultSettingsPath(): string {
  return (
    process.env.CLAUDE_SETTINGS_PATH ||
    join(homedir(), ".claude", "settings.json")
  );
}

/** Returns {} for a missing file; throws UserError if the JSON is invalid. */
export function readSettings(path: string): Record<string, unknown> {
  if (!existsSync(path)) return {};
  const text = readFileSync(path, "utf8").replace(/^\u{FEFF}/u, "");
  if (text.trim() === "") return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (e) {
    throw new UserError(
      `${path} is not valid JSON (${(e as Error).message}). Nothing was changed; fix or remove the file first.`,
    );
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new UserError(
      `${path} does not contain a JSON object. Nothing was changed.`,
    );
  }
  return parsed as Record<string, unknown>;
}

function timestamp(d = new Date()): string {
  return d.toISOString().replace(/\D/g, "").slice(0, 14); // YYYYMMDDHHMMSS (UTC)
}

/** Copies the file next to itself; returns the backup path (null if no file). */
export function backup(path: string): string | null {
  if (!existsSync(path)) return null;
  const dest = `${path}.agentwatch-backup-${timestamp()}`;
  copyFileSync(path, dest);
  return dest;
}

/** Write to a temp file in the same folder, then rename over the target. */
export function writeAtomic(path: string, data: unknown): void {
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.agentwatch-tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify(data, null, 2) + "\n");
  renameSync(tmp, path);
}

export function defaultCommand(): string {
  const shim = resolve(import.meta.dir, "..", "shim", "agentwatch-hook.ts");
  return buildCommand(process.execPath, shim);
}

export function runInstall(path: string, command = defaultCommand()): string[] {
  const current = readSettings(path);
  const next = installHooks(current, command);
  if (JSON.stringify(next) === JSON.stringify(current) && existsSync(path)) {
    return [`Already installed and up to date in ${path}`];
  }
  const b = backup(path);
  writeAtomic(path, next);
  return [
    ...(b ? [`Backup: ${b}`] : []),
    `Installed agentwatch hooks in ${path}`,
    `Command: ${command}`,
  ];
}

export function runUninstall(path: string): string[] {
  if (!existsSync(path)) return [`No settings file at ${path}; nothing to do`];
  const { settings, removed } = uninstallHooks(readSettings(path));
  if (removed === 0) return [`No agentwatch hooks found in ${path}`];
  const b = backup(path);
  writeAtomic(path, settings);
  return [
    ...(b ? [`Backup: ${b}`] : []),
    `Removed ${removed} agentwatch hook(s) from ${path}`,
  ];
}

export function runStatus(path: string): string[] {
  const { events, commands, kinds } = statusOf(readSettings(path));
  const dir = process.env.AGENTWATCH_DIR || join(homedir(), ".agentwatch");
  const endpoint = join(dir, "endpoint.json");
  return [
    `Settings: ${path}`,
    events.length
      ? `Installed: yes (${events.length} events: ${events.join(", ")})`
      : "Installed: no",
    ...events.map((e) => `  ${e}: ${kinds[e]}`),
    ...commands.map((c) => `Command: ${c}`),
    existsSync(endpoint)
      ? `Widget: endpoint file present (${endpoint}); widget is probably running`
      : `Widget: no endpoint file (${endpoint}); widget is not running`,
  ];
}

function main(argv: string[]): number {
  const [cmd, ...rest] = argv;
  const i = rest.indexOf("--settings");
  const path = resolve(
    i >= 0 && rest[i + 1] ? rest[i + 1] : defaultSettingsPath(),
  );
  try {
    const lines =
      cmd === "install"
        ? runInstall(path)
        : cmd === "uninstall"
          ? runUninstall(path)
          : cmd === "status"
            ? runStatus(path)
            : null;
    if (!lines) {
      console.error(
        "Usage: bun run scripts/hooks.ts <install|uninstall|status> [--settings <path>]",
      );
      return 2;
    }
    console.log(lines.join("\n"));
    return 0;
  } catch (e) {
    console.error(`Error: ${(e as Error).message}`);
    return 1;
  }
}

if (import.meta.main) process.exit(main(process.argv.slice(2)));
