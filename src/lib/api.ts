import { invoke } from "@tauri-apps/api/core";
import type { Mode, PermissionDecision, Settings } from "../types";

// Thin wrappers over the Tauri commands (camelCase argument names).
export const answerPermission = (
  sessionId: string,
  decision: PermissionDecision,
) => invoke<boolean>("answer_permission", { sessionId, decision });

export const sendReply = (sessionId: string, text: string) =>
  invoke<boolean>("send_reply", { sessionId, text });

export const dismissReply = (sessionId: string) =>
  invoke<boolean>("dismiss_reply", { sessionId });

export const setMode = (sessionId: string, mode: Mode) =>
  invoke<boolean>("set_mode", { sessionId, mode });

export const getSettings = () => invoke<Settings>("get_settings");

export const updateSettings = (settings: Settings) =>
  invoke<Settings>("update_settings", { settings });

export const setMuted = (sessionId: string, muted: boolean) =>
  invoke<boolean>("set_muted", { sessionId, muted });

export const focusSession = (sessionId: string) =>
  invoke<void>("focus_session", { sessionId });
