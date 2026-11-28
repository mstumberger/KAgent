// One job of the list (Chat, Vision, Embedding, Speech to text): the recommended row first.
import { GroupCard } from "../ui";
import type { Job, LocalModelsView } from "./api";
import Row, { type Actions } from "./Row";

export default function Group({ job, title, description, view, actions }: { job: Job; title: string; description: string; view: LocalModelsView; actions: Actions }) {
  const rows = view.recommended.filter((m) => m.jobs.includes(job));
  if (rows.length === 0) return null;
  // The Recommended row sits first; the rest keep the order of the list.
  const sorted = [...rows.filter((m) => m.recommendedFor.includes(job)), ...rows.filter((m) => !m.recommendedFor.includes(job))];
  return (
    <GroupCard title={title} description={description}>
      <div className="lm-list" role="list" aria-label={title}>
        {sorted.map((m) => (
          <Row key={m.id} m={m} job={job} view={view} actions={actions} />
        ))}
      </div>
    </GroupCard>
  );
}
