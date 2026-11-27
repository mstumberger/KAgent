// The tool groups as the settings pages show them (tools.md): the id on the chip, what the
// group holds as its tooltip. `agent` is the only group whose name does not say what it
// holds, so its chip carries a short label too.

const ABOUT: Record<string, string> = {
  system: "System facts: CPU, memory, disks, processes, logs",
  kernel: "Kernel: modules, parameters, messages",
  services: "Services and units",
  hardware: "Hardware: devices, sensors, power",
  skills: "Skills: load and save playbooks",
  tasks: "Scheduled tasks and reminders",
  agent: "The agent's own work: the todo list, plan mode and subagents",
  memory: "Memory: remember and recall",
  files: "Files: read, search, write",
  web: "Web: fetch and search",
  security: "Security: intrusion checks",
  change: "Changes: packages, undo",
  desktop: "Desktop: apps, windows, media, browser, typing",
  shell: "Shell commands",
  trace: "eBPF tracing",
  router: "The router (MikroTik)",
  other: "Tools outside the groups",
};

/** The text of a group's chip. */
export function groupLabel(group: string): string {
  return group === "agent" ? "agent: todo, plan, subagents" : group;
}

/** The tooltip of a group's chip. */
export function groupAbout(group: string): string | undefined {
  return ABOUT[group];
}
