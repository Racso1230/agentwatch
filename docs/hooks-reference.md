# Claude Code hooks reference (agentwatch Phase 0)

Researched 2026-11-14 from the live official docs. Source tags used below:

- [HOOKS-REF] https://code.claude.com/docs/en/hooks (fetched as https://code.claude.com/docs/en/hooks.md)
- [HOOKS-GUIDE] https://code.claude.com/docs/en/hooks-guide
- [SETTINGS] https://code.claude.com/docs/en/settings
- [SETTINGS-REF] https://code.claude.com/docs/en/settings-reference
- [VSCODE] https://code.claude.com/docs/en/vs-code

Method note: the hooks reference is ~294k characters and was read through a summarising fetch tool, so a few details could not be confirmed. Anything not stated in the docs is marked "UNVERIFIED - check in prototype".

---

## 1. Hook events

### 1.1 Event list ([HOOKS-REF], "Hook events" table)

| Event | When it fires | Matcher filters on |
|---|---|---|
| `Setup` | `--init-only`, or `--init`/`--maintenance` with `-p` | CLI flag (`init`, `maintenance`) |
| `SessionStart` | Session begins or resumes | `startup`, `resume`, `clear`, `compact`, `fork` |
| `UserPromptSubmit` | Prompt submitted, before Claude processes it. Also fires on turns Claude Code starts on its own | none |
| `UserPromptExpansion` | A user command expands to a prompt | command name |
| `PreToolUse` | Before a tool call; can block | tool name |
| `PermissionRequest` | Claude Code is about to ask the user for permission | tool name |
| `PermissionDenied` | Auto mode denies a tool call | tool name |
| `PostToolUse` / `PostToolUseFailure` | After a tool succeeds / fails | tool name |
| `PostToolBatch` | After a parallel tool batch resolves, before next model call | none |
| `Notification` | Claude Code sends a notification | notification type (1.3) |
| `MessageDisplay` | While assistant text displays | none |
| `SubagentStart` / `SubagentStop` | Subagent spawned or resumed / finished | agent type |
| `TaskCreated` / `TaskCompleted` | Task being created / marked complete | none |
| `Stop` | Main agent finished responding. NOT fired on user interrupt | none |
| `StopFailure` | Turn ended due to an API error | error type |
| `TeammateIdle` | Agent-team teammate about to go idle | none |
| `InstructionsLoaded` | CLAUDE.md or `.claude/rules/*.md` loaded | load reason |
| `ConfigChange` | Config file changes mid-session | `user_settings`, `project_settings`, `local_settings`, `policy_settings`, `skills` |
| `CwdChanged` | Working directory changes | none |
| `DirectoryAdded` | Directory added via `/add-dir` | how added |
| `FileChanged` | Watched file changes | literal filenames |
| `WorktreeCreate` / `WorktreeRemove` | Worktree created / removed | none |
| `PreCompact` / `PostCompact` | Before / after compaction | `manual`, `auto` |
| `PreModelSwitch` / `PostModelSwitch` | Before / after model switch | canonical model name |
| `Elicitation` / `ElicitationResult` | MCP server requests user input / user responds | MCP server name |
| `SessionEnd` | Session terminates | `clear`, `resume`, `logout`, `prompt_input_exit`, `other` |

Lifecycle grouping ([HOOKS-REF]): per session = SessionStart/SessionEnd; per turn = UserPromptSubmit, Stop, StopFailure; per tool call = PreToolUse, PostToolUse.

Matcher syntax ([HOOKS-REF]): `"*"`, `""` or omitted = match all. Only letters/digits/`_`/`-`/space/`,`/`|` = exact name or list (`Edit|Write`). Any other character = JavaScript regex, unanchored (`mcp__memory__.*`). An unknown hook event name is only a "Settings Warning" that skips that entry ([SETTINGS], "Fix a broken settings file"), so listing a newer event on an older version is not fatal.

