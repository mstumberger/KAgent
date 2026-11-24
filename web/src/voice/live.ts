// Live voice client for kagentd's /live socket (the daemon talks to Gemini; the API
// key never reaches the page). Audio plumbing ported from ReelGen's KITT live client:
// mic -> AudioWorklet -> 16 kHz PCM16 -> base64; 24 kHz PCM16 playback queue;
// `interrupted` flushes playback (barge-in). Analyser nodes on both directions feed
// the visualizers.

export type LiveStatus = "idle" | "connecting" | "live" | "rolling" | "error";

export type LiveEvent =
  | { type: "status"; status: LiveStatus; detail?: string }
  | { type: "session"; sessionId: string }
  | { type: "transcript"; who: "user" | "assistant"; text: string }
  | { type: "tool"; callId: string; name: string; state: string }
  | { type: "turnComplete" }
  | { type: "interrupted" };

const TOKEN_KEY = "kagent.token";

const WORKLET = `
class KagentMic extends AudioWorkletProcessor {
  constructor() { super(); this.buf = []; this.ratio = Math.max(1, Math.round(sampleRate / 16000)); }
  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    if (!ch) return true;
    for (let i = 0; i < ch.length; i++) this.buf.push(ch[i]);
    const need = 640 * this.ratio;                // 40 ms at 16 kHz
    while (this.buf.length >= need) {
      const group = this.buf.splice(0, need);
      const out = new Int16Array(640);
      for (let j = 0; j < 640; j++) {
        let s = 0;
        for (let k = 0; k < this.ratio; k++) s += group[j * this.ratio + k];
        s = (s / this.ratio) * 32767;
        out[j] = Math.max(-32768, Math.min(32767, s));
      }
      this.port.postMessage(out, [out.buffer]);
    }
    return true;
  }
}
registerProcessor("kagent-mic", KagentMic);
`;

