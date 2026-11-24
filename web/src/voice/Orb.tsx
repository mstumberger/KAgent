// Audio-reactive presence visual, driven by the live client's analyser nodes (mic =
// the user, playback = KAgent). Looks:
//  orb       glass sphere with the live waveform inside and a spectrum corona around it
//  spectrum  circular sound spectrum: mirrored bars with peak hold
//  wave      layered flowing waveforms
//  kitt      scanner bar; hal  the red eye
// Whoever is louder sets the colour: ice = KAgent speaking, green = you. With no audio
// the visual breathes slowly; prefers-reduced-motion keeps it nearly still.
import { useEffect, useRef } from "react";

export type OrbTheme = "orb" | "spectrum" | "wave" | "kitt" | "hal";
export const ORB_THEMES: OrbTheme[] = ["orb", "spectrum", "wave", "kitt", "hal"];

/** Older saved names map onto the current looks. */
export function normalizeTheme(t: string | null): OrbTheme {
  if (t === "ring") return "spectrum";
  return (ORB_THEMES as string[]).includes(t ?? "") ? (t as OrbTheme) : "orb";
}

interface Props {
  mic: AnalyserNode | null;
  play: AnalyserNode | null;
  state: string; // idle | connecting | live | muted | error | ...
  theme: OrbTheme;
  size?: number;
}

const BARS = 72;

