# Agent Widget — Build Roadmap

A floating, draggable desktop widget that shows what each coding agent (starting with Claude Code) is doing, and lets the user respond to prompts without switching back to VS Code.

This document describes **what to build and in what order**, not how to build it. Each phase lists its goal, what's in scope, what "done" looks like, and the open decisions to settle with the user before or during that phase.

---

## Guiding principles

- **Useful early.** Each phase should leave the user with something they actually run day to day.
- **Never block the agent.** If the widget is closed, crashed or slow, agents must carry on exactly as they would without it (fail open).
- **Local only.** Nothing outside the user's machine can see or answer prompts.
- **Source-agnostic core.** Claude Code is the first agent source, but the core shouldn't assume it's the only one.
- **Verify, don't assume.** Hook event names, payload fields and response formats must be checked against the current Claude Code docs, not taken from memory.

---

## Phase 0 — Discovery and decisions

**Goal:** settle the choices that are expensive to change later.

**In scope**
- Confirm which Claude Code hook events exist in the user's version, what each one sends, and which ones can return a decision.
- Prototype: a throwaway hook script that logs every event to a file, run against a real session (terminal and VS Code) to see real payloads.
- Choose the UI framework and the local communication channel.
- Sketch the session state model (what states an agent can be in, and which events move it between them).

**Done when**
- There's a captured log of real hook payloads for a typical session.
- The UI framework, channel and state model are written down.

**Open questions for the user**
- Tauri (web UI, easier to make pretty) or egui (all Rust, simpler)?
- Windows only, or should it also work on macOS/Linux?
- Named pipe / Unix socket, or localhost HTTP?
- Which agents besides Claude Code might be added later, if any?

---

## Phase 1 — Observe (read-only widget)

**Goal:** see every running agent and its status at a glance.

**In scope**
- A small hook binary that Claude Code runs on each event and that forwards the event to the widget, then exits quickly.
- A listener in the widget that receives events and maintains the session state map.
- A floating window: frameless, always on top, draggable anywhere, out of the taskbar.
- One card or row per session: project name, current state, time in that state.
- Status colours (e.g. working / needs you / done / idle).
- Stale-session handling (a session that goes quiet eventually drops off or greys out).
- A setup step that installs the hook config for the user.

**Done when**
- Running two or three agents (in a terminal and in VS Code) shows them all in the widget with correct states, live.
- Closing the widget has no effect on the agents.

**Open questions for the user**
- One compact pill that expands, or always-visible cards?
- What should "idle" mean, and how long before a session disappears?

---

## Phase 2 — Respond (the main goal)

**Goal:** answer agent prompts from the widget so VS Code doesn't need to be open.

**In scope**
- **Permission prompts:** the hook waits while the widget shows the pending tool call with Approve / Deny / Let the terminal ask. The answer goes back to the agent.
- **Next instruction:** when an agent finishes, the widget offers a reply box; a reply continues the agent, no reply lets it stop normally.
- **Timeouts:** an unanswered prompt falls back to the normal in-agent prompt after a set time.
- **Per-agent mode:** Ask me / Auto-approve safe tools / Hands off / Snooze.
- **Allowlist** for auto-approve (read-only tools and named commands only). Destructive commands are always shown.
- **Local security:** the listener only accepts connections from the hook binary (e.g. a per-install token).
- Investigate which other interactive prompts (agent questions, plan approval) can be handled through hooks, and document the ones that can't.

**Done when**
- The user can run a full session in VS Code with VS Code minimised, handling every permission prompt and follow-up from the widget.
- Killing the widget mid-prompt leaves the agent working normally.

**Open questions for the user**
- Default timeout length?
- What counts as "safe" for auto-approve?
- Should replies support multi-line input or attachments (e.g. pasting a file path)?

---

## Phase 3 — Polish and comfort

**Goal:** make it pleasant enough to leave running all day.

**In scope**
- Remember window position, size and mode between launches; snap to screen edges.
- Tray icon with show/hide, quit and settings.
- Desktop notification or quiet sound when an agent needs the user; per-agent mute.
- Opacity control and click-through mode (so it never gets in the way while gaming).
- Click a card to bring the agent's window (terminal or VS Code) to the front.
- Animations for state changes (e.g. pulsing while working).
- Start on login (optional).
- Settings screen for everything above.

**Done when**
- The user forgets it's a side project and just uses it.

**Open questions for the user**
- Which notifications are useful vs annoying?
- Global hotkey to show/hide or jump to the oldest waiting prompt?

---

## Phase 4 — Own the session (optional, bigger step)

**Goal:** full control over agents launched from the widget, closing the gaps hooks can't cover.

**In scope**
- Launch a new agent from the widget in a chosen project folder (headless with streaming JSON, the Agent SDK, or a PTY — decide here).
- Send any input to those sessions, including mid-run messages and answers to questions hooks couldn't reach.
- A transcript view for widget-owned sessions.
- Keep hook-observed sessions and widget-owned sessions side by side under the same UI.

**Done when**
- The user can start, drive and finish an agent session without ever opening VS Code or a terminal.

**Open questions for the user**
- Headless/SDK or PTY? (Trade-off: structured data vs exactly matching the terminal experience.)
- Does the transcript view need to be rich (diffs, tool output) or is plain text fine?

---

## Phase 5 — Extend (later)

Ideas once the core is solid, not commitments:

- Other agent sources behind the same interface.
- Usage and cost per session.
- History of past sessions and prompts answered.
- Quick actions (open project folder, open diff, rerun tests).
- Phone notifications for long-running agents.

---

## Testing checkpoints (every phase)

- Widget closed → agents behave exactly as normal.
- Widget crashes mid-prompt → agent falls back cleanly, no hang.
- Multiple agents at once → events never get attributed to the wrong session.
- Works with sessions from both the terminal and the VS Code extension.