function b64(buf: Int16Array): string {
  const bytes = new Uint8Array(buf.buffer);
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

// Little-endian PCM16 -> float. The <<16>>16 sign extension is required: without it
// every negative sample decodes as a large positive one (screeching).
export function pcm16ToFloat(data: string): Float32Array<ArrayBuffer> {
  const bin = atob(data);
  const out = new Float32Array(new ArrayBuffer((bin.length >> 1) * 4));
  for (let i = 0; i < out.length; i++) {
    const v = ((bin.charCodeAt(2 * i) | (bin.charCodeAt(2 * i + 1) << 8)) << 16) >> 16;
    out[i] = v / 32768;
  }
  return out;
}

export class LiveClient {
  private ws: WebSocket | null = null;
  private micCtx: AudioContext | null = null;
  private playCtx: AudioContext | null = null;
  private stream: MediaStream | null = null;
  private sources: AudioBufferSourceNode[] = [];
  private nextAt = 0;
  private playGain: GainNode | null = null;
  private volume = 0.6;
  /** Muted: the session stays open but no microphone audio is sent. */
  muted = false;
  micAnalyser: AnalyserNode | null = null;
  playAnalyser: AnalyserNode | null = null;
  status: LiveStatus = "idle";

  private onEvent: (e: LiveEvent) => void;

  constructor(onEvent: (e: LiveEvent) => void) {
    this.onEvent = onEvent;
  }

  get active() {
    return this.ws !== null;
  }

  async start(sessionId: string | null, voice?: string) {
    if (this.ws) return;
    this.emitStatus("connecting");
    try {
      this.stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      });
    } catch {
      this.emitStatus("error", "microphone unavailable (permission denied?)");
      return;
    }
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const token = localStorage.getItem(TOKEN_KEY) ?? "";
    const q = new URLSearchParams({ token });
    if (sessionId) q.set("session", sessionId);
    const ws = new WebSocket(`${proto}://${location.host}/live?${q}`);
    this.ws = ws;
    ws.onopen = () => ws.send(JSON.stringify({ type: "start", voice: voice ?? "" }));
    ws.onmessage = (m) => this.onMessage(JSON.parse(String(m.data)));
    ws.onclose = () => this.teardown();
    ws.onerror = () => this.emitStatus("error", "connection to kagentd failed");
  }

  stop() {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ type: "stop" }));
    setTimeout(() => this.teardown(), 300);
  }

  /** Playback volume 0..1 (Gemini's voice is loud at unity gain). */
  setVolume(v: number) {
    this.volume = Math.max(0, Math.min(1, v));
    if (this.playGain) this.playGain.gain.value = this.volume;
  }

  setMuted(m: boolean) {
    this.muted = m;
    this.stream?.getAudioTracks().forEach((t) => (t.enabled = !m));
  }

  sendText(text: string) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ type: "text", text }));
  }

  private emitStatus(status: LiveStatus, detail = "") {
    this.status = status;
    this.onEvent({ type: "status", status, detail });
  }

  private onMessage(msg: Record<string, unknown>) {
    switch (msg.type) {
      case "status":
        this.status = msg.status as LiveStatus;
        if (msg.status === "live") this.openAudio();
        this.onEvent(msg as LiveEvent);
        if (msg.status === "idle" || (msg.status === "error" && msg.detail === "busy")) this.teardown();
        break;
      case "audio":
        this.playChunk(String(msg.data), Number(msg.rate) || 24000);
        break;
      case "interrupted":
        this.flush();
        this.onEvent({ type: "interrupted" });
        break;
      default:
        this.onEvent(msg as LiveEvent);
    }
  }

  private openAudio() {
    if (!this.micCtx && this.stream) {
      const ctx = new AudioContext();
      this.micCtx = ctx;
      const url = URL.createObjectURL(new Blob([WORKLET], { type: "application/javascript" }));
      void ctx.audioWorklet.addModule(url).then(() => {
        URL.revokeObjectURL(url);
        if (!this.stream || this.micCtx !== ctx) return;
        const node = new AudioWorkletNode(ctx, "kagent-mic");
        node.port.onmessage = (e) => {
          if (!this.muted && this.ws?.readyState === WebSocket.OPEN)
            this.ws.send(JSON.stringify({ type: "audio", data: b64(e.data as Int16Array) }));
        };
        const src = ctx.createMediaStreamSource(this.stream);
        this.micAnalyser = ctx.createAnalyser();
        this.micAnalyser.fftSize = 512;
        src.connect(this.micAnalyser);
        src.connect(node);
        // Analysers exist now: let visualizers pick them up.
        this.onEvent({ type: "status", status: this.status });
      });
    }
    if (!this.playCtx) {
      this.playCtx = new AudioContext();
      this.playGain = this.playCtx.createGain();
      this.playGain.gain.value = this.volume;
      this.playAnalyser = this.playCtx.createAnalyser();
      this.playAnalyser.fftSize = 512;
      this.playGain.connect(this.playAnalyser);
      this.playAnalyser.connect(this.playCtx.destination);
      this.nextAt = 0;
    }
  }

  private playChunk(data: string, rate: number) {
    const ctx = this.playCtx;
    if (!ctx || !this.playGain) return;
    const f = pcm16ToFloat(data);
    if (!f.length) return;
    const buf = ctx.createBuffer(1, f.length, rate);
    buf.copyToChannel(f, 0);
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(this.playGain);
    const t = Math.max(ctx.currentTime + 0.03, this.nextAt);
    src.start(t);
    this.nextAt = t + buf.duration;
    this.sources.push(src);
    src.onended = () => {
      this.sources = this.sources.filter((s) => s !== src);
    };
  }

  private flush() {
    for (const s of this.sources) {
      try {
        s.stop();
      } catch {
        /* already stopped */
      }
    }
    this.sources = [];
    this.nextAt = 0;
  }

  private teardown() {
    if (!this.ws && !this.stream) return;
    this.flush();
    try {
      this.ws?.close();
    } catch {
      /* closed */
    }
    this.ws = null;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = null;
    void this.micCtx?.close().catch(() => {});
    this.micCtx = null;
    void this.playCtx?.close().catch(() => {});
    this.playCtx = null;
    this.micAnalyser = null;
    this.playAnalyser = null;
    if (this.status !== "error") this.emitStatus("idle");
  }
}
