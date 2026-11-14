import type { Settings } from "../types";

export const MIN_OPACITY = 0.4;
export const MAX_OPACITY = 1.0;

/** Clamp opacity to the supported range; NaN falls back to the maximum. */
export function clampOpacity(v: number): number {
  if (!Number.isFinite(v)) return MAX_OPACITY;
  return Math.min(MAX_OPACITY, Math.max(MIN_OPACITY, v));
}

/** Alpha for the panel background: scales the base alpha (0.88) by opacity. */
export function panelAlpha(opacity: number): number {
  return Math.round(clampOpacity(opacity) * 100) / 100;
}

/** "85%" for the slider label. */
export function opacityLabel(opacity: number): string {
  return `${Math.round(clampOpacity(opacity) * 100)}%`;
}

export const DEFAULT_SETTINGS: Settings = {
  opacity: 0.95,
  click_through: false,
  notify_needs_you: true,
  notify_finished: true,
  sound: false,
  autostart: false,
  snap_to_edges: true,
};
