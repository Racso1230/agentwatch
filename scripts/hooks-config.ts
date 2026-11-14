// Pure merge/unmerge logic for Claude Code's settings.json hooks.
// No file or network access here, so it is easy to unit test.

export const MARKER = "agentwatch-hook";

/** Events agentwatch listens to (SubagentStop etc. are intentionally absent). */
export const EVENTS = [
  "SessionStart",
  "SessionEnd",
  "UserPromptSubmit",
  "PreToolUse",
  "PermissionRequest",
  "PostToolUse",
  "PostToolUseFailure",
  "Notification",
  "Stop",
  "StopFailure",
] as const;

/** Events with no matcher support (docs/hooks-reference.md 1.1). */
const NO_MATCHER = new Set<string>(["UserPromptSubmit", "Stop"]);

type Json = Record<string, unknown>;

export class SettingsShapeError extends Error {}

const isObject = (v: unknown): v is Json =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const isOurs = (h: unknown): boolean =>
  isObject(h) && typeof h.command === "string" && h.command.includes(MARKER);

/** Forward slashes work for both Git Bash and cmd on Windows. */
export function buildCommand(bunPath: string, shimPath: string): string {
  const fwd = (p: string) => p.replaceAll("\\", "/");
  return `"${fwd(bunPath)}" "${fwd(shimPath)}"`;
}

function groupsOf(hooks: Json, event: string): Json[] {
  const groups = hooks[event];
  if (groups === undefined) return [];
  if (!Array.isArray(groups) || !groups.every(isObject)) {
    throw new SettingsShapeError(`hooks.${event} is not an array of groups`);
  }
  return groups as Json[];
}

function hooksOf(settings: Json): Json | undefined {
  if (settings.hooks === undefined) return undefined;
  if (!isObject(settings.hooks)) {
    throw new SettingsShapeError(`"hooks" is not an object`);
  }
  return settings.hooks;
}

/**
 * Remove our entries. Only groups we actually removed from can be dropped when
 * they end up empty; foreign groups (even empty ones) are left alone. An event
 * key is dropped only if we emptied it. `hooks` itself is dropped only if we
 * emptied it, so a pre-existing `"hooks": {}` stays untouched.
 */
export function uninstallHooks(settings: Json): {
  settings: Json;
  removed: number;
} {
  const out = structuredClone(settings);
  const outHooks = hooksOf(out);
  if (!outHooks) return { settings: out, removed: 0 };
  let removed = 0;
  for (const event of Object.keys(outHooks)) {
    const groups = groupsOf(outHooks, event);
    const kept: Json[] = [];
    for (const group of groups) {
      if (!Array.isArray(group.hooks)) {
        kept.push(group);
        continue;
      }
      const handlers: unknown[] = group.hooks;
      const remaining = handlers.filter((h) => !isOurs(h));
      const gone = handlers.length - remaining.length;
      group.hooks = remaining;
      removed += gone;
      if (gone > 0 && remaining.length === 0) continue; // we emptied it
      kept.push(group);
    }
    if (kept.length === 0 && groups.length > 0) delete outHooks[event];
    else outHooks[event] = kept;
  }
  if (removed > 0 && Object.keys(outHooks).length === 0) delete out.hooks;
  return { settings: out, removed };
}

/** Idempotent: removes our old entries, then adds one group per event. */
export function installHooks(settings: Json, command: string): Json {
  const { settings: clean } = uninstallHooks(settings);
  const hooks = hooksOf(clean) ?? {};
  for (const event of EVENTS) {
    const group: Json = {};
    if (!NO_MATCHER.has(event)) group.matcher = "*";
    group.hooks = [{ type: "command", command, async: true, timeout: 5 }];
    hooks[event] = [...groupsOf(hooks, event), group];
  }
  clean.hooks = hooks;
  return clean;
}

export function statusOf(settings: Json): {
  events: string[];
  commands: string[];
} {
  const hooks = hooksOf(settings);
  const events: string[] = [];
  const commands = new Set<string>();
  if (hooks) {
    for (const event of Object.keys(hooks)) {
      let found = false;
      for (const group of groupsOf(hooks, event)) {
        if (!Array.isArray(group.hooks)) continue;
        for (const h of group.hooks) {
          if (isOurs(h)) {
            found = true;
            commands.add((h as Json).command as string);
          }
        }
      }
      if (found) events.push(event);
    }
  }
  return { events, commands: [...commands] };
}
