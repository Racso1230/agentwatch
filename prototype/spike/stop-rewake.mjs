// THROWAWAY Phase 2 spike E2: a Stop hook run with `asyncRewake: true`.
// It waits 15 s in the background, then exits 2 with a fake "widget reply" on
// stderr. Questions: is the terminal free while it waits? Does Claude wake up
// and act on the reply? How is the reply shown?
// Runs only once per session (flag file), so it can't loop.
import { appendFileSync, existsSync, writeFileSync } from "node:fs";

const here = (name) => new URL(`./${name}`, import.meta.url);
const log = (msg) => {
  try {
    appendFileSync(here("spike.log"), `${new Date().toISOString()} E2 ${msg}\n`);
  } catch {}
};

let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  let sid = "unknown";
  try {
    sid = JSON.parse(raw).session_id || sid;
  } catch {}
  const flag = here(`rewoke-${sid}.flag`);
  if (existsSync(flag)) {
    log(`already rewoke ${sid}, exiting 0`);
    process.exit(0);
  }
  writeFileSync(flag, "1");
  log(`Stop received for ${sid}, waiting 15 s`);
  setTimeout(() => {
    log("exiting 2 with reply");
    process.stderr.write(
      "The user replied from the agentwatch widget: please answer with just the word BANANA.",
    );
    process.exit(2);
  }, 15_000);
});
