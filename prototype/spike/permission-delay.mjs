// THROWAWAY Phase 2 spike E1: a PermissionRequest hook that waits 20 s, then allows.
// Question: while this hook is waiting, does the terminal / VS Code show its own
// permission prompt at the same time? If you answer there first, what happens?
import { appendFileSync } from "node:fs";

const log = (msg) => {
  try {
    appendFileSync(new URL("./spike.log", import.meta.url), `${new Date().toISOString()} E1 ${msg}\n`);
  } catch {}
};

let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  log(`hook started (waiting 20 s) ${raw.slice(0, 120)}`);
  setTimeout(() => {
    log("hook answering allow");
    process.stdout.write(
      JSON.stringify({
        hookSpecificOutput: {
          hookEventName: "PermissionRequest",
          decision: { behavior: "allow" },
        },
      }),
    );
    process.exit(0);
  }, 20_000);
});
