// THROWAWAY Phase 4 spike: can one headless `claude -p` process take several
// user messages over stdin (stream-json in/out)? Run with:
//   bun prototype/spike/headless.ts <claude-exe> <cwd>
const [claudeExe, cwd] = process.argv.slice(2);
if (!claudeExe || !cwd) throw new Error("usage: headless.ts <claude-exe> <cwd>");

const proc = Bun.spawn(
  [
    claudeExe,
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--permission-mode",
    "default",
  ],
  { cwd, stdin: "pipe", stdout: "pipe", stderr: "pipe" },
);

const t0 = Date.now();
const log = (s: string) => console.log(`${((Date.now() - t0) / 1000).toFixed(1)}s ${s}`);

function send(text: string) {
  const line = JSON.stringify({ type: "user", message: { role: "user", content: text } });
  log(`>> ${line}`);
  proc.stdin.write(line + "\n");
  proc.stdin.flush();
}

let results = 0;
const decoder = new TextDecoder();
let buf = "";
(async () => {
  for await (const chunk of proc.stdout) {
    buf += decoder.decode(chunk);
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).trim();
      buf = buf.slice(nl + 1);
      if (!line) continue;
      try {
        const ev = JSON.parse(line);
        const summary =
          ev.type === "assistant"
            ? JSON.stringify(ev.message?.content?.map((b: { type: string; text?: string; name?: string }) => b.text ?? b.name ?? b.type)).slice(0, 120)
            : ev.type === "result"
              ? `subtype=${ev.subtype} result=${String(ev.result).slice(0, 60)} cost=${ev.total_cost_usd} session=${ev.session_id}`
              : ev.type === "system"
                ? `subtype=${ev.subtype} session=${ev.session_id ?? ""} caps=${JSON.stringify(ev.capabilities ?? [])}`
                : "";
        log(`<< ${ev.type} ${summary}`);
        if (ev.type === "result") {
          results++;
          if (results === 1) send("Reply with exactly the word TWO and nothing else.");
          else proc.stdin.end();
        }
      } catch {
        log(`<< (non-json) ${line.slice(0, 120)}`);
      }
    }
  }
})();

send("Reply with exactly the word ONE and nothing else.");
const code = await proc.exited;
const err = await new Response(proc.stderr).text();
log(`exit=${code} stderr=${err.slice(0, 300)}`);
