// The settings pages (ui-settings.md section 3): the menu, the deep-link check and the
// render switch all come from this list. A new page is one file under sections/ plus
// one entry here, and its id in `ui.rs` SECTIONS.
import type { ComponentType } from "react";
import General from "./sections/General";
import Appearance from "./sections/Appearance";
import ModelsSection from "./sections/models/ModelsSection";
import VoiceSection from "./sections/Voice";
import BrowserSection from "./sections/Browser";
import Desktop from "./sections/Desktop";
import Watchers from "./sections/Watchers";
import WebSection from "./sections/Web";
import Skills from "./sections/Skills";
import Memory from "./sections/Memory";
import Tools from "./sections/Tools";
import Subagents from "./sections/Subagents";
import Hooks from "./sections/Hooks";
import TasksSection from "./sections/Tasks";
import Telegram from "./sections/Telegram";
import CalendarSection from "./sections/Calendar";
import LocalModels from "./sections/LocalModels";

export type SectionGroup = "Basics" | "Agent capabilities" | "Channels";

/** What the shell passes to a page: the title and description of its entry. */
export interface SectionProps {
  title: string;
  description: string;
}

export interface Section {
  id: string;
  title: string;
  group: SectionGroup;
  /** One line under the title of the page. */
  description: string;
  /** Other names of the page, shown as the tooltip of its menu item. */
  keywords: string[];
  /** Icon of the menu item (components/Icon.tsx). */
  icon: string;
  component: ComponentType<SectionProps>;
}

export const SECTIONS: Section[] = [
  {
    id: "general",
    title: "General",
    group: "Basics",
    description: "Who KAgent is to you, how it starts, and how it may act as root.",
    keywords: ["persona", "name", "language", "startup", "greeting", "widget", "root helper", "privilege", "sudo"],
    icon: "sliders",
    component: General,
  },
  {
    id: "appearance",
    title: "Appearance",
    group: "Basics",
    description: "Dark, light, or as the system is set.",
    keywords: ["theme", "dark", "light", "auto", "system", "colours"],
    icon: "contrast",
    component: Appearance,
  },
  {
    id: "models",
    title: "Models",
    group: "Basics",
    description: "The providers, their models, and which model does which job. Keys stay in your desktop keyring.",
    keywords: ["provider", "API key", "chat model", "fallback", "vision", "fast", "temperature", "tokens", "Gemini", "Ollama"],
    icon: "box",
    component: ModelsSection,
  },
  {
    id: "local-models",
    title: "Local models",
    group: "Basics",
    description: "What this machine can run, the models KAgent recommends for each job, and a Download button. They run in Ollama.",
    keywords: ["ollama", "gpu", "vram", "download", "gemma", "qwen", "llama", "vision", "embedding", "offline", "local"],
    icon: "bot",
    component: LocalModels,
  },
  {
    id: "voice",
    title: "Voice",
    group: "Basics",
    description: "The live model, the voice and the look of a voice session.",
    keywords: ["live model", "Gemini live", "orb", "pause media", "microphone"],
    icon: "mic",
    component: VoiceSection,
  },
  {
    id: "browser",
    title: "Browser",
    group: "Basics",
    description: "The DevTools port and binary of the agent browser.",
    keywords: ["Brave", "Chrome", "DevTools", "port", "binary"],
    icon: "globe",
    component: BrowserSection,
  },
  {
    id: "desktop",
    title: "Desktop and apps",
    group: "Basics",
    description: "The editor that opens your code, and the folders that hold your projects.",
    keywords: ["editor", "VS Code", "projects", "folders", "apps"],
    icon: "monitor",
    component: Desktop,
  },
  {
    id: "watchers",
    title: "Watchers",
    group: "Basics",
    description: "When watchers hold back notifications that can wait.",
    keywords: ["quiet hours", "notifications", "alerts", "night"],
    icon: "bell",
    component: Watchers,
  },
  {
    id: "web",
    title: "Web",
    group: "Basics",
    description: "The search engine behind web_search, and the key or instance it needs.",
    keywords: ["search", "web", "DuckDuckGo", "Brave", "SearXNG", "fetch", "engine"],
    icon: "globe",
    component: WebSection,
  },
  {
    id: "skills",
    title: "Skills",
    group: "Agent capabilities",
    description: "What KAgent knows how to do: the playbooks it ships with and the skills you or the agent write.",
    keywords: ["skills", "playbooks", "SKILL.md", "slash", "load_skill", "skill_save"],
    icon: "book",
    component: Skills,
  },
  {
    id: "memory",
    title: "Memory",
    group: "Agent capabilities",
    description: "What KAgent remembers about you and this machine from one session to the next.",
    keywords: ["memory", "remember", "recall", "facts", "preferences", "MEMORY.md", "memory_save"],
    icon: "talk",
    component: Memory,
  },
  {
    id: "tools",
    title: "Tools and permissions",
    group: "Agent capabilities",
    description: "What the agent may do without asking: the permission mode, rules, per-tool overrides and tool groups.",
    keywords: ["permission", "approval", "always allow", "override", "groups", "MCP", "read-only", "full control"],
    icon: "shield",
    component: Tools,
  },
  {
    id: "agents",
    title: "Subagents",
    group: "Agent capabilities",
    description: "The profiles a subagent runs with, and how many of them work at once.",
    keywords: ["subagent", "agent", "profile", "background", "parallel", "explore", "task", "max concurrent"],
    icon: "talk",
    component: Subagents,
  },
  {
    id: "tasks",
    title: "Automations",
    group: "Agent capabilities",
    description: "What a scheduled task does when it does not say: approvals, where the result goes, its time limit, and how many run at once.",
    keywords: ["tasks", "automations", "schedule", "timeout", "time limit", "deliver", "approval", "max concurrent", "cron"],
    icon: "bell",
    component: TasksSection,
  },
  {
    id: "calendar",
    title: "Calendar",
    group: "Agent capabilities",
    description: "Google Calendar: what KAgent may see and add, places for events, and the morning report.",
    keywords: ["calendar", "google", "events", "meeting", "schedule", "oauth", "maps", "places", "morning report", "koledar"],
    icon: "calendar",
    component: CalendarSection,
  },
  {
    id: "hooks",
    title: "Hooks",
    group: "Agent capabilities",
    description: "Commands of your own that run when the agent starts a session, is asked something, calls a tool, answers or raises an alert.",
    keywords: ["hooks", "command", "script", "PreToolUse", "PostToolUse", "guard", "notify", "event", "decision"],
    icon: "shield",
    component: Hooks,
  },
  {
    id: "telegram",
    title: "Telegram",
    group: "Channels",
    description: "Talk to KAgent from your phone, and get alerts, approvals and automation results in Telegram.",
    keywords: ["bot", "BotFather", "token", "pairing", "chat", "notifications", "approvals", "phone"],
    icon: "talk",
    component: Telegram,
  },
];

/** The order of the menu groups. A group without a page is not shown. */
export const GROUPS: SectionGroup[] = ["Basics", "Agent capabilities", "Channels"];

/** The page used last is kept here. */
export const REMEMBERED = "kagent.settings.section";

/** The page `id` names, else the one used last, else `general`. */
export function currentSection(id: string | null | undefined): Section {
  const remembered = localStorage.getItem(REMEMBERED);
  return SECTIONS.find((x) => x.id === id) ?? SECTIONS.find((x) => x.id === remembered) ?? SECTIONS[0];
}
