# Prototype and spike code (history)

This folder holds the throwaway code from the early research phases. It is not part of the app and is not built, tested or shipped. It is kept because `docs/state-model.md` and `docs/DECISIONS.md` refer to what these spikes found.

- `log-hook.mjs` (Phase 0): a Claude Code hook that appends every hook payload it receives to `logs/events.jsonl`. It fails open: no stdout or stderr, always exit 0, 2 s safety timer. Set `AGENTWATCH_LOG_PATH` to write elsewhere. The captured logs are git-ignored and not published; the real payloads were turned into the neutral fixtures in `src-tauri/tests/fixtures/`.
- `spike/permission-delay.mjs` (Phase 2): does Claude show its own permission prompt while a `PermissionRequest` hook is waiting?
- `spike/stop-rewake.mjs` (Phase 2): how does a `Stop` hook with `asyncRewake` behave when it exits with code 2?
- `spike/headless.ts` (Phase 4): can one headless `claude -p` process take several messages over stdin (stream-json)?

To try the logger, register it as a `command` hook in a Claude Code settings file, for example `node "<path-to-repo>/prototype/log-hook.mjs" <EventName>`, and remove the entry afterwards. Never leave it installed: the app itself is the supported way to connect (Settings, Connect).
