// The skills API (api.md section 6): the types and the calls of the skills library,
// its import and its git sources.
import { api } from "./api";

/** What POST and PUT `/api/skills` send; PUT leaves out what stays. */
export interface SkillDraft {
  name?: string;
  description?: string;
  whenToUse?: string;
  body?: string;
}

export interface SkillView {
  name: string;
  description: string;
  whenToUse?: string;
  source: "bundled" | "user" | "imported";
  /** The skill's folder; `realPath` is it with every link resolved. */
  path: string;
  realPath: string;
  enabled: boolean;
  /** A skill of the owner hides it (same name, higher row of skills.md section 2). */
  shadowed: boolean;
  sizeBytes: number;
  /** Only `get` and the answers of writes carry it; the list does not. */
  body?: string;
  uses: number;
  lastUsed?: string;
  symlink: boolean;
  /** When `SKILL.md` was last written, RFC 3339. */
  updatedAt?: string;
  /** An imported skill: where it came from (S21). */
  importedFrom?: { source: string; path: string; hash: string };
}

/** A skill of another tool's folder, as the import dialog lists it (S21). */
export interface ImportCandidate {
  name: string;
  description: string;
  /** The skill's folder; `realPath` is it with links resolved. */
  path: string;
  realPath: string;
  /** A skill has the same folder or the same name. */
  installed: boolean;
  /** The name of that skill. */
  duplicateOf?: string;
}

/** A folder of another tool that was looked at; `exists` is false when the tool is not installed. */
export interface ImportSource {
  agent: string;
  path: string;
  exists: boolean;
  skills: ImportCandidate[];
}

export type ImportMode = "copy" | "symlink";

/** Why a skill was not imported or installed. */
export interface SkippedSkill {
  name: string;
  reason: string;
}

/** What an import did. `disabled` names the skills that arrived switched off. */
export interface ImportResult {
  imported: string[];
  skipped: SkippedSkill[];
  disabled: string[];
}

/** A git source (api.md `SourceView`). */
export interface SourceView {
  id: string;
  name: string;
  /** An https:// address, or the path of a folder. */
  url: string;
  /** Branch, tag or commit; empty is the default branch. */
  ref: string;
  /** The folder inside the repository that holds the skills; empty is its root. */
  subdir: string;
  commit?: string;
  lastSync?: string;
  lastError?: string;
  skills: { name: string; hash: string; syncedAt: string }[];
}

/** What adding a source (its first install), or syncing one, did. */
export interface SourceReport {
  added: string[];
  updated?: string[];
  removed?: string[];
  skipped: SkippedSkill[];
}

/** What `POST /api/skills/sources` sends. */
export interface SourceForm {
  url: string;
  ref?: string;
  subdir?: string;
  name?: string;
}

/** One suggestion of the catalog. */
export interface CatalogEntry {
  name: string;
  url: string;
  subdir: string;
  note: string;
}

export const skillsApi = {
  /** The library. A daemon without the route (before S20) gives an empty list. */
  list: async (): Promise<SkillView[]> => {
    const r = await api.get<{ skills?: SkillView[] }>("/api/skills");
    return Array.isArray(r?.skills) ? r.skills : [];
  },
  get: (name: string) => api.get<SkillView>(`/api/skills/${encodeURIComponent(name)}`),
  create: (draft: SkillDraft) => api.post<SkillView>("/api/skills", draft),
  update: (name: string, draft: SkillDraft) => api.put<SkillView>(`/api/skills/${encodeURIComponent(name)}`, draft),
  remove: (name: string) => api.del(`/api/skills/${encodeURIComponent(name)}`),
  setEnabled: (name: string, enabled: boolean) => api.put<SkillView>(`/api/skills/${encodeURIComponent(name)}/enabled`, { enabled }),
  rescan: () => api.post<{ count: number }>("/api/skills/rescan", {}),
  // ---- import and sources (S21)
  /** The folders of other tools under the home directory. */
  importSources: async (): Promise<ImportSource[]> => {
    const r = await api.get<{ sources?: ImportSource[] }>("/api/skills/import-sources");
    return Array.isArray(r?.sources) ? r.sources : [];
  },
  import: (path: string, names: string[], mode: ImportMode) => api.post<ImportResult>("/api/skills/import", { path, names, mode }),
  sources: async (): Promise<{ sources: SourceView[]; catalog: CatalogEntry[] }> => {
    const r = await api.get<{ sources?: SourceView[]; catalog?: CatalogEntry[] }>("/api/skills/sources");
    return { sources: Array.isArray(r?.sources) ? r.sources : [], catalog: Array.isArray(r?.catalog) ? r.catalog : [] };
  },
  /** Clones (an address) or reads a folder, and installs the skills switched off. */
  addSource: (form: SourceForm) => api.post<SourceView & SourceReport>("/api/skills/sources", form),
  syncSource: (id: string) => api.post<SourceReport>(`/api/skills/sources/${encodeURIComponent(id)}/sync`, {}),
  removeSource: (id: string, removeSkills: boolean) => api.del(`/api/skills/sources/${encodeURIComponent(id)}`, { removeSkills }),
};