function cssVar(name: string, fallback: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** "#7cc4ff" + alpha 0..1 -> rgba(). */
function rgba(hex: string, a: number): string {
  const h = hex.replace("#", "");
  const n = parseInt(h.length === 3 ? h.split("").map((c) => c + c).join("") : h.slice(0, 6), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${Math.max(0, Math.min(1, a))})`;
}

export default function Orb({ mic, play, state, theme, size = 280 }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);
  const live = useRef({ mic, play, state, theme });
  useEffect(() => {
    live.current = { mic, play, state, theme };
  }, [mic, play, state, theme]);

  useEffect(() => {
    const canvas = ref.current!;
    const ctx = canvas.getContext("2d")!;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = size * dpr;
    canvas.height = size * dpr;
    ctx.scale(dpr, dpr);
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

    const freq = new Uint8Array(new ArrayBuffer(256));
    const wave = new Uint8Array(new ArrayBuffer(512));
    const bars = new Float32Array(BARS); // smoothed spectrum
    const peaks = new Float32Array(BARS); // peak hold
    let raf = 0;
    let level = 0;
    const t0 = performance.now();
    const ice = cssVar("--ice", "#7cc4ff");
    const you = cssVar("--ok", "#4fd1a5");
    const bad = cssVar("--bad", "#ff6b7d");
    const amber = cssVar("--signal", "#ffb547");

    const loudness = (a: AnalyserNode | null): number => {
      if (!a) return 0;
      a.getByteFrequencyData(freq);
      let s = 0;
      for (let i = 2; i < 96; i++) s += freq[i];
      return Math.min(1, s / (94 * 150));
    };

    const draw = (now: number) => {
      const { mic, play, state, theme } = live.current;
      const t = (now - t0) / 1000;
      const lu = loudness(mic);
      const la = loudness(play);
      const src = la >= lu ? play : mic;
      const color = state === "error" ? bad : state === "muted" ? amber : la >= lu || lu < 0.04 ? ice : you;
      level += (Math.max(lu, la) - level) * 0.2;

      // Spectrum of the active source, smoothed, with slowly falling peaks.
      if (src) src.getByteFrequencyData(freq);
      for (let i = 0; i < BARS; i++) {
        const raw = src ? freq[2 + Math.floor((i / BARS) * 90)] / 255 : 0;
        bars[i] += (raw - bars[i]) * (raw > bars[i] ? 0.5 : 0.12);
        peaks[i] = Math.max(bars[i], peaks[i] - 0.008);
      }
      // Time-domain waveform (or a calm synthetic line when silent).
      const hasWave = !!src && level > 0.02;
      if (src && hasWave) src.getByteTimeDomainData(wave);
      const sample = (k: number, n: number): number => {
        if (hasWave) return (wave[Math.floor((k / n) * (wave.length - 1))] - 128) / 128;
        const idle = reduce ? 0 : 0.06;
        return idle * Math.sin(k / n * Math.PI * 4 + t * 1.6) * Math.sin(t * 0.7 + k * 0.05);
      };

      const c = size / 2;
      ctx.clearRect(0, 0, size, size);
      const breathe = reduce ? 0 : (state === "live" ? 0.025 : 0.012) * Math.sin(t * 1.8);

      if (theme === "orb") {
        // Everything stays inside the square canvas: the corona needs ~0.16 * size
        // beyond the sphere, the halo fades out exactly at the edge (a glow larger than
        // the canvas showed as a square cut-off).
        const r = Math.min(size * (0.27 + 0.07 * level + breathe), c - size * 0.17);
        // Corona: thin spectrum strokes around the sphere.
        ctx.lineCap = "round";
        for (let i = 0; i < BARS; i++) {
          const v = bars[(i < BARS / 2 ? i : BARS - 1 - i)];
          const ang = (i / BARS) * Math.PI * 2 - Math.PI / 2;
          const r0 = r * 1.12;
          const r1 = Math.min(r0 + size * (0.012 + v * 0.13), c - 2);
          ctx.strokeStyle = rgba(color, 0.18 + v * 0.6);
          ctx.lineWidth = Math.max(1.2, size / 160);
          ctx.beginPath();
          ctx.moveTo(c + Math.cos(ang) * r0, c + Math.sin(ang) * r0);
          ctx.lineTo(c + Math.cos(ang) * r1, c + Math.sin(ang) * r1);
          ctx.stroke();
        }
        // Halo.
        const halo = ctx.createRadialGradient(c, c, r * 0.8, c, c, c);
        halo.addColorStop(0, rgba(color, 0.22 + level * 0.3));
        halo.addColorStop(1, rgba(color, 0));
        ctx.fillStyle = halo;
        ctx.beginPath();
        ctx.arc(c, c, c, 0, Math.PI * 2);
        ctx.fill();
        // Glass sphere.
        const g = ctx.createRadialGradient(c - r * 0.35, c - r * 0.4, r * 0.05, c, c, r);
        g.addColorStop(0, "rgba(255,255,255,0.95)");
        g.addColorStop(0.18, rgba(color, 0.95));
        g.addColorStop(0.75, rgba(color, 0.55));
        g.addColorStop(1, rgba(color, 0.25));
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(c, c, r, 0, Math.PI * 2);
        ctx.fill();
        // Waveform inside, clipped to the sphere.
        ctx.save();
        ctx.beginPath();
        ctx.arc(c, c, r * 0.96, 0, Math.PI * 2);
        ctx.clip();
        for (const [amp, alpha, width] of [[0.55, 0.9, 2.2], [0.35, 0.35, 1.2]] as const) {
          ctx.beginPath();
          const n = 96;
          for (let k = 0; k <= n; k++) {
            const x = c - r + (2 * r * k) / n;
            const edge = Math.sin((k / n) * Math.PI); // taper to the rim
            const y = c + sample(k, n) * r * amp * edge * (hasWave ? 1.6 : 1);
            if (k === 0) ctx.moveTo(x, y);
            else ctx.lineTo(x, y);
          }
          ctx.strokeStyle = `rgba(255,255,255,${alpha})`;
          ctx.lineWidth = Math.max(1, (size / 140) * width * 0.5);
          ctx.shadowColor = "rgba(255,255,255,0.8)";
          ctx.shadowBlur = hasWave ? 8 : 2;
          ctx.stroke();
        }
        ctx.restore();
        // Rim light.
        ctx.strokeStyle = "rgba(255,255,255,0.18)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(c, c, r, 0, Math.PI * 2);
        ctx.stroke();
      } else if (theme === "spectrum") {
        const r = size * (0.2 + breathe);
        ctx.lineCap = "round";
        const rot = reduce ? 0 : t * 0.15;
        for (let i = 0; i < BARS; i++) {
          // Mirror so low frequencies sit at the top and bottom.
          const v = bars[i < BARS / 2 ? i * 2 : (BARS - 1 - i) * 2 - 1] ?? 0;
          const p = peaks[i < BARS / 2 ? i * 2 : (BARS - 1 - i) * 2 - 1] ?? 0;
          const ang = (i / BARS) * Math.PI * 2 + rot;
          const len = Math.min(size * (0.015 + v * 0.26), c - r - size * 0.04);
          const cos = Math.cos(ang);
          const sin = Math.sin(ang);
          ctx.strokeStyle = rgba(color, 0.35 + v * 0.65);
          ctx.lineWidth = Math.max(2, size / 70);
          ctx.beginPath();
          ctx.moveTo(c + cos * r, c + sin * r);
          ctx.lineTo(c + cos * (r + len), c + sin * (r + len));
          ctx.stroke();
          const pr = Math.min(r + size * (0.02 + p * 0.26), c - size * 0.02);
          ctx.fillStyle = rgba("#ffffff", 0.25 + p * 0.6);
          ctx.beginPath();
          ctx.arc(c + cos * pr, c + sin * pr, Math.max(1, size / 180), 0, Math.PI * 2);
          ctx.fill();
        }
        const core = ctx.createRadialGradient(c, c, 0, c, c, r * 0.9);
        core.addColorStop(0, rgba(color, 0.55 + level * 0.4));
        core.addColorStop(1, rgba(color, 0));
        ctx.fillStyle = core;
        ctx.beginPath();
        ctx.arc(c, c, r * 0.9, 0, Math.PI * 2);
        ctx.fill();
      } else if (theme === "wave") {
        const n = 120;
        const h = size * 0.34;
        for (let layer = 0; layer < 4; layer++) {
          ctx.beginPath();
          for (let k = 0; k <= n; k++) {
            const x = (size * k) / n;
            const env = Math.sin((k / n) * Math.PI) ** 2;
            const ph = t * (1.2 + layer * 0.4) + layer;
            const base = Math.sin((k / n) * Math.PI * (2 + layer) + ph) * (0.08 + level * 0.9);
            const y = c + (base + sample(k, n) * 0.6) * h * env * (1 - layer * 0.18);
            if (k === 0) ctx.moveTo(x, y);
            else ctx.lineTo(x, y);
          }
          ctx.strokeStyle = rgba(layer === 0 ? "#ffffff" : color, layer === 0 ? 0.85 : 0.55 - layer * 0.1);
          ctx.lineWidth = Math.max(1, size / (layer === 0 ? 110 : 150));
          ctx.shadowColor = rgba(color, 0.9);
          ctx.shadowBlur = layer === 0 ? 10 : 0;
          ctx.stroke();
        }
        ctx.shadowBlur = 0;
      } else if (theme === "kitt") {
        const n = 9;
        const w = size * 0.08;
        const h = size * 0.12;
        const pos = state === "live" && level < 0.05 && !reduce ? (Math.sin(t * 3) + 1) / 2 : -1;
        for (let i = 0; i < n; i++) {
          const x = c + (i - (n - 1) / 2) * (w * 1.15) - w / 2;
          const center = 1 - Math.abs(i - (n - 1) / 2) / ((n - 1) / 2);
          let a = level > 0.05 ? Math.max(0, level * 1.4 - (1 - center) * 0.9) : 0.08;
          if (pos >= 0) a = Math.max(a, 1 - Math.min(1, Math.abs(i / (n - 1) - pos) * 4));
          const hh = h * (0.5 + center * level * 2.2);
          ctx.fillStyle = state === "error" ? bad : "#ff2a1a";
          ctx.globalAlpha = 0.15 + 0.85 * a;
          ctx.fillRect(x, c - hh / 2, w, hh);
        }
        ctx.globalAlpha = 1;
      } else {
        const r = size * 0.36;
        ctx.beginPath();
        ctx.arc(c, c, r, 0, Math.PI * 2);
        ctx.fillStyle = "#111";
        ctx.fill();
        ctx.lineWidth = size * 0.03;
        ctx.strokeStyle = "#999";
        ctx.stroke();
        const glow = 0.35 + level * 0.9 + (state === "live" && !reduce ? 0.08 * Math.sin(t * 2) : 0);
        const g = ctx.createRadialGradient(c, c, 0, c, c, r * 0.9);
        g.addColorStop(0, `rgba(255,220,120,${Math.min(1, glow)})`);
        g.addColorStop(0.15, `rgba(255,40,20,${Math.min(1, glow)})`);
        g.addColorStop(0.6, `rgba(120,0,0,${Math.min(1, glow * 0.6)})`);
        g.addColorStop(1, "rgba(0,0,0,0)");
        ctx.beginPath();
        ctx.arc(c, c, r * 0.9, 0, Math.PI * 2);
        ctx.fillStyle = g;
        ctx.fill();
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [size]);

  return <canvas ref={ref} style={{ width: size, height: size }} className="orb" aria-hidden="true" />;
}