The installed Claude Code version is UNVERIFIED - check in prototype (`claude --version`). Some features cite minimum versions (e.g. `onFailure` needs v2.1.295+).

### 1.2 Common stdin fields ([HOOKS-REF], "Common input fields")

For `command` hooks this JSON is on stdin; for `http` hooks it is the POST body.

| Field | Notes |
|---|---|
| `session_id` | Current session id |
| `prompt_id` | UUID of the user prompt being processed; absent until first user input |
| `transcript_path` | Conversation JSONL path; written asynchronously, may lag |
| `cwd` | Working directory when the hook was invoked |
| `scratchpad_dir` | May be absent (v2.1.257+) |
| `permission_mode` | `"default"`, `"plan"`, `"acceptEdits"`, `"auto"`, `"dontAsk"`, `"bypassPermissions"`. Manual mode arrives as `"default"`. Not sent on all events |
| `effort` | `{ level: low/medium/high/xhigh/max }`; on tool-context events |
| `hook_event_name` | Event name |
| `agent_id`, `agent_type` | When applicable (subagent context) |

### 1.3 Event-specific fields

| Event | Extra stdin fields | Source |
|---|---|---|
| `SessionStart` | `source` (`startup`/`resume`/`clear`/`compact`/`fork`); optional `model` (only SessionStart can get it, not always) | [HOOKS-REF] |
| `SessionEnd` | `reason` (`clear`, `resume`, `logout`, `prompt_input_exit`, `other`) | [HOOKS-REF] |
| `UserPromptSubmit` | `prompt` (pasted content arrives expanded) | [HOOKS-REF] |
| `PreToolUse` | `tool_name`, `tool_input`, `tool_use_id` | [HOOKS-REF] |
| `PermissionRequest` | `tool_name`, `tool_input`, optional `permission_suggestions` (e.g. `{type:"addRules", rules:[{toolName,ruleContent}], behavior:"allow", destination:"localSettings"}`). **No `tool_use_id`** | [HOOKS-REF] |
| `PostToolUse` | `tool_name`, `tool_input`, `tool_response`, `tool_use_id`, optional `duration_ms` | [HOOKS-REF] |
| `Notification` | `message`, optional `title`, `notification_type` | [HOOKS-REF] |
| `Stop` | `stop_hook_active`, `last_assistant_message`, `background_tasks[]`, `session_crons[]` | [HOOKS-REF] |
| `SubagentStop` | `stop_hook_active`, `agent_id`, `agent_type`, `agent_transcript_path`, `last_assistant_message` | [HOOKS-REF] |
| `SubagentStart` | `agent_type` (matcher); other fields UNVERIFIED | [HOOKS-REF] |
| `Elicitation` | `mcp_server_name`, `message`, optional `mode` (`form`/`url`), `url`, `elicitation_id`, `requested_schema` | [HOOKS-REF] |
| `PreCompact` / `PostCompact` | matcher `manual`/`auto`; other fields UNVERIFIED | [HOOKS-REF] |
| `PreModelSwitch` / `PostModelSwitch` | `from_model`, `to_model` | [HOOKS-REF] |

`Notification` `notification_type` values ([HOOKS-REF], [HOOKS-GUIDE]): `permission_prompt` (needs approval; fires once the prompt has waited ~6 s, each keystroke defers it), `idle_prompt` (Claude finished ~60 s ago and user has not typed), `auth_success`, `auth_storage_failure` (v2.1.288+), `elicitation_dialog`, `elicitation_url_dialog`, `elicitation_complete`, `elicitation_response`, `agent_needs_input`, `agent_completed`, `quota_auto_resume_fired|stale|disabled` (v2.1.234+), `plugin_notification`. In Claude Desktop, the VS Code extension and other Agent-SDK hosts: "Expect `permission_prompt` about six seconds after Claude asks for permission. Claude Code doesn't defer it while you type." Disable with `CLAUDE_CODE_DISABLE_PERMISSION_PROMPT_NOTIFY_HOOKS=1`. If the user or a PermissionRequest hook answers before ~6 s the notification does not run.

