export type View = "widget" | "launcher" | "transcript";

export interface ParsedView {
  view: View;
  /** Session id for the transcript view; null otherwise. */
  session: string | null;
}

/** Pick the view from the URL query (`?view=launcher`); default is the widget. */
export function parseView(search: string): ParsedView {
  const params = new URLSearchParams(search);
  const raw = params.get("view");
  const session = params.get("session");
  if (raw === "launcher") return { view: "launcher", session: null };
  if (raw === "transcript" && session) return { view: "transcript", session };
  return { view: "widget", session: null };
}
