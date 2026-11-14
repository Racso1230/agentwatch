import { invoke } from "@tauri-apps/api/core";
import type {
  Mode,
  PermissionDecision,
  Settings,
  TranscriptItem,
} from "../types";

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

export const launchSession = (cwd: string, prompt: string) =>
  invoke<string>("launch_session", { cwd, prompt });

export const getTranscript = (sessionId: string) =>
  invoke<TranscriptItem[]>("get_transcript", { sessionId });

/** Real session id for a temporary "pending-..." id (or the id itself). */
export const resolveSession = (sessionId: string) =>
  invoke<string>("resolve_session", { sessionId });

export const sendMessage = (sessionId: string, text: string) =>
  invoke<void>("send_message", { sessionId, text });

export const interruptSession = (sessionId: string) =>
  invoke<void>("interrupt_session", { sessionId });

export const endSession = (sessionId: string) =>
  invoke<void>("end_session", { sessionId });

export const ownedRunningCount = () => invoke<number>("owned_running_count");

export const quitApp = () => invoke<void>("quit_app");

export const openLauncher = () => invoke<void>("open_launcher");

export const openTranscript = (sessionId: string) =>
  invoke<void>("open_transcript", { sessionId });
