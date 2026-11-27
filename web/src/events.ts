// WS client with reconnect; typed events mirroring kagent-core protocol.rs.

export type AgentEvent =
  | { type: "turnStarted"; sessionId: string; turnId: string; input: string }
  | { type: "textDelta"; sessionId: string; turnId: string; text: string }
  | { type: "reasoningDelta"; sessionId: string; turnId: string; text: string }
  | { type: "messageAppended"; sessionId: string; message: import("./api").Message }
  | { type: "toolCallStarted"; sessionId: string; turnId: string; callId: string; name: string; input: unknown; summary: string }
  | { type: "toolCallFinished"; sessionId: string; turnId: string; callId: string; name: string; status: string; output: string; durationMs: number }
  | { type: "approvalRequested"; requestId: string; sessionId: string; tool: string; summary: string; risk: string; reason: string; canRemember: boolean; privileged: boolean }
  | { type: "approvalResolved"; requestId: string; decision: string }
  | { type: "turnFinished"; sessionId: string; turnId: string; status: string; error?: string; usage: { inputTokens: number; outputTokens: number } }
  | { type: "sessionUpdated"; sessionId: string; title: string }
  | { type: "settingsChanged"; settings: unknown }
  | { type: "notice"; level: string; text: string }
  | { type: "hello"; version: string }
  | { type: "alertRaised"; alert: unknown }
  | { type: "alertUpdated"; alert: unknown }
  | { type: "changeRecorded"; change: unknown }
  | { type: "liveStatus"; sessionId: string; status: string; detail: string }
  | { type: "uiCommand"; command: string; args: unknown }
  | { type: "skillsChanged"; count: number }
  | { type: "todoUpdated"; sessionId: string; items: import("./api").TodoItem[] }
  | { type: "planUpdated"; sessionId: string; active: boolean }
  /** The whole meta of a session whose model or effort changed (picker.md section 3). */
  | { type: "sessionMetaChanged"; sessionId: string; meta: import("./api").SessionMeta }
  | { type: "memoryChanged"; name: string; removed: boolean }
  | { type: "tasksChanged" }
  | { type: "taskRun"; taskId: string; run: { at: string; status: string; durationSec?: number; sessionId?: string; result?: string } }
  | { type: "telegramStatus"; status: string; detail: string }
  /** The Google Calendar connection changed state (calendar.md section 7). */
  | { type: "calendarChanged"; state: string }
  /** A subagent of `sessionId` changed state (agents.md section 3). */
  | { type: "subagentUpdated"; sessionId: string; child: import("./settings/subagents/api").ChildView; status: import("./settings/subagents/api").ChildStatus };

type Handler = (ev: AgentEvent) => void;

export class EventStream {
  private ws: WebSocket | null = null;
  private handlers = new Set<Handler>();
  private closed = false;
  private retryMs = 1000;

  start(wsUrl: () => string) {
    this.closed = false;
    const connect = () => {
      if (this.closed) return;
      const ws = new WebSocket(wsUrl());
      this.ws = ws;
      ws.onmessage = (m) => {
        try {
          const ev = JSON.parse(m.data) as AgentEvent;
          this.handlers.forEach((h) => h(ev));
        } catch {
          // ignore malformed frames
        }
      };
      ws.onopen = () => {
        this.retryMs = 1000;
      };
      ws.onclose = () => {
        if (!this.closed) {
          setTimeout(connect, this.retryMs);
          this.retryMs = Math.min(this.retryMs * 2, 15000);
        }
      };
    };
    connect();
  }

  stop() {
    this.closed = true;
    this.ws?.close();
  }

  on(h: Handler): () => void {
    this.handlers.add(h);
    return () => this.handlers.delete(h);
  }
}

export const events = new EventStream();
