import { useState } from "react";
import {
  useExternalMatches,
  useAcoustidMatches,
  useReleaseReview,
} from "../api/queries";
import { post } from "../api/client";
import type {
  ExternalMatchesData,
  AcoustidMatchEntry,
  ReviewableRelease,
} from "../api/generated/types";

// -- Helpers --

function pluralS(n: number): string {
  return n === 1 ? "" : "s";
}

function countNoun(n: number, noun: string): string {
  return `${n} ${noun}${pluralS(n)}`;
}

function formatApprovalSummary(s: {
  staged_releases: number;
  staged_tracks: number;
  skipped_tracks: number;
  skipped_releases: number;
}): string {
  const staged = `Staged ${countNoun(s.staged_releases, "release")} [${countNoun(s.staged_tracks, "track")}]`;
  if (s.skipped_tracks === 0 && s.skipped_releases === 0) {
    return staged;
  }
  return `${staged}; skipped ${countNoun(s.skipped_tracks, "track")} across ${countNoun(s.skipped_releases, "release")}`;
}

// -- Summary page --

function PackingRow({
  label,
  count,
  filter,
  onOpenReview,
}: {
  label: string;
  count: number;
  filter: string;
  onOpenReview: (filter: string) => void;
}) {
  if (count === 0) return null;
  return (
    <div className="em-row em-row--link" onClick={() => onOpenReview(filter)}>
      <span>{label}</span>
      <span className="em-count">{count.toLocaleString()}</span>
    </div>
  );
}

function MatchSummary({
  data,
  onOpenAcoustid,
  onOpenReview,
  onQueueFetch,
}: {
  data: ExternalMatchesData;
  onOpenAcoustid: (tier: string) => void;
  onOpenReview: (filter: string) => void;
  onQueueFetch: (task: string) => void;
}) {
  const totalMatches = data.confidence_buckets.reduce((s, b) => s + b.total, 0);

  return (
    <>
      <div className="em-actions">
        <button onClick={() => onQueueFetch("ExternalFetch")}>
          Queue External Fetch
        </button>
        <button onClick={() => onQueueFetch("CoverArtFetch")}>
          Download Cover Art
        </button>
        <button onClick={() => onQueueFetch("DeezerArtFetch")}>
          Fetch Cover Art via Deezer
        </button>
      </div>

      <section className="em-section">
        <h3>AcoustID Matches ({totalMatches.toLocaleString()})</h3>
        <div className="em-list">
          {data.confidence_buckets.map((b) => (
            <div
              key={b.tier}
              className="em-row em-row--link"
              onClick={() => {
                const tier =
                  b.tier === "Perfect" || b.tier === "VeryHigh" || b.tier === "High"
                    ? "high"
                    : b.tier === "Medium"
                      ? "medium"
                      : "low";
                onOpenAcoustid(tier);
              }}
            >
              <span>{b.tier}</span>
              <span className="em-count">{b.total.toLocaleString()}</span>
            </div>
          ))}
        </div>
      </section>

      <section className="em-section">
        <h3>Release Packing</h3>
        <div className="em-list">
          <PackingRow label="Perfect" count={data.packing_perfect_count} filter="perfect" onOpenReview={onOpenReview} />
          <PackingRow label="Full Match" count={data.packing_full_match_count} filter="full-match" onOpenReview={onOpenReview} />
          <PackingRow label="Singles" count={data.packing_singles_count} filter="singles" onOpenReview={onOpenReview} />
          <PackingRow label="Incomplete" count={data.packing_incomplete_count} filter="incomplete" onOpenReview={onOpenReview} />
          <PackingRow label="Low Confidence" count={data.packing_low_confidence_count} filter="low-confidence" onOpenReview={onOpenReview} />
          {data.packing_knots_count > 0 && (
            <div className="em-row">
              <span>Knots</span>
              <span className="em-count">{data.packing_knots_count}</span>
            </div>
          )}
        </div>
      </section>

      {(data.unsolved_conflict_count > 0 ||
        data.unsolved_no_release_count > 0 ||
        data.unsolved_no_match_count > 0) && (
        <section className="em-section">
          <h3>Unsolved</h3>
          <div className="em-list">
            {data.unsolved_conflict_count > 0 && (
              <div className="em-row"><span>Conflicts</span><span className="em-count">{data.unsolved_conflict_count}</span></div>
            )}
            {data.unsolved_no_release_count > 0 && (
              <div className="em-row"><span>No release</span><span className="em-count">{data.unsolved_no_release_count}</span></div>
            )}
            {data.unsolved_no_match_count > 0 && (
              <div className="em-row"><span>No match</span><span className="em-count">{data.unsolved_no_match_count}</span></div>
            )}
          </div>
        </section>
      )}

      {data.va_override_count > 0 && (
        <section className="em-section">
          <h3>VA Overrides</h3>
          <div className="em-list">
            <div
              className="em-row em-row--link"
              onClick={() => { window.location.hash = "#/va-overrides"; }}
            >
              <span>Suggestions to review</span>
              <span className="em-count">{data.va_override_count}</span>
            </div>
          </div>
        </section>
      )}
    </>
  );
}

// -- AcoustID browse --