Example Stop input ([HOOKS-REF]):
```json
{ "session_id":"abc123", "transcript_path":"...jsonl", "cwd":"/Users/...", "permission_mode":"default",
  "hook_event_name":"Stop", "stop_hook_active":true,
  "last_assistant_message":"I've completed the refactoring...",
  "background_tasks":[{"id":"task-001","type":"shell","status":"running","description":"tail logs","command":"tail -f /var/log/syslog"}],
  "session_crons":[{"id":"cron-001","schedule":"0 9 * * 1-5","recurring":true,"prompt":"check the build"}] }
```
`background_tasks`/`session_crons` let a hook tell "session is done" from "paused waiting on background work".

---

## 2. Decisions and output semantics

### 2.1 Exit codes ([HOOKS-REF], "Exit codes" / "Exit code output")

| Exit | Effect |
|---|---|
| `0` | Success. Stdout parsed as JSON only if it is a single JSON object ("Print nothing but the JSON object to stdout"). Otherwise plain text, which is added to Claude's context ONLY for `UserPromptSubmit`, `UserPromptExpansion`, `SessionStart`, `PostModelSwitch`. Claude never sees stderr from a hook that exits 0 |
| `2` | Blocking error on events that can block; stderr is the reason. "Exit 2 blocks even when your hook prints JSON"; JSON cannot override the block (even `permissionDecision:"allow"`). Choose exit codes OR JSON, not both |
| other (e.g. 1) | Non-blocking error; action proceeds; transcript shows `<hook name> hook error` plus first stderr line. Exit 1 is NOT a block |

Stdout/exit matrix: valid JSON + exit 0 = fields apply; valid JSON + other non-2 code = "the fields alone decide"; JSON + exit 2 = block; plain text or empty + other code = non-blocking error.

Exit 2 per event:
- Blocks: `PreToolUse`, `UserPromptSubmit`, `UserPromptExpansion`, `Stop`, `SubagentStop`, `TeammateIdle`, `TaskCreated`, `TaskCompleted`, `ConfigChange`, `PostToolBatch`, `PreCompact`, `PreModelSwitch`, `ElicitationResult`, `WorktreeCreate`, `WorktreeRemove`.
- Ignored: `PermissionRequest` ("exit 2 without a `decision` object leaves the permission flow unchanged, stderr discarded"), `PermissionDenied`, `Notification`, `StopFailure`, `Setup`, `MessageDisplay`, `InstructionsLoaded`. `Elicitation`: exit 2 declines (no dialog).
- Stderr shown to user only: `PostToolUse`, `PostToolUseFailure`, `SessionStart`, `SessionEnd`, `CwdChanged`, `FileChanged`, `PostCompact`, `PostModelSwitch`.

### 2.2 Universal JSON fields ([HOOKS-REF], "JSON output")

| Field | Default | Meaning |
|---|---|---|
| `continue` | `true` | `false` stops Claude entirely |
| `stopReason` | - | Shown to user when `continue:false` |
| `suppressOutput` | `false` | Docs say "No effect" in this version |
| `systemMessage` | - | Warning shown to the user (discarded for Notification, SessionEnd, Elicitation) |
| `terminalSequence` | - | Allowlisted OSC/BEL sequence emitted by Claude Code (desktop notification). Hooks have no controlling terminal so can't write `/dev/tty` |

### 2.3 Per-event decision formats

