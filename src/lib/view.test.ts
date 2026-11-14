import { describe, expect, test } from "bun:test";
import { parseView } from "./view";

describe("parseView", () => {
  test("defaults to widget", () => {
    expect(parseView("")).toEqual({ view: "widget", session: null });
    expect(parseView("?view=nonsense")).toEqual({
      view: "widget",
      session: null,
    });
  });
  test("launcher", () => {
    expect(parseView("?view=launcher").view).toBe("launcher");
  });
  test("transcript needs a session", () => {
    expect(parseView("?view=transcript&session=abc%20d")).toEqual({
      view: "transcript",
      session: "abc d",
    });
    expect(parseView("?view=transcript").view).toBe("widget");
  });
});
