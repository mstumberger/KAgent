// The Activity section (work-windows.md section 5.2): the last 20 notices of the open conversation,
// kept in the page's store for the session's life, so a closed toast is still listed. It is not drawn
// while there is none.
import Icon from "../Icon";
import { activityOf, useActivity, type NoticeKind } from "../toasts";
import WorkSection from "./WorkSection";

const ICON: Record<NoticeKind, string> = { error: "alert", recovery: "check", info: "info" };
const time = (at: number) => new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

export default function ActivitySection({ sessionId }: { sessionId: string }) {
  useActivity();
  const list = activityOf(sessionId);
  if (list.length === 0) return null;
  return (
    <WorkSection id="activity" title="Activity" count={String(list.length)} hasSomething>
      <ul className="activity-list">
        {list.map((a) => (
          <li className={`work-line activity-row ${a.kind}`} key={a.id} data-kind={a.kind} data-level={a.level}>
            <span className="work-line-icon" aria-hidden="true">
              <Icon name={ICON[a.kind]} size={13} />
            </span>
            <span className="activity-level work-line-note">{a.level}</span>
            <span className="activity-text">{a.text}</span>
            {a.count > 1 && <span className="work-line-note activity-count">x{a.count}</span>}
            <time className="work-line-note" dateTime={new Date(a.at).toISOString()}>
              {time(a.at)}
            </time>
          </li>
        ))}
      </ul>
    </WorkSection>
  );
}
