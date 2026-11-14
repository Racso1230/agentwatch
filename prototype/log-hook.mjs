#!/usr/bin/env node
// THROWAWAY Phase 0 logger: appends every Claude Code hook payload to a JSONL file.
//
// FAIL OPEN: this script must never affect Claude Code. Therefore it
//  - never writes to stdout/stderr (stdout could be parsed as a hook decision),
//  - always exits 0 (exit 2 would BLOCK on many events; other codes show hook errors),
//  - has a safety timer so a stdin that never closes cannot hang the agent.
import { appendFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const done = () => process.exit(0);

// Safety net: if stdin never closes, give up after 2s (unref'd so it doesn't keep us alive).
setTimeout(done, 2000).unref();

// Last-resort handlers: swallow anything unexpected and still exit 0.
process.on('uncaughtException', done);
process.on('unhandledRejection', done);

function main(raw) {
  try {
    let payload;
    try {
      payload = JSON.parse(raw);
    } catch {
      payload = raw; // keep raw string if not JSON
    }
    const event =
      (payload && typeof payload === 'object' && payload.hook_event_name) ||
      process.argv[2] ||
      'unknown';
    const line = JSON.stringify({
      ts: new Date().toISOString(),
      event,
      pid: process.ppid,
      payload,
    });
    // AGENTWATCH_LOG_PATH is a test-only override.
    const logPath =
      process.env.AGENTWATCH_LOG_PATH ||
      resolve(dirname(fileURLToPath(import.meta.url)), 'logs', 'events.jsonl');
    mkdirSync(dirname(logPath), { recursive: true });
    appendFileSync(logPath, line + '\n');
  } catch {
    // Fail open: logging problems are silently ignored.
  }
  done();
}

try {
  let raw = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (c) => (raw += c));
  process.stdin.on('end', () => main(raw));
  process.stdin.on('error', () => main(raw));
} catch {
  done();
}
