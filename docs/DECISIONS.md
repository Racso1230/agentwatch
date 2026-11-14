# Decisions (ADRs)

All decided by the user on 2026-11-14 (end of Phase 0).

| # | Topic | Decision |
|---|---|---|
| 1 | UI | Tauri 2 + React + TypeScript |
| 2 | Platforms | Windows, Linux, macOS |
| 3 | Channel | localhost HTTP on 127.0.0.1 + per-install token |
| 4 | Stakes | Personal tool; no signing/auto-update; must fail open |
| 5 | Hook transport | Command shim (Bun/TypeScript) that POSTs to the widget (not native `http` hooks) |
| 6 | Sources | Claude Code only for now; source-agnostic core event type |
| 7 | Session identity | `session_id` only |

---

## ADR-1: UI stack — Tauri 2 + React + TypeScript

- **Context:** The roadmap asked Tauri vs egui. The user already knows Tauri/React/TS from another project.
- **Decision:** Tauri 2 shell, React + TypeScript frontend, Rust backend (listener, state map).
- **Consequences:** Easy to make pretty; frameless/always-on-top/click-through come from Tauri window APIs. The Rust workspace can also host the hook shim (ADR-5). Larger footprint than egui; accepted.

## ADR-2: Platforms — Windows, Linux, macOS

- **Context:** Phase 0 was captured on Windows only (backslash paths in every payload).
- **Decision:** Support all three. Anything OS-specific (focusing a terminal/VS Code window, click-through, autostart, hook install paths) lives in small per-OS modules behind a common trait/interface.
- **Consequences:** Paths in payloads must be treated as opaque strings (or normalised for display only). Window focus (Phase 3) is the riskiest OS code. CI/manual testing on all three is needed before claiming support.

## ADR-3: Channel — localhost HTTP with per-install token

- **Context:** Options were named pipe / Unix socket vs localhost HTTP.
- **Decision:** HTTP server bound to `127.0.0.1` only (never `0.0.0.0`), random per-install token stored in a file the shim can read; requests without the token are rejected. Also validate the `Host` header.
- **Consequences:** One cross-platform code path (no pipe vs socket split). Port must be discoverable by the shim (fixed default, or port written to a file next to the token). Any local process that can read the token file can talk to the widget; acceptable for a personal tool.

## ADR-4: Stakes — personal tool, still fail open

- **Context:** Single-user tool, not distributed.
- **Decision:** No code signing, no auto-update, no installer polish. The fail-open principle is NOT relaxed: widget closed, crashed or slow must never block or disturb an agent.
- **Consequences:** Unsigned binaries may trigger SmartScreen/Gatekeeper warnings; accepted. Effort goes into robustness of the hook path instead.

## ADR-5: Hook transport — command shim, not native `http` hooks

- **Context:** Claude Code has a native `http` hook type (see `hooks-reference.md` 3.1). It was tested in a real terminal session with the URL pointing at a closed port (widget not running). Observed:
  - every tool call showed `PreToolUse:Write hook error` / `connect ECONNREFUSED 127.0.0.1:47821`;
  - every turn showed `Stop hook error`;
  - a footer line `Stop hook error occurred · ctrl+o to see`.

  Also, `SessionStart` does not support `http` hooks. The user's rule: **widget closed = a conscious choice not to use it, so agents must behave exactly as without it, with no noise.**
- **Decision:** Hooks are `command` hooks that run a small shim (`agentwatch-hook`). The shim reads the event JSON from stdin, POSTs it to `http://127.0.0.1:<port>/event` with the token, and relays the widget's response. Requirements:
  - connect timeout ~100-200 ms;
  - on ANY failure (refused, timeout, bad response, internal error): exit 0 with empty stdout and empty stderr;
  - never set `onFailure: "block"`;
  - set explicit hook `timeout` values per event (default is 600 s): short for observe-only events, longer (60-120 s) for `PermissionRequest`/`Stop` once Phase 2 holds them; `SessionEnd` shares a 1.5 s budget, so fire-and-forget;
  - print the widget's response only if it parses as a single JSON object; never print plain text (it is injected into Claude's context for `UserPromptSubmit`/`SessionStart`);
  - never exit 2.
- **Consequences:** Widget down = silent no-op. Costs one process spawn per event. A binary must be built and installed; the installer writes the hook config idempotently and merges with existing hooks.
- **Shim language (decided 2026-11-14): Bun / TypeScript.** Bun is also the frontend package manager and script runner.

  | Option considered | Pros | Cons |
  |---|---|---|
  | **Bun (chosen)** | Same language as the frontend; ~80 ms per event measured (vs ~110 ms Node, same machine via Git Bash) | Needs Bun on PATH, or `bun build --compile` (single exe, ~60–100 MB) |
  | Rust binary | ~ms startup, ~1 MB exe | Second language for the shim |
  | Node script | Fast to write | Slowest; needs Node on PATH |

  Whether the hook config calls `bun shim.ts` or a compiled exe is decided in Phase 1.

## ADR-6: Sources — Claude Code only, source-agnostic core

- **Context:** Roadmap principle: the core shouldn't assume Claude Code is the only source.
- **Decision:** Only a Claude Code adapter for now. The core event type carries `source` (e.g. `"claude-code"`) plus `session_id`; the adapter maps raw hook payloads to core events and states. Raw payload is kept for debugging.
- **Consequences:** Small up-front cost (one mapping layer). Session key is `(source, session_id)`.

## ADR-7: Session identity — `session_id` only

- **Context:** In the captured log the parent pid (`pid` in each record) differs for every event (each hook is a fresh process), and `cwd` is shared by all sessions opened in the same folder (all three sessions used `C:\dev\agentwatch-sandbox`).
- **Decision:** A session is identified by `session_id` alone. `cwd` basename is display text only; pid is never used.
- **Consequences:** Two sessions in one folder show as two cards. Events with a missing `session_id` are dropped. `agent_id` (subagents) is a secondary field and never creates a session.
