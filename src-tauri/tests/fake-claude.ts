// Stand-in for `claude -p --input-format stream-json --output-format
// stream-json`, used by the Rust tests so they never spend real money.
// It reads one JSON user message per stdin line and prints canned
// stream-json lines shaped like the real CLI's. Run: bun fake-claude.ts
//
// Magic words inside the user text:
//   TOOL     -> the turn also contains a Bash tool_use and its tool_result
//   SUBAGENT -> adds a message with a non-null parent_tool_use_id
//   SLOW     -> waits 400 ms before answering
//   CRASH    -> writes to stderr and exits with code 3
//
// The interrupt control request is deliberately NOT implemented: its wire
// format is not documented, so agentwatch does not send one.

const sessionId = `fake-${crypto.randomUUID()}`;
let turn = 0;

function emit(obj: unknown): void {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

function text(msg: { message?: { content?: unknown } }): string {
  const c = msg.message?.content;
  return typeof c === "string" ? c : JSON.stringify(c ?? "");
}

async function handle(userText: string): Promise<void> {
  turn++;
  if (userText.includes("CRASH")) {
    await Bun.write(Bun.stderr, "fake claude: simulated crash\n");
    process.exit(3);
  }
  if (userText.includes("SLOW")) await Bun.sleep(400);

  emit({
    type: "system",
    subtype: "init",
    session_id: sessionId,
    cwd: process.cwd(),
    model: "fake-model",
    tools: ["Bash"],
    capabilities: ["interrupt_receipt_v1"],
  });
  emit({
    type: "assistant",
    message: {
      role: "assistant",
      content: [{ type: "text", text: `fake reply ${turn}` }],
    },
    parent_tool_use_id: null,
    session_id: sessionId,
  });
  if (userText.includes("SUBAGENT")) {
    emit({
      type: "assistant",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "inner subagent text" }],
      },
      parent_tool_use_id: "toolu_parent",
      session_id: sessionId,
    });
  }
  if (userText.includes("TOOL")) {
    const id = `toolu_fake_${turn}`;
    emit({
      type: "assistant",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id,
            name: "Bash",
            input: { command: "git status" },
          },
        ],
      },
      parent_tool_use_id: null,
      session_id: sessionId,
    });
    emit({
      type: "user",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: id,
            content: "On branch main\nnothing to commit",
            is_error: false,
          },
        ],
      },
      parent_tool_use_id: null,
      session_id: sessionId,
    });
    emit({
      type: "assistant",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "the tree is clean" }],
      },
      parent_tool_use_id: null,
      session_id: sessionId,
    });
  }
  emit({ type: "rate_limit_event", session_id: sessionId });
  emit({
    type: "result",
    subtype: "success",
    is_error: false,
    result: `fake reply ${turn}`,
    total_cost_usd: 0.01 * turn,
    duration_ms: 5,
    session_id: sessionId,
  });
}

// Handle messages one at a time, in order, even if SLOW delays one.
let queue: Promise<void> = Promise.resolve();
let buffer = "";
const decoder = new TextDecoder();
for await (const chunk of Bun.stdin.stream()) {
  buffer += decoder.decode(chunk, { stream: true });
  let nl: number;
  while ((nl = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;
    try {
      const msg = JSON.parse(line);
      if (msg.type === "user") queue = queue.then(() => handle(text(msg)));
    } catch {
      // ignore non-JSON input like the real CLI would report an error
    }
  }
}
// stdin closed: finish pending turns, then exit 0 by letting the loop end.
await queue;

export {};