| Event | Decides? | Exact output |
|---|---|---|
| `PreToolUse` | Yes | `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"\|"deny"\|"ask"\|"defer","permissionDecisionReason":"...","updatedInput":{...},"additionalContext":"..."}}`. `deny`: reason shown to Claude. `ask`: reason shown to user (forces the normal prompt). `allow`: skips the prompt, with exceptions (section 6); reason is debug-log only. `defer` only in `-p` mode. `updatedInput` replaces the whole input. Multiple hooks: `deny > defer > ask > allow`. **Exit 0 with no JSON does NOT approve** - normal permission flow still runs. Exit 2 = block, stderr to Claude |
| `PermissionRequest` | Yes | Allow: `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow","updatedInput":{...},"updatedPermissions":[...]}}}`. Deny: `"decision":{"behavior":"deny","message":"why (told to Claude)","interrupt":true}`. `updatedInput`/`updatedPermissions` allow-only; `message`/`interrupt` deny-only (`interrupt:true` stops Claude). Deny and ask rules are still evaluated. Exit 2 not honored |
| `UserPromptSubmit` | Yes | Block: `{"decision":"block","reason":"..."}`. Optional `hookSpecificOutput:{hookEventName:"UserPromptSubmit",additionalContext,sessionTitle,suppressOriginalPrompt}`. Exit 2 blocks the prompt. Plain stdout on exit 0 is added to context |
| `Stop` / `SubagentStop` | Yes | Make Claude continue: `{"decision":"block","reason":"<text Claude receives>"}` (`reason` REQUIRED), or exit 2 with stderr = reason. Non-error alternative: `{"hookSpecificOutput":{"hookEventName":"Stop","additionalContext":"..."}}` (continues, labelled "Stop hook feedback", no hook-error notice). Omit `decision` to allow stop. Loop protection: `stop_hook_active` is true when already continuing; **8-consecutive-continuation cap** (resets each time Claude calls a tool; override with `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`) |
| `PostToolUse` / `PostToolUseFailure` | Partial | `decision:"block"` + `reason` adds feedback; Claude still sees original output. `updatedToolOutput` replaces output. Tool already ran |
| `Notification` | No | Side effects only; `systemMessage`/`continue` discarded; `terminalSequence` still honored |
| `SessionStart` / `SessionEnd` | No | SessionStart stdout/`additionalContext` goes to context; SessionEnd JSON discarded |
| `Elicitation` | Yes | `{"hookSpecificOutput":{"hookEventName":"Elicitation","action":"accept"\|"decline"\|"cancel","content":{...}}}`. No output + exit 0 = user sees dialog. Exit 2 or top-level `decision:"block"` = decline. `reason`/`systemMessage`/`continue` discarded. Several hooks: a decline overrides accept/cancel |
| `ElicitationResult` | Yes | Exit 2 blocks the response (becomes decline); full output schema UNVERIFIED |

Matching hooks run in parallel and all run to completion before results merge; one `deny` does not stop sibling hooks ([HOOKS-GUIDE]).

---

## 3. Hook types, config, timeouts, failure

### 3.1 Handler types ([HOOKS-REF] "Configuration shape"; [HOOKS-GUIDE])

| `type` | Required | Notes |
|---|---|---|
| `command` | `command` | Optional `args` (exec form, no shell), `async`, `asyncRewake`, `shell` (`bash`/`powershell`), `onFailure`. Full user permissions, no controlling terminal |
| **`http`** | `url` | **YES, an http hook type exists.** POSTs the event JSON, `Content-Type: application/json`. Optional `headers` (values support `$VAR`/`${VAR}`, resolved ONLY for vars in `allowedEnvVars`; unlisted become empty), `allowedEnvVars`, `onFailure`, `timeout` |
| `mcp_tool` | `server`, `tool` | Calls an MCP tool; optional `input` with `${path}` substitution |
| `prompt` | `prompt` | Single-turn model evaluation; response `{ "ok": true }` or `{ "ok": false, "reason": "..." }`; default 30 s |
| `agent` | `prompt` | Subagent with Read/Grep/Glob; experimental; default 60 s |

Common fields: `type`, `if` (one permission-rule expression like `"Bash(rm *)"`, tool events only), `timeout` (seconds), `statusMessage`, `once` (skill frontmatter only).

