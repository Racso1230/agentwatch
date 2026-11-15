import { expect, test } from "bun:test";
import { parseHostTriple } from "./prepare-hook";

test("parseHostTriple reads the host line", () => {
  const out =
    "rustc 1.99.1\nbinary: rustc\nhost: x86_64-pc-windows-msvc\r\nrelease: 1.99.1\n";
  expect(parseHostTriple(out)).toBe("x86_64-pc-windows-msvc");
});

test("parseHostTriple throws without a host line", () => {
  expect(() => parseHostTriple("rustc 1.99.1")).toThrow();
});
