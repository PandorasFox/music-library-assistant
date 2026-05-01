import type { WitchStatus } from "../api/generated/types";
import type { WsState } from "../api/ws";

interface StatusBarProps {
  status: WitchStatus | null;
  wsState: WsState;
}

function WorkIndicator({ status }: { status: WitchStatus }) {
  const { work } = status;

  if (work.state === "Working") {
    const pct =
      work.session_queued > 0
        ? Math.round((work.total_processed / work.session_queued) * 100)
        : 0;
    return (
      <span className="status-work status-work--active">
        Working {pct > 0 ? `${pct}%` : ""} ({work.pending} pending)
      </span>
    );
  }

  if (work.state === "Done") {
    return (
      <span className="status-work status-work--done">
        Done ({work.total_processed} processed)
      </span>
    );
  }

  return <span className="status-work status-work--idle">Idle</span>;
}

function FetchIndicator({ status }: { status: WitchStatus }) {
  if (!status.is_external_fetch_active || !status.external_fetch_progress)
    return null;

  const fp = status.external_fetch_progress;
  const total = fp.acoustid.total + fp.mb.total;
  const processed = fp.acoustid.processed + fp.mb.processed;

  return (
    <span className="status-fetch">
      Fetch {processed}/{total}
    </span>
  );
}

function CoverArtIndicator({ status }: { status: WitchStatus }) {
  if (!status.is_cover_art_fetch_active || !status.cover_art_progress)
    return null;

  const p = status.cover_art_progress;
  return (
    <span className="status-fetch">
      Art {p.processed}/{p.total_releases}
    </span>
  );
}

function DeezerIndicator({ status }: { status: WitchStatus }) {
  if (!status.is_deezer_fetch_active || !status.deezer_progress) return null;
  const p = status.deezer_progress;
  return (
    <span className="status-fetch">
      Deezer {p.processed}/{p.total_dirs}
    </span>
  );
}

function TransactionIndicator({ status }: { status: WitchStatus }) {
  if (!status.transaction) return null;

  return (
    <span className="status-transaction">
      TX: {status.transaction.label} ({status.transaction.decision_count}d /{" "}
      {status.transaction.mutation_count}m)
    </span>
  );
}

export function StatusBar({ status, wsState }: StatusBarProps) {
  return (
    <footer className="statusbar">
      <div className="statusbar__left">
        {wsState === "disconnected" && (
          <span className="status-ws status-ws--disconnected">WS disconnected</span>
        )}
        {wsState === "connecting" && (
          <span className="status-ws status-ws--connecting">WS connecting...</span>
        )}
        {status && <WorkIndicator status={status} />}
        {status && <FetchIndicator status={status} />}
        {status && <CoverArtIndicator status={status} />}
        {status && <DeezerIndicator status={status} />}
      </div>
      <div className="statusbar__right">
        {status && <TransactionIndicator status={status} />}
        {status?.last_error && (
          <span className="status-error" title={status.last_error}>
            Error
          </span>
        )}
        {status && (
          <span className="status-muted">
            {status.startup_state === "Ready"
              ? status.reasoning_level === "Full"
                ? "Ready"
                : `Reasoning: ${status.reasoning_level}`
              : status.startup_state}
          </span>
        )}
      </div>
    </footer>
  );
}
