# Changelog

All notable changes to this project are documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-11-15

First public release (Windows and Linux, unsigned builds).

### Added

- Session overview
  - A frameless, always-on-top widget that lists every Claude Code session, from the terminal and the VS Code extension, with its live state: working, needs you, waiting for a reply, idle, ended.
  - Project name, time in the current state, and a short subtitle from the first prompt to tell sessions in the same folder apart.
  - Idle after 5 minutes of waiting, grey after 30 minutes of silence, and removal of ended or long-stale sessions.
  - Sessions whose Claude process has died move to Ended.
  - Click a VS Code session to bring its window to the front.
- Responding
  - Approve or deny permission prompts from the widget. Answer in the widget or in Claude, and the first answer wins; unanswered prompts fall back to Claude's own prompt after 2 minutes.
  - Reply to Claude when it finishes a turn; the reply continues the session.
  - Per-session modes: Ask me, Auto-approve safe tools (read-only tools and an allowlist of read-only shell commands), Hands off and Snooze.
- Agents launched from the widget
  - Start a headless Claude Code agent in a chosen folder, with a recent-folders list.
  - A transcript window per agent with text, expandable one-line tool summaries, running cost, Stop (interrupt the turn) and End.
  - Permission prompts from these agents appear in the transcript window and on the widget card.
  - Quitting asks for confirmation while launched agents are running.
- Comfort
  - System tray icon with show/hide, settings and quit.
  - Desktop notifications when a session needs you or finishes, with per-session mute and an optional sound.
  - Remembered window position and size, snapping to screen edges, opacity slider, click-through mode, and start on login (off by default).
- Claude Code connection
  - Connect and Disconnect buttons in Settings, with a consent step. Connect backs up `~/.claude/settings.json` first, merges the hook entries without touching your own, and installs the hook program to `~/.agentwatch/bin`.
  - A native, roughly 200 KB hook program that fails open: if the widget is not running, Claude Code is not affected in any way.
  - The Windows uninstaller removes the hooks automatically (but not on upgrade).
- Configuration through `~/.agentwatch/config.json` (timeouts, safe command allowlist, path to `claude`).
- Local-only operation: an HTTP server bound to `127.0.0.1` protected by a per-install token, and no telemetry.
- Release builds for Windows (NSIS installer) and Linux (`.deb` and AppImage) through GitHub Actions.

### Known limitations

- Builds are not code-signed; Windows SmartScreen shows a warning.
- macOS is not supported.
- Linux builds have not been tested on a Linux desktop by the author.