Config shape (event -> matcher group -> handlers):
```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash",
        "hooks": [ { "type": "command", "command": "/abs/path/hook", "args": [], "timeout": 30 } ] }
    ]
  }
}
```
HTTP example from [HOOKS-REF]:
```json
{ "type": "http", "url": "http://localhost:8080/hooks/pre-tool-use", "timeout": 30,
  "headers": { "Authorization": "Bearer $MY_TOKEN" }, "allowedEnvVars": ["MY_TOKEN"] }
```
Related settings keys ([SETTINGS-REF] index; only one-line descriptions seen): `allowedHttpHookUrls` ("Limit which URLs HTTP hooks can target", any file), `httpHookAllowedEnvVars` ("Limit which env vars HTTP hooks can put in headers"), `disableAllHooks`, `allowManagedHooksOnly` (managed only). UNVERIFIED - check in prototype: whether `http://127.0.0.1:PORT` works by default without `allowedHttpHookUrls`, and whether literal header values (no env var) are fine.

### 3.2 Timeouts ([HOOKS-REF] "Default timeouts")

| Handler | Default |
|---|---|
| `command`, `http`, `mcp_tool` | **600 s** |
| same on `UserPromptSubmit`, `PreModelSwitch`, `PostModelSwitch` | 30 s |
| same on `MessageDisplay` | 10 s |
| `prompt` | 30 s |
| `agent` | 60 s |
| `SessionEnd` (shared budget for all hooks) | **1.5 s** (raise via per-hook `timeout`, up to 60 s, or `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS`) |

The 600 s default means a hung widget could stall Claude for 10 minutes unless agentwatch sets its own short `timeout`. Always set explicit timeouts.

### 3.3 Failure behaviour (fail open?) ([HOOKS-REF] "HTTP response handling", "onFailure", "Timeouts")

Default is FAIL OPEN: after a non-blocking error "the action proceeds unless `onFailure` is `"block"`".

| Condition | command | http |
|---|---|---|
| Can't start | non-blocking error, proceeds | n/a |
| Exit code other than 0 or 2 | non-blocking error, proceeds | n/a |
| Connection failure | n/a | **non-blocking error, proceeds** |
| Non-2xx status | n/a | non-blocking error, proceeds |
| 2xx empty body | n/a | success (= exit 0, no output) |
| 2xx JSON object | n/a | parsed with the command-hook schema; invalid = non-blocking error |
| 2xx plain text | n/a | non-blocking error; text not added to context |
| Timeout | cancelled, output discarded; on PreToolUse the tool call proceeds | same |
| Invalid JSON | non-blocking error, proceeds | same |

`"onFailure": "block"` (command/http, v2.1.295+) turns all of these into a block (deny on PermissionRequest). No effect on `Stop`, `SubagentStop`, `TaskCompleted`, `TeammateIdle`, or async hooks. agentwatch must NOT set it. The only timeout that blocks is `PreModelSwitch` (not used). Non-blocking errors show a `<hook name> hook error` notice in the transcript (cosmetic noise while the widget is down).

Async hooks: `"async": true` (command type only) runs in the background, cannot decide or block, output delivered on the next turn, `timeout` not enforced once backgrounded.

---

## 4. Settings locations and precedence ([SETTINGS])

| Scope | File | Notes |
|---|---|---|
| User | `~/.claude/settings.json` (Windows `%USERPROFILE%\.claude\settings.json`; relocatable via `CLAUDE_CONFIG_DIR`) | all projects on the machine |
| Shared project | `<project>/.claude/settings.json` | committable; read from the session's primary working directory |
| Project local | `<project>/.claude/settings.local.json` | personal, kept out of git |
| Managed | `managed-settings.json`, MDM, or server-managed | org policy; highest precedence |
| CLI | `--settings <file-or-json>` | per session; above project/user, below managed |
| Other | plugin `hooks/hooks.json`; skill and subagent frontmatter (subagent `Stop` becomes `SubagentStop`) | |

Precedence, highest first: managed > `--settings` > project local > shared project > user. Hook entries do not override each other; they merge ([HOOKS-REF]: "Entries merge across levels"; [SETTINGS]: lists merge). So a user-level agentwatch hook runs in addition to project hooks.

