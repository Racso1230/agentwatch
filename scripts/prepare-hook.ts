// Builds the native hook binary and copies it where Tauri's `externalBin`
// expects it: src-tauri/binaries/agentwatch-hook-<target-triple>[.exe].
// Run via `bun run release:build` (see src-tauri/tauri.release.conf.json).
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

function run(cmd: string[]): string {
  const p = Bun.spawnSync(cmd, { stdout: "pipe", stderr: "inherit" });
  if (p.exitCode !== 0) {
    throw new Error(`${cmd.join(" ")} failed with exit code ${p.exitCode}`);
  }
  return p.stdout.toString();
}

/** Host triple from the `host:` line of `rustc -vV`. */
export function parseHostTriple(rustcVV: string): string {
  const line = rustcVV.split(/\r?\n/).find((l) => l.startsWith("host:"));
  const triple = line?.slice("host:".length).trim();
  if (!triple) throw new Error("could not find `host:` in `rustc -vV` output");
  return triple;
}

if (import.meta.main) {
  const root = join(import.meta.dir, "..");
  const triple = parseHostTriple(run(["rustc", "-vV"]));
  const exe = triple.includes("windows") ? ".exe" : "";

  run([
    "cargo",
    "build",
    "--release",
    "--manifest-path",
    join(root, "hook", "Cargo.toml"),
  ]);

  const from = join(root, "hook", "target", "release", `agentwatch-hook${exe}`);
  const outDir = join(root, "src-tauri", "binaries");
  const to = join(outDir, `agentwatch-hook-${triple}${exe}`);
  mkdirSync(outDir, { recursive: true });
  copyFileSync(from, to);
  console.log(`hook binary: ${to}`);
}
