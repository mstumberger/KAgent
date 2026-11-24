// Small inline icons (stroke = currentColor) so controls read without text labels.
const P: Record<string, string> = {
  mic: "M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3Z M5 11a7 7 0 0 0 14 0 M12 18v3",
  micOff: "M9 9v3a3 3 0 0 0 5.1 2.1 M15 9.3V6a3 3 0 0 0-5.7-1.3 M5 11a7 7 0 0 0 11.6 5.3 M19 11a7 7 0 0 1-.6 2.8 M12 18v3 M3 3l18 18",
  stop: "M7 7h10v10H7z",
  talk: "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16Z M12 9v6 M9 10.5v3 M15 10.5v3",
  volume: "M4 9v6h4l5 4V5L8 9H4Z M16.5 8.5a5 5 0 0 1 0 7",
  look: "M12 3a9 9 0 1 0 0 18c1 0 1.5-.8 1.5-1.6 0-.8-.6-1.2-.6-2 0-.9.7-1.6 1.6-1.6H17a4 4 0 0 0 4-4C21 6.8 17 3 12 3Z M7.5 11.5h.01 M10 7.5h.01 M14.5 7.5h.01",
  shrink: "M4 14h6v6 M20 10h-6V4 M14 10l7-7 M3 21l7-7",
  expand: "M15 3h6v6 M9 21H3v-6 M21 3l-7 7 M3 21l7-7",
  tray: "M5 19h14",
  max: "M5 5h14v14H5z",
  grip: "M14 20l6-6 M8 20l12-12",
};

export default function Icon({ name, size = 16 }: { name: keyof typeof P | string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={P[name] ?? ""} />
    </svg>
  );
}
