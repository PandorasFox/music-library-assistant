import { useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  queryKeys,
  useGenrePromotionReview,
  useGenrePromotionInodeDetail,
  useGenreCoverageSummary,
} from "../api/queries";
import { post } from "../api/client";
import type {
  GenrePromotionApplication,
  GenrePromotionChip,
  GenrePromotionReviewRow,
  GenrePromotionStagingSummary,
} from "../api/generated/types";

const SOURCE_LABELS: Record<number, string> = {
  1: "FT", // FileTagImport
  2: "MB",
  3: "DG",
  4: "AM", // Audiomuse
  5: "M",  // Manual
  6: "→",  // Implied
};

function pluralS(n: number): string {
  return n === 1 ? "" : "s";
}

function countNoun(n: number, noun: string): string {
  return `${n} ${noun}${pluralS(n)}`;
}

function chipKey(c: GenrePromotionChip): string {
  return `${c.genre_id}:${c.kind}`;
}

function sourceBadges(bits: number): string[] {
  const out: string[] = [];
  for (let s = 1; s <= 6; s++) {
    if ((bits >> (s - 1)) & 1) {
      out.push(SOURCE_LABELS[s] ?? `?${s}`);
    }
  }
  return out;
}

interface RowState {
  selected: boolean;
  /** Set of "genre_id:kind" chip keys the operator toggled OFF. */
  excluded: Set<string>;
  /** Expanded per-inode detail view. */
  expanded: boolean;
}

