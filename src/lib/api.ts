import { invoke } from "@tauri-apps/api/core";
import type { Mode, PermissionDecision } from "../types";

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
