import { expect, test } from "bun:test";
import {
  clampOpacity,
  DEFAULT_SETTINGS,
  opacityLabel,
  panelAlpha,
} from "./settings";

test("clampOpacity", () => {
  expect(clampOpacity(0.1)).toBe(0.4);
  expect(clampOpacity(2)).toBe(1);
  expect(clampOpacity(0.7)).toBe(0.7);
  expect(clampOpacity(NaN)).toBe(1);
});

test("panelAlpha and label", () => {
  expect(panelAlpha(0.953)).toBe(0.95);
  expect(panelAlpha(0)).toBe(0.4);
  expect(opacityLabel(0.85)).toBe("85%");
});

test("defaults are within range", () => {
  expect(clampOpacity(DEFAULT_SETTINGS.opacity)).toBe(DEFAULT_SETTINGS.opacity);
});