Scoping to one project: put hooks in that project's `.claude/settings.local.json` (personal) or `.claude/settings.json`. All projects: `~/.claude/settings.json`.

Other facts:
- Workspace trust: in interactive sessions hooks from every settings file, including the user file, are held back until the folder's trust dialog is accepted; `-p`/SDK sessions treat the folder as trusted ([HOOKS-REF] "Workspace trust").
- Settings are live reloaded (including `hooks`), no restart needed ([SETTINGS]). Files are strict JSON (no comments, trailing commas).
- `disableAllHooks` cannot disable managed hooks from outside managed settings. `/hooks` lists hooks read-only; `/status` shows loaded sources; `claude --debug` shows hook activity.
- Windows: `"shell": "powershell"` on a command hook; Claude Code auto-detects `pwsh.exe`, falls back to `powershell.exe` 5.1 ([HOOKS-REF]). The default shell for command hooks on Windows is UNVERIFIED - check in prototype. Using `args` (exec form) avoids shell quoting.

---

## 5. VS Code extension vs terminal CLI ([VSCODE], [SETTINGS])

Documented:
- `~/.claude/settings.json` is "shared between the extension and CLI. Use it for allowed commands, environment variables, hooks, and MCP servers." ([VSCODE])
- Terminal, VS Code, JetBrains and the desktop app "all read the same settings files" ([SETTINGS]).
- The extension has a Hooks panel (v2.1.269+) listing session hooks and allowing add/edit/remove in user/project/local settings; managed and plugin hooks are read-only.
- Hooks and permission rules see VS Code's diagnostics tool as `mcp__claude-vscode__getDiagnostics` (CLI: `mcp__ide__getDiagnostics`).
- The VS Code Quick Pick confirmation is "separate from `PreToolUse` hooks".
- VS Code hosts Claude Code through the Agent SDK `canUseTool` mechanism, which changes `permission_prompt` Notification timing (1.3). PermissionRequest hooks "run alongside that host, and whichever decides first applies" ([HOOKS-REF]).
- The extension bundles a private CLI copy and shares conversation history with the CLI. Feature gaps listed (subset of slash commands, no `!` shortcut) are not hook-related. No hook-specific difference is documented.

UNVERIFIED - check in prototype (key for agentwatch): that all events fire identically in the extension; that `PermissionRequest` fires; whether a hook answer dismisses the permission card in the chat panel; `permission_mode` values; whether `SessionEnd` fires when a panel/tab closes (and within its 1.5 s budget); whether `allowedEnvVars` interpolation works the same. The docs state no differences but give no explicit parity guarantee.

---

## 6. Interactive prompts: which can be handled via hooks

| Prompt | Via hooks? | How ([HOOKS-REF], [HOOKS-GUIDE]) |
|---|---|---|
| Tool permission prompt | YES | `PermissionRequest` hook (`decision.behavior` allow/deny), or `PreToolUse` `permissionDecision`. PermissionRequest fires only when Claude Code is "about to ask you for permission", so not for calls already allowed by rules or a PreToolUse allow (implied, not stated). Not fired for sandbox network requests. Auto-mode firing UNVERIFIED |
| "Let the terminal ask" | YES | Return nothing (exit 0 / empty body): the normal prompt appears. PreToolUse can also return `ask` |
| `AskUserQuestion` | YES, needs `updatedInput` | `PreToolUse` matcher `AskUserQuestion`: `permissionDecision:"allow"` plus `updatedInput` containing the original `questions` array and `answers` (map question text to selected option label; multi-select joins with commas). "`AskUserQuestion` and `ExitPlanMode` need `updatedInput` paired with `"allow"`. Returning `"allow"` alone is not sufficient." Example:<br>`{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"questions":[...],"answers":{"Which framework?":"React"}}}}` |
| Plan approval (`ExitPlanMode`) | YES, needs `updatedInput` | Input has `plan`, `planFilePath` (deprecated `allowedPrompts` ignored). Same rule: allow + `updatedInput`; with PermissionRequest allow the hook-guide says without `updatedInput` "Claude Code ignores it and still shows the prompt". Rejecting a plan with feedback is UNVERIFIED (try `deny` + reason) |
| MCP tools with `_meta["anthropic/requiresUserInteraction"]` | NO | Hook cannot skip approval, "with or without `updatedInput`" |
| Agent SDK `always_ask` tools, org `ask` connector tools | Mostly NO | Listed among cases where a hook allow cannot skip the prompt |
| MCP elicitation (form / URL) | YES | `Elicitation` hook returns `action` accept/decline/cancel (+ `content`). For URL mode, `accept` skips the dialog so the URL never opens - agentwatch should pass through |
| Sandbox network-request approval | NO | PermissionRequest not fired; `permission_prompt` Notification fires but cannot respond |
| Trust dialog, slash commands, login, config dialogs | NO | Not hook events |
| Next instruction after Stop | YES (via block) | `Stop` `decision:"block"` + `reason` (section 7.3) |

