import { describe, expect, test } from "bun:test";
import type { HooksStatus } from "../types";
import {
  canDisconnect,
  connectButtonLabel,
  connectConsent,
  hintText,
  hooksDetail,
  hooksLabel,
  hooksState,
  showConnectHint,
} from "./hooksUi";

const base: HooksStatus = {
  connected: false,
  events: [],
  command: null,
  stale: false,
  settings_path: "/home/u/.claude/settings.json",
};

describe("hooksUi", () => {
  test("state mapping", () => {
    expect(hooksState(null)).toBe("unknown");
    expect(hooksState(base)).toBe("disconnected");
    expect(hooksState({ ...base, connected: true })).toBe("connected");
    expect(hooksState({ ...base, connected: true, stale: true })).toBe("stale");
  });

  test("labels", () => {
    expect(hooksLabel("connected")).toBe("Connected");
    expect(hooksLabel("disconnected")).toBe("Not connected");
    expect(hooksLabel("stale")).toBe("Needs update");
    expect(hooksLabel("unknown")).toBe("Checking…");
  });

  test("detail only for stale", () => {
    expect(hooksDetail("stale")).toContain("Reconnect");
    expect(hooksDetail("connected")).toBeNull();
  });

  test("buttons", () => {
    expect(connectButtonLabel("stale")).toBe("Reconnect");
    expect(connectButtonLabel("disconnected")).toBe("Connect");
    expect(canDisconnect("connected")).toBe(true);
    expect(canDisconnect("stale")).toBe(true);
    expect(canDisconnect("disconnected")).toBe(false);
    expect(canDisconnect("unknown")).toBe(false);
  });

  test("consent mentions path and backup", () => {
    const t = connectConsent("C:/x/settings.json");
    expect(t).toContain("C:/x/settings.json");
    expect(t).toContain("backup");
  });

  test("hint", () => {
    expect(showConnectHint(null)).toBe(false);
    expect(showConnectHint(base)).toBe(true);
    expect(showConnectHint({ ...base, connected: true })).toBe(false);
    expect(hintText(base)).toContain("Not connected");
    expect(hintText({ ...base, connected: true, stale: true })).toContain(
      "update",
    );
  });
});
