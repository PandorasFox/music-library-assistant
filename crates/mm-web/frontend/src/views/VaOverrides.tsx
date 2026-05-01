import { useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useVaOverrideReview, queryKeys } from "../api/queries";
import { post } from "../api/client";
import type {
  VaOverrideReviewRow,
  VaOverrideApplication,
} from "../api/generated/types";

function pluralS(n: number): string {
  return n === 1 ? "" : "s";
}

function countNoun(n: number, noun: string): string {
  return `${n} ${noun}${pluralS(n)}`;
}

function sourceLabel(source: VaOverrideReviewRow["source"]): string {
  return source === "ExactAlternative" ? "exact alternative" : "competing proposal";
}

interface RowState {
  selected: boolean;
  /** Edited value (defaults to row.suggested_artist on init). */
  albumartist: string;
}

export function VaOverrides() {
  const { data, isLoading, error, refetch } = useVaOverrideReview();
  const queryClient = useQueryClient();
  const rows = useMemo<VaOverrideReviewRow[]>(() => data?.rows ?? [], [data]);

  // Per-row state keyed on release_id. Initialized from the query payload.
  // When the query refetches and produces new rows, merge them in (preserving
  // any operator edits already in flight for the same release_id).
  const [state, setState] = useState<Record<string, RowState>>({});
  useEffect(() => {
    setState((prev) => {
      const next: Record<string, RowState> = {};
      for (const row of rows) {
        next[row.release_id] = prev[row.release_id] ?? {
          selected: false,
          albumartist: row.suggested_artist,
        };
      }
      return next;
    });
  }, [rows]);

  const [applying, setApplying] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [messageOk, setMessageOk] = useState(true);

  const selectedCount = useMemo(
    () => Object.values(state).filter((s) => s.selected).length,
    [state],
  );

  function toggleRow(release_id: string, value?: boolean) {
    setState((prev) => {
      const existing = prev[release_id];
      if (!existing) return prev;
      return {
        ...prev,
        [release_id]: {
          albumartist: existing.albumartist,
          selected: value ?? !existing.selected,
        },
      };
    });
  }

  function setAlbumArtist(release_id: string, albumartist: string) {
    setState((prev) => {
      const existing = prev[release_id];
      if (!existing) return prev;
      return {
        ...prev,
        [release_id]: { selected: existing.selected, albumartist },
      };
    });
  }

  function selectAll() {
    setState((prev) => {
      const next: Record<string, RowState> = {};
      for (const [id, row] of Object.entries(prev)) {
        next[id] = { albumartist: row.albumartist, selected: true };
      }
      return next;
    });
  }

  function selectNone() {
    setState((prev) => {
      const next: Record<string, RowState> = {};
      for (const [id, row] of Object.entries(prev)) {
        next[id] = { albumartist: row.albumartist, selected: false };
      }
      return next;
    });
  }

  async function handleApply() {
    const applications: VaOverrideApplication[] = Object.entries(state)
      .filter(([, s]) => s.selected && s.albumartist.trim().length > 0)
      .map(([release_id, s]) => ({
        release_id,
        albumartist: s.albumartist.trim(),
      }));
    if (applications.length === 0) {
      setMessage("Nothing selected (or selected rows have empty albumartist).");
      setMessageOk(false);
      return;
    }

    setApplying(true);
    setMessage(null);
    try {
      const res = await post<{
        staged_releases: number;
        staged_inodes: number;
        skipped_releases: number;
        already_matching_inodes: number;
      }>("/tx/apply-va-overrides", { applications });

      const parts = [
        `Staged ${countNoun(res.staged_releases, "VA override")} [${countNoun(res.staged_inodes, "inode")}]`,
      ];
      if (res.already_matching_inodes > 0) {
        parts.push(`${countNoun(res.already_matching_inodes, "inode")} already matching`);
      }
      if (res.skipped_releases > 0) {
        parts.push(`skipped ${countNoun(res.skipped_releases, "release")}`);
      }
      setMessage(parts.join("; "));
      setMessageOk(true);
      // Invalidate so the list reflects what's now staged in the active txn.
      queryClient.invalidateQueries({ queryKey: queryKeys.vaOverrideReview });
      queryClient.invalidateQueries({ queryKey: queryKeys.txDetails });
      refetch();
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
      setMessageOk(false);
    } finally {
      setApplying(false);
    }
  }

  if (isLoading) return <div className="view-loading">Loading VA overrides…</div>;
  if (error) return <div className="view-error">{error.message}</div>;

  if (rows.length === 0) {
    return (
      <div>
        <h2>VA Overrides</h2>
        <div className="cfg-message">No VA-override suggestions outstanding.</div>
      </div>
    );
  }

  return (
    <div>
      <div className="em-review-header">
        <h2>VA Overrides ({rows.length})</h2>
        <div style={{ display: "flex", gap: "0.5rem" }}>
          <button onClick={selectAll} disabled={applying}>Select all</button>
          <button onClick={selectNone} disabled={applying}>Select none</button>
          <button
            onClick={handleApply}
            disabled={applying || selectedCount === 0}
            style={{ fontWeight: 600 }}
          >
            {applying ? "Staging…" : `Stage Apply (${selectedCount})`}
          </button>
        </div>
      </div>

      {message && (
        <div className={`cfg-message ${messageOk ? "cfg-message--ok" : "cfg-message--error"}`}>
          {message}
        </div>
      )}

      <p style={{ color: "var(--mm-text-muted, #888)", marginBottom: "1rem" }}>
        Each suggestion proposes overriding a release's <code>ALBUMARTIST</code> away from
        "Various Artists" to a more useful single name. Edit the value before applying if you
        prefer something different. Applies as a transaction decision — it stages an
        <code>ApplyTagOps</code> on every inode currently packed to the release; you confirm
        in the transaction view as usual.
      </p>

      <div className="em-releases" style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
        {rows.map((row) => {
          const s = state[row.release_id];
          if (!s) return null;
          const edited = s.albumartist !== row.suggested_artist;
          return (
            <div
              key={row.release_id}
              className="em-release"
              style={{
                display: "grid",
                gridTemplateColumns: "auto 1fr auto",
                gap: "0.75rem",
                alignItems: "center",
                padding: "0.5rem",
                borderBottom: "1px solid var(--mm-border, #333)",
              }}
            >
              <input
                type="checkbox"
                checked={s.selected}
                onChange={(e) => toggleRow(row.release_id, e.target.checked)}
                disabled={applying}
              />
              <div>
                <div style={{ fontWeight: 600 }}>{row.release_title}</div>
                <div style={{ fontSize: "0.85rem", color: "var(--mm-text-muted, #888)" }}>
                  {countNoun(row.packed_inode_count, "track")} · source: {sourceLabel(row.source)}
                  {row.current_albumartist != null && (
                    <>
                      {" · current: "}
                      <code>{row.current_albumartist}</code>
                      {!row.albumartist_uniform && " (mixed)"}
                    </>
                  )}
                  {row.current_albumartist == null && " · current: (none)"}
                </div>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: "0.25rem" }}>
                <input
                  type="text"
                  value={s.albumartist}
                  onChange={(e) => setAlbumArtist(row.release_id, e.target.value)}
                  disabled={applying}
                  style={{
                    minWidth: "20rem",
                    fontFamily: "inherit",
                    fontSize: "0.95rem",
                    padding: "0.25rem 0.5rem",
                  }}
                />
                {edited && (
                  <div style={{ fontSize: "0.75rem", color: "var(--mm-text-muted, #888)" }}>
                    suggested: <code>{row.suggested_artist}</code>
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
