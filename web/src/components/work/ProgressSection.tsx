// The Progress section (work-windows.md section 5.2): the todo list of the session, as TodoPanel
// draws it, without a box of its own.
import { useStore } from "../../store";
import TodoPanel from "../TodoPanel";
import WorkSection from "./WorkSection";

export default function ProgressSection({ sessionId }: { sessionId: string }) {
  const items = useStore().todos[sessionId] ?? [];
  const done = items.filter((item) => item.status === "completed").length;
  return (
    <WorkSection id="progress" title="Progress" count={items.length === 0 ? undefined : `${done} of ${items.length}`} hasSomething={items.length > 0}>
      {items.length === 0 ? <div className="work-empty">No todo list</div> : <TodoPanel sessionId={sessionId} />}
    </WorkSection>
  );
}
