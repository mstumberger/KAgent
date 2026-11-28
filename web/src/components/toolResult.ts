// How a tool result is labelled on its card (agent-loop.md B.1). The daemon's three result kinds besides ok and
// error have a fixed beginning (`ToolOutput::not_done`, `shown`, `partial`), so a stored message, which has no
// status field, is told apart by it, as the live event is told apart by its `status`.
export type ToolResultKind = "ok" | "error" | "notDone" | "shown" | "partial";

const BEGINNINGS: [string, ToolResultKind][] = [
  ["not done: ", "notDone"],
  ["shown to the owner: ", "shown"],
  ["partly done: ", "partial"],
];

export function toolResultKind(content: string, isError: boolean | undefined, status?: string): ToolResultKind {
  if (status === "notDone" || status === "shown" || status === "partial") return status;
  for (const [start, kind] of BEGINNINGS) if (content.startsWith(start)) return kind;
  return isError ? "error" : "ok";
}

export const TOOL_RESULT_LABEL: Record<ToolResultKind, string> = {
  ok: "done",
  error: "failed",
  notDone: "not done",
  shown: "shown",
  partial: "partial",
};
