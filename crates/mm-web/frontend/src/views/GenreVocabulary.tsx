import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  queryKeys,
  useGenreVocabulary,
  useUnresolvedGenreObservations,
} from "../api/queries";
import { post } from "../api/client";
import type {
  GenreNameEntry,
  GenreVocabularyOp,
  UnresolvedGenreRow,
} from "../api/generated/types";

const SOURCE_LABELS: Record<number, string> = {
  1: "FileTagImport",
  2: "MusicBrainz",
  3: "Discogs",
  4: "AudiomuseInferred",
  5: "Manual",
  6: "Implied",
};

function pluralS(n: number): string {
  return n === 1 ? "" : "s";
}

function countNoun(n: number, noun: string): string {
  return `${n} ${noun}${pluralS(n)}`;
}

/** Pending "map this unresolved raw value to genres {N…}" op the operator
 *  has assembled but not yet submitted. Keyed by raw_value (unresolved
 *  values are unique-by-raw in the DB queue). The Set carries every selected
 *  canonical target — multi-target mapping is supported so a compound raw
 *  like "alternative, pop, rock" can resolve to all three canonicals. */
interface PendingMap {
  raw_value: string;
  target_ids: Set<number>;
}

export function GenreVocabulary() {
  const vocab = useGenreVocabulary();
  const unresolved = useUnresolvedGenreObservations();
  const queryClient = useQueryClient();

  const entries: GenreNameEntry[] = useMemo(
    () => vocab.data?.entries ?? [],
    [vocab.data],
  );
  const unresolvedRows: UnresolvedGenreRow[] = useMemo(
    () => unresolved.data?.rows ?? [],
    [unresolved.data],
  );

  // Operator-edited target id per unresolved raw_value.
  const [pending, setPending] = useState<Record<string, PendingMap>>({});
  const [applying, setApplying] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [messageOk, setMessageOk] = useState(true);

  // Filter input for the vocabulary panel.
  const [vocabFilter, setVocabFilter] = useState("");

  const filteredEntries = useMemo(() => {
    if (!vocabFilter.trim()) return entries;
    const needle = vocabFilter.toLowerCase();
    return entries.filter(
      (e) =>
        e.canonical_name.toLowerCase().includes(needle) ||
        e.display_name.toLowerCase().includes(needle) ||
        e.aliases.some((a) => a.toLowerCase().includes(needle)),
    );
  }, [entries, vocabFilter]);

  // Each pending raw_value with N target ids produces N AddAlias ops. The
  // backend's composite-PK genre_aliases table accepts them as N distinct
  // rows so resolve_genre will return all N canonical ids when ingesting.
  const selectedOps: GenreVocabularyOp[] = useMemo(() => {
    const out: GenreVocabularyOp[] = [];
    for (const p of Object.values(pending)) {
      for (const gid of p.target_ids) {
        out.push({ AddAlias: { alias: p.raw_value, genre_id: gid } });
      }
    }
    return out;
  }, [pending]);

  // Count of unresolved rows the operator has mapped to at least one target,
  // for the "Stage N edits" / "Mapping M rows" indicator.
  const mappedRowCount = useMemo(
    () =>
      Object.values(pending).filter((p) => p.target_ids.size > 0).length,
    [pending],
  );

  function toggleTarget(raw_value: string, target_id: number) {
    setPending((prev) => {
      const existing = prev[raw_value];
      const next = new Set(existing?.target_ids ?? []);
      if (next.has(target_id)) {
        next.delete(target_id);
      } else {
        next.add(target_id);
      }
      return {
        ...prev,
        [raw_value]: { raw_value, target_ids: next },
      };
    });
  }

  function clearRow(raw_value: string) {
    setPending((prev) => {
      const next = { ...prev };
      delete next[raw_value];
      return next;
    });
  }

  function clearPending() {
    setPending({});
  }

  async function handleStage() {
    if (selectedOps.length === 0) {
      setMessage("Nothing mapped yet — pick a canonical target on at least one row.");
      setMessageOk(false);
      return;
    }
    setApplying(true);
    setMessage(null);
    try {
      const res = await post<{ staged_ops: number }>(
        "/tx/edit-genre-vocabulary",
        { ops: selectedOps },
      );
      setMessage(
        `Staged ${countNoun(res.staged_ops, "vocabulary edit")} on the active transaction.`,
      );
      setMessageOk(true);
      // The mutation's `additional_computations()` queues a re-run of
      // ImportGenresFromTags on confirm — until confirm, the unresolved queue
      // still shows the entries. Invalidate anyway so a refresh post-confirm
      // catches up promptly.
      queryClient.invalidateQueries({ queryKey: queryKeys.txDetails });
      clearPending();
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
      setMessageOk(false);
    } finally {
      setApplying(false);
    }
  }

  if (vocab.isLoading || unresolved.isLoading) {
    return <div className="view-loading">Loading genre vocabulary…</div>;
  }
  if (vocab.error) return <div className="view-error">{vocab.error.message}</div>;
  if (unresolved.error)
    return <div className="view-error">{unresolved.error.message}</div>;

  return (
    <div>
      <div className="em-review-header">
        <h2>Genre Vocabulary</h2>
        <div style={{ display: "flex", gap: "0.5rem" }}>
          <button onClick={clearPending} disabled={applying || selectedOps.length === 0}>
            Clear pending
          </button>
          <button
            onClick={handleStage}
            disabled={applying || selectedOps.length === 0}
            style={{ fontWeight: 600 }}
          >
            {applying
              ? "Staging…"
              : `Stage ${countNoun(selectedOps.length, "alias")} (${countNoun(mappedRowCount, "row")})`}
          </button>
        </div>
      </div>

      {message && (
        <div
          className={`cfg-message ${messageOk ? "cfg-message--ok" : "cfg-message--error"}`}
        >
          {message}
        </div>
      )}

      <p style={{ color: "var(--mm-text-muted, #888)", marginBottom: "1rem" }}>
        Curate the canonical genre vocabulary and resolve unmapped values
        observed during ingestion. For each unresolved raw value, pick one
        OR MORE canonicals it should map to — a compound tag like
        "alternative, pop, rock" can legitimately resolve to all three. Each
        selected target stages a separate <code>AddAlias</code> op; the
        importer will then write one <code>inode_genres</code> row per
        target on the next pass.
      </p>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1fr 1fr",
          gap: "1.5rem",
          alignItems: "flex-start",
        }}
      >
        {/* === Unresolved queue (left) === */}
        <section>
          <h3 style={{ marginTop: 0 }}>
            Unresolved values ({unresolvedRows.length})
          </h3>
          {unresolvedRows.length === 0 ? (
            <div className="cfg-message">
              Nothing unresolved. Run a corpus scan to populate.
            </div>
          ) : (
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: "0.25rem",
              }}
            >
              {unresolvedRows.map((row) => {
                const selected =
                  pending[row.raw_value]?.target_ids ?? new Set<number>();
                return (
                  <div
                    key={`${row.raw_value}:${row.source}`}
                    style={{
                      padding: "0.4rem 0.5rem",
                      borderBottom: "1px solid var(--mm-border, #333)",
                    }}
                  >
                    <div
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        alignItems: "baseline",
                        gap: "0.5rem",
                      }}
                    >
                      <div>
                        <div style={{ fontWeight: 600 }}>
                          <code>{row.raw_value}</code>
                        </div>
                        <div
                          style={{
                            fontSize: "0.8rem",
                            color: "var(--mm-text-muted, #888)",
                          }}
                        >
                          seen {countNoun(row.observation_count, "time")} ·
                          source: {SOURCE_LABELS[row.source] ?? `#${row.source}`}
                          {selected.size > 0 && (
                            <>
                              {" · → "}
                              <strong>{countNoun(selected.size, "target")}</strong>
                            </>
                          )}
                        </div>
                      </div>
                      {selected.size > 0 && (
                        <button
                          onClick={() => clearRow(row.raw_value)}
                          disabled={applying}
                          style={{ fontSize: "0.75rem" }}
                        >
                          clear
                        </button>
                      )}
                    </div>
                    <div
                      style={{
                        display: "flex",
                        flexWrap: "wrap",
                        gap: "0.25rem",
                        marginTop: "0.35rem",
                      }}
                    >
                      {entries.map((e) => {
                        const on = selected.has(e.id);
                        return (
                          <button
                            key={e.id}
                            onClick={() => toggleTarget(row.raw_value, e.id)}
                            disabled={applying}
                            title={
                              on
                                ? `unselect ${e.display_name}`
                                : `map "${row.raw_value}" → ${e.display_name}`
                            }
                            style={{
                              padding: "0.1rem 0.4rem",
                              border: "1px solid var(--mm-border, #333)",
                              borderRadius: "999px",
                              background: on
                                ? "var(--mm-chip-active-bg, #4a4)"
                                : "var(--mm-chip-bg, #222)",
                              color: on
                                ? "var(--mm-chip-active-fg, #fff)"
                                : undefined,
                              fontSize: "0.78rem",
                              cursor: "pointer",
                            }}
                          >
                            {e.display_name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </section>

        {/* === Vocabulary (right) === */}
        <section>
          <h3 style={{ marginTop: 0 }}>
            Canonical names ({entries.length})
          </h3>
          <input
            type="text"
            placeholder="Filter by canonical, display, or alias…"
            value={vocabFilter}
            onChange={(e) => setVocabFilter(e.target.value)}
            style={{ width: "100%", marginBottom: "0.5rem", padding: "0.25rem 0.5rem" }}
          />
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: "0.25rem",
              maxHeight: "60vh",
              overflowY: "auto",
            }}
          >
            {filteredEntries.map((entry) => (
              <div
                key={entry.id}
                style={{
                  padding: "0.4rem 0.5rem",
                  borderBottom: "1px solid var(--mm-border, #333)",
                }}
              >
                <div
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    gap: "0.5rem",
                  }}
                >
                  <span style={{ fontWeight: 600 }}>{entry.display_name}</span>
                  <span
                    style={{
                      fontSize: "0.8rem",
                      color: "var(--mm-text-muted, #888)",
                    }}
                  >
                    ledger: {entry.ledger_row_count}
                  </span>
                </div>
                {entry.aliases.length > 0 && (
                  <div
                    style={{
                      fontSize: "0.85rem",
                      color: "var(--mm-text-muted, #888)",
                      display: "flex",
                      flexWrap: "wrap",
                      gap: "0.25rem",
                      marginTop: "0.15rem",
                    }}
                  >
                    {entry.aliases.map((a) => (
                      <code
                        key={a}
                        style={{
                          padding: "0 0.25rem",
                          background: "var(--mm-chip-bg, #222)",
                          borderRadius: "3px",
                        }}
                      >
                        {a}
                      </code>
                    ))}
                  </div>
                )}
                {entry.implies_parents.length > 0 && (
                  <div
                    style={{
                      fontSize: "0.75rem",
                      color: "var(--mm-text-muted, #888)",
                      marginTop: "0.15rem",
                    }}
                  >
                    implies parents: {entry.implies_parents.join(", ")}
                  </div>
                )}
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
