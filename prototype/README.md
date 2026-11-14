# Phase 0 prototype: hook event logger (THROWAWAY)

`log-hook.mjs` appends every Claude Code hook payload it receives to `logs/events.jsonl`
(one JSON line: `ts`, `event`, `pid`, `payload`). It exists only to capture real payloads
for Phase 0 and will be deleted once the real hook shim exists.

It fails open: no stdout/stderr, always exits 0, 2s safety timer. It never returns a decision.

## Enable
Register it as a `command` hook in a settings file, e.g.
`node "c:/dev/agentwatch/prototype/log-hook.mjs" <EventName>`.
The sandbox at `c:\dev\agentwatch-sandbox\.claude\settings.local.json` already does this
for every event. Run `claude` from that folder (accept workspace trust in interactive mode).

## Disable
Delete or rename the sandbox `.claude/settings.local.json`, or remove the hook entries.
Delete `prototype/logs/` to clear captured data (git-ignored).

## Testing
Set `AGENTWATCH_LOG_PATH` to write elsewhere (test-only override).