function AcoustidBrowse({
  confidence,
  onBack,
}: {
  confidence: string;
  onBack: () => void;
}) {
  const { data, isLoading, error } = useAcoustidMatches(confidence);

  return (
    <div>
      <button className="em-back" onClick={onBack}>&larr; Back</button>
      <h3>AcoustID Matches ({confidence})</h3>
      {isLoading && <div className="view-loading">Loading...</div>}
      {error && <div className="view-error">{error.message}</div>}
      {data && (
        <div className="em-match-list">
          {data.map((m: AcoustidMatchEntry) => (
            <div key={`${m.inode}-${m.recording_id}`} className="em-match">
              <div className="em-match__top">
                <span className="em-match__name">{m.display_name}</span>
                <span className="em-match__conf">
                  {Math.round(m.confidence * 100)}%
                </span>
              </div>
              {(m.recording_artist || m.recording_title) && (
                <div className="em-match__detail">
                  {[m.recording_artist, m.recording_title]
                    .filter(Boolean)
                    .join(" \u2014 ")}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// -- Release review --

function ReleaseCard({
  release,
  onApprove,
}: {
  release: ReviewableRelease;
  onApprove: (ids: string[]) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const pct = Math.round(release.avg_confidence * 100);

  return (
    <div className="em-release">
      <div className="em-release__header">
        <button
          className="em-release__toggle"
          onClick={() => setExpanded(!expanded)}
          type="button"
        >
          <span
            className="dir-arrow"
            style={{ transform: expanded ? "rotate(90deg)" : "none" }}
          >
            {"\u25b6"}
          </span>
          <span className="em-release__title">
            {release.artist} &mdash; {release.title}
          </span>
          <span className="em-release__meta">
            {release.matched_count}/{release.track_count} tracks &middot;{" "}
            {pct}% &middot; {release.category}
          </span>
        </button>
        <button
          className="em-release__approve"
          onClick={() => onApprove([release.release_id])}
        >
          Approve
        </button>
      </div>
      {expanded && (
        <div className="em-release__tracks">
          {release.tracks.map((t) => (
            <div key={`${t.position}-${t.recording_id}`} className="em-track">
              <span className="em-track__pos">{t.position}.</span>
              <span className="em-track__title">{t.mb_title}</span>
              <span className="em-track__match">
                {t.matched_display_name
                  ? `${t.matched_display_name} [${t.confidence != null ? Math.round(t.confidence * 100) : "?"}%]`
                  : "(unmatched)"}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ReleaseReview({
  filter,
  onBack,
}: {
  filter: string;
  onBack: () => void;
}) {
  const { data, isLoading, error } = useReleaseReview(filter);
  const [approving, setApproving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  async function handleApprove(ids: string[]) {
    setApproving(true);
    setMessage(null);
    try {
      const res = await post<{
        staged_releases: number;
        staged_tracks: number;
        skipped_tracks: number;
        skipped_releases: number;
      }>("/tx/approve-releases", { release_ids: ids });
      setMessage(formatApprovalSummary(res));
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setApproving(false);
    }
  }

  const releases = data?.releases ?? [];

  return (
    <div>
      <div className="em-review-header">
        <button className="em-back" onClick={onBack}>&larr; Back</button>
        <h3>Release Review ({filter})</h3>
        {releases.length > 0 && (
          <button
            onClick={() => handleApprove(releases.map((r) => r.release_id))}
            disabled={approving}
          >
            {approving ? "Approving..." : `Approve All (${releases.length})`}
          </button>
        )}
      </div>
      {message && (
        <div className={`cfg-message ${message.startsWith("Staged") ? "cfg-message--ok" : "cfg-message--error"}`}>
          {message}
        </div>
      )}
      {isLoading && <div className="view-loading">Loading...</div>}
      {error && <div className="view-error">{error.message}</div>}
      <div className="em-releases">
        {releases.map((r) => (
          <ReleaseCard key={r.release_id} release={r} onApprove={handleApprove} />
        ))}
      </div>
    </div>
  );
}

// -- Main view --

type SubView =
  | { kind: "summary" }
  | { kind: "acoustid"; confidence: string }
  | { kind: "review"; filter: string };

export function ExternalMatches() {
  const { data, isLoading, error } = useExternalMatches();
  const [subView, setSubView] = useState<SubView>({ kind: "summary" });

  async function queueTask(task: string) {
    try {
      await post("/commands/queue-task", { task });
    } catch (err) {
      alert(err instanceof Error ? err.message : String(err));
    }
  }

  if (isLoading) return <div className="view-loading">Loading external matches...</div>;
  if (error) return <div className="view-error">{error.message}</div>;
  if (!data) return null;

  if (subView.kind === "acoustid") {
    return (
      <div className="em-view">
        <AcoustidBrowse
          confidence={subView.confidence}
          onBack={() => setSubView({ kind: "summary" })}
        />
      </div>
    );
  }

  if (subView.kind === "review") {
    return (
      <div className="em-view">
        <ReleaseReview
          filter={subView.filter}
          onBack={() => setSubView({ kind: "summary" })}
        />
      </div>
    );
  }

  return (
    <div className="em-view">
      <MatchSummary
        data={data}
        onOpenAcoustid={(c) => setSubView({ kind: "acoustid", confidence: c })}
        onOpenReview={(f) => setSubView({ kind: "review", filter: f })}
        onQueueFetch={queueTask}
      />
    </div>
  );
}
