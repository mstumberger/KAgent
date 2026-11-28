// Small inline icons (stroke = currentColor) so controls read without text labels.
const P: Record<string, string> = {
  eye: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
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
  gear: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.39a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
  plus: "M12 5v14 M5 12h14",
  back: "M19 12H5 M12 19l-7-7 7-7",
  forward: "M5 12h14 M12 5l7 7-7 7",
  sliders: "M21 4h-7 M10 4H3 M21 12h-9 M8 12H3 M21 20h-5 M12 20H3 M14 2v4 M8 10v4 M16 18v4",
  contrast: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M12 3a9 9 0 0 1 0 18 M12 3v18",
  box: "M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z M12 12l8-4.5 M12 12L4 7.5 M12 12v9",
  globe: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M3 12h18 M12 3a14 14 0 0 1 0 18 M12 3a14 14 0 0 0 0 18",
  monitor: "M3 5h18v11H3z M12 16v3 M8 21h8",
  calendar: "M3 6a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z M3 10h18 M8 2v4 M16 2v4",
  bell: "M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9 M10 21h4",
  shield: "M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6l8-3z M9 12l2 2 4-4",
  book: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20 M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z",
  pencil: "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z M15 5 4 4",
  close: "M18 6 6 18 M6 6l12 12",
  chevron: "M6 9l6 6 6-6",
  check: "M20 6 9 17l-5-5",
  image: "M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z M7 9a2 2 0 1 0 4 0 2 2 0 1 0-4 0z M21 15l-3.086-3.086a2 2 0 0 0-2.828 0L6 21",
  // The drag handle of a row (`grip` is the resize corner of the window).
  handle: "M8 5a1 1 0 1 0 2 0 1 1 0 1 0-2 0z M8 12a1 1 0 1 0 2 0 1 1 0 1 0-2 0z M8 19a1 1 0 1 0 2 0 1 1 0 1 0-2 0z M14 5a1 1 0 1 0 2 0 1 1 0 1 0-2 0z M14 12a1 1 0 1 0 2 0 1 1 0 1 0-2 0z M14 19a1 1 0 1 0 2 0 1 1 0 1 0-2 0z",
  trash: "M3 6h18 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6 M10 11v6 M14 11v6",
  // The sidebar (sidebar.md): its toggle, the folder of a project, a pinned session, a row's menu, the search, the theme.
  sidebar: "M3 5h18v14H3z M9 5v14",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  pin: "M12 17v5 M9 10.8a2 2 0 0 1-1.1 1.8l-1.8.9A2 2 0 0 0 5 15.2V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.8a2 2 0 0 0-1.1-1.7l-1.8-.9A2 2 0 0 1 15 10.8V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z",
  more: "M5 12h.01 M12 12h.01 M19 12h.01",
  search: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M21 21l-4.3-4.3",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z M12 2v2 M12 20v2 M4.9 4.9l1.4 1.4 M17.7 17.7l1.4 1.4 M2 12h2 M20 12h2 M4.9 19.1l1.4-1.4 M17.7 6.3l1.4-1.4",
  // Notices (notices.md) and the work panel (work-windows.md).
  alert: "M12 3 2 20h20L12 3z M12 10v5 M12 18h.01",
  info: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M12 11v5 M12 8h.01",
  bot: "M5 8h14v11H5z M12 8V4 M9 13h.01 M15 13h.01 M3 13v3 M21 13v3",
  terminal: "M4 5h16v14H4z M7 10l3 2-3 2 M12 15h5",
  list: "M9 6h12 M9 12h12 M9 18h12 M4 6h.01 M4 12h.01 M4 18h.01",
  arrowDown: "M12 5v14 M6 13l6 6 6-6",
  paperclip: "M21 12.5 12.6 20.9a5 5 0 0 1-7.1-7.1l9-9a3.3 3.3 0 0 1 4.7 4.7l-9 9a1.7 1.7 0 0 1-2.4-2.4L15.4 8",
  moon: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z",
};

export default function Icon({ name, size = 16 }: { name: keyof typeof P | string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={P[name] ?? ""} />
    </svg>
  );
}
