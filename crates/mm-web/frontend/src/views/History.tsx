import { useState } from "react";
import { useEditHistory, useSessionDetail } from "../api/queries";
import type { EditRecord, EditSessionSummary } from "../api/generated/types";

function EditRow({
  edit,
  path,
}: {
  edit: EditRecord;
  path: string | undefined;
}) {
  return (
    <div className="hist-edit">
      <span className="hist-edit__field">{edit.field_name}</span>
      <span className="hist-edit__path">{path ?? `inode:${edit.inode}`}</span>
      <div className="hist-edit__diff">
        {edit.old_value != null && (
          <span className="hist-edit__old">{edit.old_value}</span>
        )}
        <span className="hist-edit__arrow">&rarr;</span>
        {edit.new_value != null && (
          <span className="hist-edit__new">{edit.new_value}</span>
        )}
      </div>
    </div>
  );
}

function SessionDetail({ sessionId }: { sessionId: string }) {
  const { data, isLoading, error } = useSessionDetail(sessionId);

  if (isLoading) return <div className="view-loading">Loading edits...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;

  return (
    <div className="hist-detail">
      {data.edits.map((edit: EditRecord) => (
        <EditRow
          key={edit.id}
          edit={edit}
          path={data.inode_paths[String(edit.inode)]}
        />
      ))}
    </div>
  );
}

function SessionRow({ session }: { session: EditSessionSummary }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="hist-session">
      <button
        className="hist-session__header"
        onClick={() => setExpanded(!expanded)}
        type="button"
      >
        <span
          className="dir-arrow"
          style={{ transform: expanded ? "rotate(90deg)" : "none" }}
        >
          {"\u25b6"}
        </span>
        <span className="hist-session__date">{session.earliest_at}</span>
        <span className="hist-session__stats">
          {session.edit_count} edit{session.edit_count !== 1 ? "s" : ""} across{" "}
          {session.inode_count} file{session.inode_count !== 1 ? "s" : ""}
        </span>
      </button>
      {expanded && <SessionDetail sessionId={session.session_id} />}
    </div>
  );
}

export function History() {
  const { data, isLoading, error } = useEditHistory();

  if (isLoading) return <div className="view-loading">Loading history...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data || data.sessions.length === 0) {
    return <div className="view-placeholder">No edit history</div>;
  }

  return (
    <div className="history-view">
      <h2>Edit History ({data.sessions.length} sessions)</h2>
      <div className="hist-sessions">
        {data.sessions.map((s) => (
          <SessionRow key={s.session_id} session={s} />
        ))}
      </div>
    </div>
  );
}
