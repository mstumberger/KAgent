// The Changes tab of a task (workflows.md section 8): the diff of the files its worker changed, in the diff view.
import { useEffect, useState } from "react";
import DiffView from "../../components/diff/DiffView";
import type { Diff } from "../../components/diff/model";
import { errorMessage, runsApi } from "./api";

export default function ChangesTab({ runId, task, stamp }: { runId: string; task: string; stamp: string }) {
  const [diff, setDiff] = useState<Diff | null>(null);
  const [files, setFiles] = useState<string[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    runsApi.diff(runId, task).then(
      (r) => {
        if (!live) return;
        setDiff(r.diff);
        setFiles(r.files);
        setError("");
      },
      (e) => {
        if (!live) return;
        setDiff(null);
        setError(errorMessage(e));
      },
    );
    return () => {
      live = false;
    };
  }, [runId, task, stamp]);
  return (
    <div className="rn-changes">
      {error && <div className="rn-error" role="alert">{error}</div>}
      {diff && diff.files.length === 0 && <p className="rn-none">No changes in {files.length > 0 ? files.join(", ") : "the files of this task"} against the last commit.</p>}
      {diff && diff.files.length > 0 && <DiffView diff={diff} label={`Changes of ${task}`} />}
    </div>
  );
}