`AskUserQuestion`/`ExitPlanMode` are offered in `-p` runs only "when the run has a permission host". Check in prototype whether PreToolUse or PermissionRequest (or both) fire for these tools in terminal vs VS Code.

---

## 7. Implications for agentwatch

### 7.1 Event to widget state mapping (proposal)

| Widget state | Entered on | Left on |
|---|---|---|
| started | `SessionStart` | next prompt |
| working | `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `SubagentStart` (tool name/input as activity text) | `PermissionRequest`, `Stop`, `SessionEnd` |
| needs-you | `PermissionRequest` (actionable); `PreToolUse` with `AskUserQuestion`/`ExitPlanMode` (actionable); `Elicitation` form mode (actionable); `Notification` `permission_prompt`/`elicitation_dialog`/`agent_needs_input` (informational fallback) | `PostToolUse`, `PostToolUseFailure`, `PermissionDenied`, next `PreToolUse`, `Stop`, `UserPromptSubmit` |
| waiting-for-reply | `Stop` (card text = `last_assistant_message`; if `background_tasks` non-empty show "paused on background work") | `UserPromptSubmit` or widget reply back to working |
| idle | `Notification` `idle_prompt` (~60 s after Stop) or a widget-side timer after Stop | `UserPromptSubmit` |
| error | `StopFailure` | `UserPromptSubmit` |
| ended | `SessionEnd` (with `reason`) | - |

Key sessions by `session_id`; show `cwd` basename as project. Use `agent_id` to avoid attributing subagent events to the parent (UNVERIFIED). SessionEnd is not guaranteed on crash/kill and `Stop` does not fire on user interrupt, so add a staleness timer.

### 7.2 Permission response

1. Register a hook on `PermissionRequest` (all tools) with a `timeout` long enough for a human (e.g. 60-120 s; this is the roadmap's "default timeout" decision).
2. Widget gets `session_id`, `tool_name`, `tool_input`, `permission_suggestions` and shows Approve / Deny / Let terminal ask.
3. Respond (HTTP 200 JSON body, or stdout with exit 0):
   - Approve: `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}`
   - Deny: `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied from agentwatch","interrupt":false}}}`
   - Let terminal ask / timeout / widget down: empty body or exit 0 with empty stdout = normal prompt.
   - "Always allow": pass a received `permission_suggestions` entry back in `updatedPermissions`.
4. `PermissionRequest` has no `tool_use_id`: correlate with later `PostToolUse` by `session_id` + `tool_name` + `tool_input` or ordering.
5. Race with the in-app prompt (user answers in terminal while the hook is pending) is UNVERIFIED - check in prototype; the widget should clear its card on the next event for that session.
6. Deny/ask rules in settings still apply after a hook allow.

### 7.3 Stop-continue (follow-up instruction)