export function GenrePromotion() {
  const review = useGenrePromotionReview();
  const summary = useGenreCoverageSummary();
  const queryClient = useQueryClient();

  const rows: GenrePromotionReviewRow[] = useMemo(
    () => review.data?.rows ?? [],
    [review.data],
  );

  // Per-release UI state keyed by release_id.
  const [state, setState] = useState<Record<string, RowState>>({});
  useEffect(() => {
    setState((prev) => {
      const next: Record<string, RowState> = {};
      for (const row of rows) {
        next[row.release_id] = prev[row.release_id] ?? {
          selected: false,
          excluded: new Set(),
          expanded: false,
        };
      }
      return next;
    });
  }, [rows]);

  const [filter, setFilter] = useState<string>("");
  const [showDiscogsOnly, setShowDiscogsOnly] = useState<boolean>(false);
  const [applying, setApplying] = useState<boolean>(false);
  const [message, setMessage] = useState<string | null>(null);
  const [messageOk, setMessageOk] = useState<boolean>(true);

  const filteredRows = useMemo(() => {
    let xs = rows;
    if (showDiscogsOnly) {
      xs = xs.filter((r) => (r.sources_bits >> 2) & 1);
    }
    if (filter.trim()) {
      const needle = filter.toLowerCase();
      xs = xs.filter(
        (r) =>
          r.release_title.toLowerCase().includes(needle) ||
          r.release_artist.toLowerCase().includes(needle) ||
          r.proposed_chips.some((c) =>
            c.canonical_name.toLowerCase().includes(needle),
          ),
      );
    }
    return xs;
  }, [rows, filter, showDiscogsOnly]);

  const selectedCount = useMemo(
    () =>
      filteredRows.filter((r) => state[r.release_id]?.selected ?? false).length,
    [filteredRows, state],
  );

  function toggleSelected(release_id: string) {
    setState((p) => ({
      ...p,
      [release_id]: {
        ...(p[release_id] ?? { selected: false, excluded: new Set(), expanded: false }),
        selected: !(p[release_id]?.selected ?? false),
      },
    }));
  }

  function toggleExcluded(release_id: string, chip: GenrePromotionChip) {
    setState((p) => {
      const existing = p[release_id] ?? {
        selected: false,
        excluded: new Set<string>(),
        expanded: false,
      };
      const k = chipKey(chip);
      const next = new Set(existing.excluded);
      if (next.has(k)) {
        next.delete(k);
      } else {
        next.add(k);
      }
      return { ...p, [release_id]: { ...existing, excluded: next } };
    });
  }

  function toggleExpanded(release_id: string) {
    setState((p) => ({
      ...p,
      [release_id]: {
        ...(p[release_id] ?? { selected: false, excluded: new Set(), expanded: false }),
        expanded: !(p[release_id]?.expanded ?? false),
      },
    }));
  }

  function selectAllVisible() {
    setState((p) => {
      const next = { ...p };
      for (const r of filteredRows) {
        const e = next[r.release_id];
        if (e) next[r.release_id] = { ...e, selected: true };
      }
      return next;
    });
  }

  function selectNone() {
    setState((p) => {
      const next: Record<string, RowState> = {};
      for (const [id, e] of Object.entries(p)) {
        next[id] = { ...e, selected: false };
      }
      return next;
    });
  }

  function buildApplications(): GenrePromotionApplication[] {
    return filteredRows
      .filter((r) => state[r.release_id]?.selected)
      .map((r) => {
        const excluded = state[r.release_id]?.excluded ?? new Set<string>();
        const excludedPairs: [number, number][] = [];
        for (const k of excluded) {
          const parts = k.split(":").map(Number);
          const gid = parts[0];
          const kind = parts[1];
          if (gid != null && !Number.isNaN(gid) && kind != null && !Number.isNaN(kind)) {
            excludedPairs.push([gid, kind]);
          }
        }
        // inodes = packed inode count's worth — the server has them, but the
        // API requires the client supply the inode list. We fetch via the
        // detail query lazily when expanding; for the un-expanded promote
        // path the inodes come from the detail. For now: ask the server to
        // promote the FULL release by sending an empty inodes list and
        // letting the server fan it out via the bulk endpoint instead.
        // (Equivalent semantic; cheaper for the typical "promote everything"
        // case where the operator hasn't expanded any rows.)
        return {
          release_id: r.release_id,
          inodes: [], // server-side fanout
          excluded: excludedPairs,
        };
      });
  }

  async function handleStage() {
    const applications = buildApplications();
    if (applications.length === 0) {
      setMessage("Nothing selected.");
      setMessageOk(false);
      return;
    }
    // The server resolves `inodes: []` to "all packed inodes for this
    // release", so the client doesn't need per-row detail fetches just to
    // stage. Chip exclusions ride through as `excluded` pairs.
    setApplying(true);
    setMessage(null);
    try {
      const res = await post<GenrePromotionStagingSummary>(
        "/tx/promote-genres",
        { applications },
      );

      const parts = [
        `Staged ${countNoun(res.staged_releases, "release")} [${countNoun(res.staged_inodes, "inode")}]`,
      ];
      if (res.already_matching_inodes > 0) {
        parts.push(`${countNoun(res.already_matching_inodes, "inode")} already matching`);
      }
      if (res.inodes_without_ledger > 0) {
        parts.push(`${countNoun(res.inodes_without_ledger, "inode")} without ledger`);
      }
      setMessage(parts.join("; "));
      setMessageOk(true);
      queryClient.invalidateQueries({ queryKey: queryKeys.txDetails });
      queryClient.invalidateQueries({ queryKey: queryKeys.genreCoverageSummary });
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
      setMessageOk(false);
    } finally {
      setApplying(false);
    }
  }

  if (review.isLoading) {
    return <div className="view-loading">Loading promotion review…</div>;
  }
  if (review.error) {
    return <div className="view-error">{review.error.message}</div>;
  }

  const cov = summary.data;

  return (
    <div>
      <div className="em-review-header">
        <h2>Genre Promotion ({filteredRows.length})</h2>
        <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
          <button onClick={selectAllVisible} disabled={applying}>
            Select all visible
          </button>
          <button onClick={selectNone} disabled={applying}>
            Select none
          </button>
          <button
            onClick={handleStage}
            disabled={applying || selectedCount === 0}
            style={{ fontWeight: 600 }}
          >
            {applying
              ? "Staging…"
              : `Stage ${countNoun(selectedCount, "release")}`}
          </button>
        </div>
      </div>

      {cov && (
        <div
          style={{
            display: "flex",
            gap: "1.5rem",
            padding: "0.5rem 0",
            fontSize: "0.85rem",
            color: "var(--mm-text-muted, #888)",
            flexWrap: "wrap",
          }}
        >
          <span>
            ledger: <strong>{cov.inodes_with_any_ledger}</strong> /{" "}
            {cov.total_audio_inodes}
          </span>
          <span>
            Discogs-confirmed: <strong>{cov.inodes_with_discogs_ledger}</strong>
          </span>
          <span>
            FileTag: <strong>{cov.inodes_with_filetag_ledger}</strong>
          </span>
          <span>
            already promoted: <strong>{cov.inodes_already_promoted}</strong>
          </span>
          <span>
            pending: <strong>{cov.pending_promotion}</strong>
          </span>
          <span>
            unresolved: <strong>{cov.unresolved_observations}</strong>
          </span>
        </div>
      )}

      {message && (
        <div
          className={`cfg-message ${
            messageOk ? "cfg-message--ok" : "cfg-message--error"
          }`}
        >
          {message}
        </div>
      )}

      <div style={{ display: "flex", gap: "0.5rem", margin: "0.5rem 0" }}>
        <input
          type="text"
          placeholder="Filter title / artist / chip name…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          style={{ flex: 1, padding: "0.25rem 0.5rem" }}
        />
        <label style={{ display: "flex", alignItems: "center", gap: "0.25rem" }}>
          <input
            type="checkbox"
            checked={showDiscogsOnly}
            onChange={(e) => setShowDiscogsOnly(e.target.checked)}
          />
          Discogs-confirmed only
        </label>
      </div>

      <p style={{ color: "var(--mm-text-muted, #888)" }}>
        Each row groups by release. The chip strip is the union of every
        proposed (genre, kind) across the release's packed inodes — chip
        toggle excludes uniformly. Expand a release to load per-inode
        details (lazy). Stage selected releases as one transaction; the
        ApplyTagOps executor chain-emits disk flushes per inode automatically.
      </p>

      <div className="em-releases" style={{ display: "flex", flexDirection: "column" }}>
        {filteredRows.map((row) => {
          const s = state[row.release_id];
          if (!s) return null;
          const excludedCount = s.excluded.size;
          return (
            <div
              key={row.release_id}
              style={{
                padding: "0.5rem",
                borderBottom: "1px solid var(--mm-border, #333)",
              }}
            >
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: "auto 1fr auto auto",
                  gap: "0.5rem",
                  alignItems: "center",
                }}
              >
                <input
                  type="checkbox"
                  checked={s.selected}
                  onChange={() => toggleSelected(row.release_id)}
                  disabled={applying}
                />
                <div>
                  <div style={{ fontWeight: 600 }}>{row.release_title}</div>
                  <div
                    style={{
                      fontSize: "0.8rem",
                      color: "var(--mm-text-muted, #888)",
                    }}
                  >
                    {row.release_artist} ·{" "}
                    {countNoun(row.packed_inode_count, "track")}
                    {row.current_genre_summary != null && (
                      <>
                        {" · current: "}
                        <code>{row.current_genre_summary}</code>
                        {!row.current_genre_uniform && " (mixed)"}
                      </>
                    )}
                  </div>
                </div>
                <div style={{ display: "flex", gap: "0.25rem", flexWrap: "wrap" }}>
                  {row.proposed_chips.map((chip) => {
                    const k = chipKey(chip);
                    const excluded = s.excluded.has(k);
                    return (
                      <button
                        key={k}
                        onClick={() => toggleExcluded(row.release_id, chip)}
                        disabled={applying}
                        title={`${chip.canonical_name} · ${
                          chip.kind === 0 ? "Genre" : "Style"
                        } · ${chip.inode_count} inode${pluralS(chip.inode_count)} · ${
                          sourceBadges(chip.sources_bits).join(" ") || "?"
                        }`}
                        style={{
                          padding: "0.1rem 0.4rem",
                          border: "1px solid var(--mm-border, #333)",
                          borderRadius: "999px",
                          background:
                            chip.kind === 1
                              ? "var(--mm-chip-style-bg, #2a2235)"
                              : "var(--mm-chip-bg, #222)",
                          textDecoration: excluded ? "line-through" : "none",
                          opacity: excluded ? 0.5 : 1.0,
                          fontSize: "0.8rem",
                          cursor: "pointer",
                        }}
                      >
                        {chip.canonical_name}
                        <span
                          style={{
                            marginLeft: "0.25rem",
                            fontSize: "0.65rem",
                            color: "var(--mm-text-muted, #888)",
                          }}
                        >
                          {sourceBadges(chip.sources_bits).join("")}
                        </span>
                      </button>
                    );
                  })}
                </div>
                <button
                  onClick={() => toggleExpanded(row.release_id)}
                  disabled={applying}
                  style={{ fontSize: "0.8rem" }}
                >
                  {s.expanded ? "Collapse" : "Expand"}
                </button>
              </div>
              {excludedCount > 0 && (
                <div
                  style={{
                    fontSize: "0.75rem",
                    color: "var(--mm-warning, #ffb86c)",
                    paddingLeft: "1.5rem",
                  }}
                >
                  {countNoun(excludedCount, "chip")} excluded from this release
                </div>
              )}
              {s.expanded && (
                <InodeDetailPanel release_id={row.release_id} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function InodeDetailPanel({ release_id }: { release_id: string }) {
  const detail = useGenrePromotionInodeDetail(release_id);
  if (detail.isLoading) {
    return (
      <div style={{ padding: "0.5rem 1.5rem", fontSize: "0.8rem" }}>
        Loading inode detail…
      </div>
    );
  }
  if (detail.error) {
    return (
      <div
        style={{
          padding: "0.5rem 1.5rem",
          fontSize: "0.8rem",
          color: "var(--mm-error, #ff5555)",
        }}
      >
        {detail.error.message}
      </div>
    );
  }
  const rows = detail.data?.rows ?? [];
  return (
    <div
      style={{
        padding: "0.5rem 1.5rem 0.5rem 2rem",
        fontSize: "0.8rem",
        background: "var(--mm-panel-bg, rgba(255,255,255,0.02))",
      }}
    >
      {rows.length === 0 && <em>No packed inodes.</em>}
      {rows.map((ir) => (
        <div
          key={ir.inode}
          style={{
            display: "grid",
            gridTemplateColumns: "1fr auto",
            gap: "0.5rem",
            padding: "0.15rem 0",
          }}
        >
          <span
            style={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              fontFamily: "monospace",
            }}
          >
            {ir.path}
          </span>
          <span style={{ display: "flex", gap: "0.25rem", flexWrap: "wrap" }}>
            {ir.chips.map((c) => (
              <code
                key={`${c.genre_id}:${c.kind}`}
                style={{
                  padding: "0 0.25rem",
                  background:
                    c.kind === 1
                      ? "var(--mm-chip-style-bg, #2a2235)"
                      : "var(--mm-chip-bg, #222)",
                  borderRadius: "3px",
                }}
              >
                {c.canonical_name}
              </code>
            ))}
          </span>
        </div>
      ))}
    </div>
  );
}
