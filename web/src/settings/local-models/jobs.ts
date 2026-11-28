// The four jobs of a local model, in the order the page lists them (local-models.md section 4).
import type { Job } from "./api";

export const JOBS: { job: Job; title: string; description: string }[] = [
  { job: "chat", title: "Chat", description: "Answers your questions and runs tools. Only a model that calls tools reliably can be the Brain; the others help as a fallback." },
  { job: "vision", title: "Vision", description: "Reads screenshots and photos, for screen watch and 'look at my screen'." },
  { job: "embedding", title: "Embedding", description: "Turns text into numbers so it can be searched by meaning." },
  { job: "stt", title: "Speech to text", description: "KAgent cannot use a local speech model yet (planned); these are listed for when it can." },
];