1. `Stop` hook forwards `last_assistant_message` to the widget and holds the request while the reply box is shown (timeout configurable, e.g. 60-120 s).
2. Reply text `T`: respond `{"decision":"block","reason":"<T>"}` (exit 0). Claude receives `reason` as why it should continue, so phrase it as an instruction (e.g. prefix "The user says via agentwatch: ..."). Alternative: `hookSpecificOutput.additionalContext` (no hook-error label); test which gives better model behaviour.
3. No reply / timeout / widget down: empty response / exit 0 and Claude stops normally.
4. 8-consecutive-continuation cap applies; resets when Claude calls a tool. Check `stop_hook_active`.
5. UNVERIFIED: whether `UserPromptSubmit` fires for the injected instruction (likely not), how it shows in the transcript, and how VS Code/terminal UI behaves while the Stop hook is pending.
6. Probably do not offer replies for `SubagentStop`.

### 7.4 Fail-open strategy

Documented support: connection failure, non-2xx, timeout, invalid output and any exit code other than 0/2 are non-blocking and the action proceeds (unless `onFailure:"block"`, which agentwatch never sets); exit 0 with empty stdout is "no opinion" so the normal permission flow runs. Risks: default timeout 600 s; visible `hook error` notices; exit 2 would block.

Recommended design:
- Prefer a tiny command shim (`agentwatch-hook`, small native binary) over a raw `http` hook: it can read the token from a file, swallow connection-refused silently (exit 0, empty stdout, no hook-error notice), enforce its own deadline, and exit 0 on any internal error. The shim POSTs to `http://127.0.0.1:<port>/event` with the per-install token and prints the widget's body only if it is a valid JSON object.
- Native `http` hooks are the alternative (no binary): the URL is static, so a fixed port is needed; token via `"Authorization": "Bearer $AGENTWATCH_TOKEN"` + `allowedEnvVars` (or literal); a down widget produces a hook-error notice per event; `allowedHttpHookUrls` requirement UNVERIFIED.
- Always set explicit `timeout`: observe events 2-5 s (or `async:true` command hooks); `PermissionRequest` and `Stop` ~60-120 s. Shim deadline a few seconds under the configured timeout. `SessionEnd` has a shared 1.5 s budget so use a <=1 s fire-and-forget.
- Fast path when widget is down: connect with ~100-200 ms timeout, on failure exit 0 immediately; cache the "down" state briefly.
- Never emit `decision`, `permissionDecision` or `continue:false` unless the user chose it in the widget. Never print plain text on stdout for `UserPromptSubmit`/`SessionStart` (it is injected into Claude's context).
- Install into `~/.claude/settings.json` (all projects, shared with VS Code) or per-project `.claude/settings.local.json`. Hooks merge with existing ones, so the installer must be idempotent and not clobber other entries (strict JSON).
- Security: bind 127.0.0.1 only, check the per-install token, and validate Host header (general practice, not from the docs).
- Ask me / Auto-approve / Hands off / Snooze modes are decided in the widget at response time; hook config does not change.

### 7.5 Open items for the prototype (all UNVERIFIED in docs)

1. Installed Claude Code version; that `PermissionRequest`, `Elicitation`, `StopFailure` exist there.
2. Real payloads in terminal vs VS Code (log everything; compare fields, `permission_mode`, `agent_id`).
3. VS Code: does `PermissionRequest` fire; does a hook answer dismiss the card; does `SessionEnd` fire when the panel closes.
4. PermissionRequest vs terminal race.
5. PermissionRequest in auto/acceptEdits/bypass modes; AskUserQuestion and ExitPlanMode end-to-end; rejecting a plan.
6. `http` hooks to `127.0.0.1` without `allowedHttpHookUrls`; how hook-error notices look when the widget is down.
7. Stop-block reply behaviour (`reason` presentation, `UserPromptSubmit`, cap).
8. Subagent attribution (`session_id` vs `agent_id`), whether parent PermissionRequest hooks fire for subagent tool calls.
9. Windows: default shell for command hooks, quoting, shim spawn latency (target < 50 ms).
10. Whether an interrupted turn (Esc) emits any event.
