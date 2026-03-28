import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { get, post, ApiError } from "../api/client";
import { useQuery } from "@tanstack/react-query";

// -- Shared helpers --

async function stageBatch(
  key: unknown,
  label: string,
  mutations: unknown[],
  navigate: (path: string) => void,
  setError: (msg: string | null) => void,
  setStaging: (v: boolean) => void,
) {
  if (mutations.length === 0) return;
  setStaging(true);
  setError(null);
  try {
    try {
      await post("/tx/start", { label });
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 409)) throw e;
    }
    await post("/tx/add", { key, decision: { label, mutations } });
    navigate("/tx");
  } catch (err) {
    setError(err instanceof Error ? err.message : String(err));
  } finally {
    setStaging(false);
  }
}

// -- Tag Canonicity --

interface CanonicityCluster {
  signal_key: string;
  variants: { value: string; files: { inode: number; display_name: string }[] }[];
  confirmed_canonical: string | null;
  suggested_canonical: string | null;
}

interface TagCanonicityData {
  tag_name: string;
  clusters: CanonicityCluster[];
}

export function TagCanonicity({ tagName }: { tagName: string }) {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["tag-canonicity", tagName],
    queryFn: () =>
      get<TagCanonicityData>(
        `/queries/tag-canonicity-resolution?tag_name=${encodeURIComponent(tagName)}&zone=Corpus&filter_existing_canonicals=true`,
      ),
  });

  const [clusterIdx, setClusterIdx] = useState(0);
  const [canonical, setCanonical] = useState("");
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;
  if (!data || data.clusters.length === 0)
    return <div className="view-placeholder">No canonicity clusters for {tagName}</div>;

  const cluster = data.clusters[clusterIdx]!;
  const effCanonical = canonical || cluster.suggested_canonical || "";

  function applyCluster() {
    const ops: unknown[] = [];
    for (const variant of cluster.variants) {
      if (variant.value === effCanonical) continue;
      for (const f of variant.files) {
        ops.push({
          inode: f.inode,
          tag_name: data!.tag_name,
          old_value: variant.value,
          new_value: effCanonical,
        });
      }
    }
    void stageBatch(
      { TagCanonicity: { tag_name: data!.tag_name, cluster_index: clusterIdx } },
      `Canonicalize ${data!.tag_name}: "${effCanonical}"`,
      [{ ApplyTagOps: { ops, zone: "Corpus" } }],
      navigate, setErr, setStaging,
    );
  }

  return (
    <div className="resolve-view">
      <h2>Tag Canonicity: {data.tag_name}</h2>
      <div className="cr-nav">
        <button disabled={clusterIdx === 0} onClick={() => { setClusterIdx(clusterIdx - 1); setCanonical(""); }}>Prev</button>
        <span>Cluster {clusterIdx + 1} / {data.clusters.length}</span>
        <button disabled={clusterIdx >= data.clusters.length - 1} onClick={() => { setClusterIdx(clusterIdx + 1); setCanonical(""); }}>Next</button>
      </div>
      {err && <div className="form-error">{err}</div>}
      <div className="cr-field">
        <label>Canonical value</label>
        <input
          type="text"
          value={effCanonical}
          onChange={(e) => setCanonical(e.target.value)}
        />
      </div>
      <div className="cr-variants">
        {cluster.variants.map((v) => (
          <div
            key={v.value}
            className={`cr-variant ${v.value === effCanonical ? "cr-variant--selected" : ""}`}
            onClick={() => setCanonical(v.value)}
          >
            <span className="cr-variant__value">"{v.value}"</span>
            <span className="cr-variant__count">{v.files.length} files</span>
          </div>
        ))}
      </div>
      <div className="resolve-actions">
        <button onClick={applyCluster} disabled={staging || !effCanonical}>
          {staging ? "Staging..." : "Apply Canonical"}
        </button>
      </div>
    </div>
  );
}

// -- Compound Split --

interface CompoundSplitCluster {
  tag_name: string;
  compound_value: string;
  split_parts: string[];
  matching_parts: string[];
  files: { inode: number; display_name: string }[];
}

interface CompoundSplitData {
  groups: CompoundSplitCluster[];
}

export function CompoundSplit({ tagName, safe }: { tagName: string; safe: boolean }) {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["compound-split", tagName, safe],
    queryFn: () =>
      get<CompoundSplitData>(
        `/queries/compound-split-resolution?tag_name=${encodeURIComponent(tagName)}&zone=Corpus&safe_only=${safe}`,
      ),
  });

  const [groupIdx, setGroupIdx] = useState(0);
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;
  if (!data || data.groups.length === 0)
    return <div className="view-placeholder">No compound splits for {tagName}</div>;

  const group = data.groups[groupIdx]!;

  function applyGroup() {
    const ops: unknown[] = [];
    const tagUpper = group.tag_name.toUpperCase();
    const isArtistTag = tagUpper === "ARTIST" || tagUpper === "ALBUMARTIST";

    for (const f of group.files) {
      if (isArtistTag) {
        // Navidrome singular/plural convention: leave the singular tag intact
        // (it's the display string) and add individual values as the plural form.
        const pluralTag = group.tag_name + "S";
        for (const part of group.split_parts) {
          ops.push({ inode: f.inode, tag_name: pluralTag, old_value: null, new_value: part });
        }
      } else {
        // Non-artist: replace compound value with first part, add remaining parts.
        const [first, ...rest] = group.split_parts;
        if (first != null) {
          ops.push({ inode: f.inode, tag_name: group.tag_name, old_value: group.compound_value, new_value: first });
        }
        for (const part of rest) {
          ops.push({ inode: f.inode, tag_name: group.tag_name, old_value: null, new_value: part });
        }
      }
    }
    const keyVariant = safe ? "CompoundSplitSafe" : "CompoundSplitReview";
    const label = isArtistTag
      ? `Add ${group.tag_name}S from "${group.compound_value}" → ${group.split_parts.join(", ")}`
      : `Split ${group.tag_name}: "${group.compound_value}" → ${group.split_parts.join(", ")}`;
    void stageBatch(
      { [keyVariant]: { tag_name: group.tag_name, cluster_index: groupIdx } },
      label,
      [{ ApplyTagOps: { ops, zone: "Corpus" } }],
      navigate, setErr, setStaging,
    );
  }

  return (
    <div className="resolve-view">
      <h2>Compound Split: {tagName} ({safe ? "Safe" : "Review"})</h2>
      <div className="cr-nav">
        <button disabled={groupIdx === 0} onClick={() => setGroupIdx(groupIdx - 1)}>Prev</button>
        <span>Group {groupIdx + 1} / {data.groups.length}</span>
        <button disabled={groupIdx >= data.groups.length - 1} onClick={() => setGroupIdx(groupIdx + 1)}>Next</button>
      </div>
      {err && <div className="form-error">{err}</div>}
      <div className="cr-split-info">
        <div className="cr-split-original">"{group.compound_value}"</div>
        <div className="cr-split-arrow">&darr;</div>
        <div className="cr-split-parts">
          {group.split_parts.map((p, i) => (
            <span key={i} className={`cr-split-part ${group.matching_parts.includes(p) ? "cr-split-part--match" : ""}`}>
              {p}
            </span>
          ))}
        </div>
        <div className="resolve-muted">{group.files.length} files affected</div>
      </div>
      <div className="resolve-actions">
        <button onClick={applyGroup} disabled={staging}>
          {staging ? "Staging..." : "Apply Split"}
        </button>
        <button className="cfg-toolbar__discard" onClick={() => setGroupIdx(Math.min(groupIdx + 1, data.groups.length - 1))}>
          Skip
        </button>
      </div>
    </div>
  );
}

// -- Missing Album Singles --

interface SingleTrackInfo { inode: number; title: string; path: string; }
interface MissingAlbumGroup { key: string; data: { artist: string; tracks: SingleTrackInfo[] } }

export function MissingAlbum() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["missing-album-singles"],
    queryFn: () => get<MissingAlbumGroup[]>("/queries/missing-album-single-signals"),
  });

  const [groupIdx, setGroupIdx] = useState(0);
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;
  if (!data || data.length === 0)
    return <div className="view-placeholder">No missing album singles</div>;

  const group = data[groupIdx]!;

  function perTrackTitle() {
    const ops = group.data.tracks.map((t) => ({
      inode: t.inode,
      tag_name: "ALBUM",
      old_value: null,
      new_value: t.title,
    }));
    void stageBatch(
      { MissingAlbum: { group_index: groupIdx } },
      `Missing album: ${group.data.artist} (per-track titles)`,
      [{ ApplyTagOps: { ops, zone: "Corpus" } }],
      navigate, setErr, setStaging,
    );
  }

  function allSingles() {
    const ops = group.data.tracks.map((t) => ({
      inode: t.inode,
      tag_name: "ALBUM",
      old_value: null,
      new_value: "Singles",
    }));
    void stageBatch(
      { MissingAlbum: { group_index: groupIdx } },
      `Missing album: ${group.data.artist} → "Singles"`,
      [{ ApplyTagOps: { ops, zone: "Corpus" } }],
      navigate, setErr, setStaging,
    );
  }

  return (
    <div className="resolve-view">
      <h2>Missing Album: {group.data.artist}</h2>
      <div className="cr-nav">
        <button disabled={groupIdx === 0} onClick={() => setGroupIdx(groupIdx - 1)}>Prev</button>
        <span>Group {groupIdx + 1} / {data.length}</span>
        <button disabled={groupIdx >= data.length - 1} onClick={() => setGroupIdx(groupIdx + 1)}>Next</button>
      </div>
      {err && <div className="form-error">{err}</div>}
      <div className="cr-tracks">
        {group.data.tracks.map((t) => (
          <div key={t.inode} className="resolve-row">
            <span>{t.title}</span>
            <span className="resolve-muted">{t.path}</span>
          </div>
        ))}
      </div>
      <div className="resolve-actions">
        <button onClick={perTrackTitle} disabled={staging}>Per-Track Title</button>
        <button onClick={allSingles} disabled={staging}>All "Singles"</button>
      </div>
    </div>
  );
}

// -- Disc Extraction --

interface DiscFileEntry { inode: number; path: string; original_value: string; cleaned_value: string; source_tag: string; }
interface DiscExtractionGroup { description: string; disc_value: string; files: DiscFileEntry[]; }
interface DiscExtractionData { groups: DiscExtractionGroup[]; }

export function DiscExtraction() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["disc-extraction"],
    queryFn: () => get<DiscExtractionData>("/queries/disc-extraction-data?map_letters_to_numbers=false"),
  });

  const [groupIdx, setGroupIdx] = useState(0);
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;
  if (!data || data.groups.length === 0)
    return <div className="view-placeholder">No disc extraction candidates</div>;

  const group = data.groups[groupIdx]!;

  function apply() {
    const ops: unknown[] = [];
    for (const f of group.files) {
      // Replace source tag value with cleaned value
      ops.push({ inode: f.inode, tag_name: f.source_tag, old_value: f.original_value, new_value: f.cleaned_value });
      // Add disc number
      ops.push({ inode: f.inode, tag_name: "DISCNUMBER", old_value: null, new_value: group.disc_value });
    }
    void stageBatch(
      { DiscExtraction: { group_index: groupIdx } },
      `Disc extraction: ${group.description}`,
      [{ ApplyTagOps: { ops, zone: "Corpus" } }],
      navigate, setErr, setStaging,
    );
  }

  return (
    <div className="resolve-view">
      <h2>Disc Extraction</h2>
      <div className="cr-nav">
        <button disabled={groupIdx === 0} onClick={() => setGroupIdx(groupIdx - 1)}>Prev</button>
        <span>Group {groupIdx + 1} / {data.groups.length}</span>
        <button disabled={groupIdx >= data.groups.length - 1} onClick={() => setGroupIdx(groupIdx + 1)}>Next</button>
      </div>
      {err && <div className="form-error">{err}</div>}
      <div className="resolve-muted" style={{ marginBottom: 8 }}>{group.description} &rarr; DISCNUMBER={group.disc_value}</div>
      <div className="cr-tracks">
        {group.files.map((f) => (
          <div key={f.inode} className="resolve-row">
            <span className="resolve-muted">{f.original_value}</span>
            <span className="deploy-stale-arrow">&rarr;</span>
            <span>{f.cleaned_value}</span>
          </div>
        ))}
      </div>
      <div className="resolve-actions">
        <button onClick={apply} disabled={staging}>{staging ? "Staging..." : "Apply"}</button>
        <button className="cfg-toolbar__discard" onClick={() => setGroupIdx(Math.min(groupIdx + 1, data.groups.length - 1))}>Skip</button>
      </div>
    </div>
  );
}

// -- Directory Cluster (cross-source overlaps + release overlaps) --

export function DirectoryCluster({ queryKey, queryUrl, title }: {
  queryKey: string;
  queryUrl: string;
  title: string;
}) {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: [queryKey],
    queryFn: () => get<{
      clusters: {
        cluster_key: string;
        directories: {
          path_suffix: string;
          inodes: number[];
          paths: string[];
          format_summary: string;
          can_stash_dupes: boolean;
        }[];
        overlap_count: number;
      }[];
    }>(queryUrl),
  });

  const [clusterIdx, setClusterIdx] = useState(0);
  const [selected, setSelected] = useState<number | null>(null);
  const [staging, setStaging] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;
  if (!data || data.clusters.length === 0)
    return <div className="view-placeholder">No clusters</div>;

  const cluster = data.clusters[clusterIdx]!;

  function navigateCluster(delta: number) {
    const next = clusterIdx + delta;
    if (next >= 0 && next < data!.clusters.length) {
      setClusterIdx(next);
      setSelected(null);
    }
  }

  function stashSelected() {
    if (selected == null) return;
    const dir = cluster.directories[selected];
    if (!dir) return;

    const mutations = dir.paths.flatMap((path: string, i: number) => [
      { StashFromZone: { path, stash_name: "overlaps" } },
      { DropFromIndex: { path, inode: dir.inodes[i] ?? null, zone: "corpus" } },
    ]);

    void stageBatch(
      { DirectoryCluster: { cluster_index: clusterIdx } },
      "Stash overlapping directory",
      mutations,
      navigate, setErr, setStaging,
    );
  }

  function markExpected() {
    const parts = cluster.cluster_key.split("|");
    const source_a = parts[0] ?? "";
    const source_b = parts[1] ?? "";

    void stageBatch(
      { DirectoryCluster: { cluster_index: clusterIdx } },
      "Mark expected overlap",
      [{ EmitExpectedOverlap: { source_a, source_b } }],
      navigate, setErr, setStaging,
    );
  }

  return (
    <div className="resolve-view">
      <h2>{title}</h2>
      <div className="cr-nav">
        <button disabled={clusterIdx === 0} onClick={() => navigateCluster(-1)}>Prev</button>
        <span>Cluster {clusterIdx + 1} / {data.clusters.length}</span>
        <button disabled={clusterIdx >= data.clusters.length - 1} onClick={() => navigateCluster(1)}>Next</button>
      </div>
      {err && <div className="form-error">{err}</div>}
      <div className="cr-cluster-key">{cluster.cluster_key} ({cluster.overlap_count} overlaps)</div>
      <div className="cr-dir-list">
        {cluster.directories.map((dir, i) => (
          <div
            key={dir.path_suffix}
            className={`cr-dir-item ${selected === i ? "cr-dir-item--selected" : ""} ${!dir.can_stash_dupes ? "cr-dir-item--locked" : ""}`}
            onClick={() => dir.can_stash_dupes && setSelected(selected === i ? null : i)}
          >
            <span className="cr-dir-radio">
              {dir.can_stash_dupes ? (selected === i ? "\u25c9" : "\u25cb") : "\u2014"}
            </span>
            <span className="cr-dir-path">{dir.path_suffix}</span>
            <span className="cr-dir-fmt">{dir.format_summary}</span>
            <span className="cr-dir-count">{dir.paths.length} files</span>
          </div>
        ))}
      </div>
      <div className="resolve-actions">
        <button onClick={stashSelected} disabled={staging || selected == null}>
          {staging ? "Staging..." : "Stash Selected"}
        </button>
        <button onClick={markExpected} disabled={staging}>
          Mark Expected
        </button>
        <button className="cfg-toolbar__discard" onClick={() => navigateCluster(1)}>
          Skip
        </button>
      </div>
    </div>
  );
}

// -- Tag Canonicity Picker (lists tags with cluster counts, drills into per-tag resolution) --

export function TagCanonicityPicker() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["insights-for-picker"],
    queryFn: () => get<{ bucket_placeholder: { tag_canonicity: { tag_name: string; cluster_count: number; _total_tracks: number }[] } }>("/queries/insights"),
  });

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;

  const entries = data?.bucket_placeholder.tag_canonicity ?? [];
  if (entries.length === 0) return <div className="view-placeholder">No canonicity clusters</div>;

  return (
    <div className="resolve-view">
      <h2>Tag Canonicity</h2>
      <div className="cr-picker-list">
        {entries.map((e) => (
          <div
            key={e.tag_name}
            className="cr-picker-row"
            onClick={() => navigate(`/resolve/tag-canonicity/${encodeURIComponent(e.tag_name)}`)}
          >
            <span className="cr-picker-name">{e.tag_name}</span>
            <span className="cr-picker-count">{e.cluster_count} clusters</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// -- Compound Tag Picker (lists tags with safe/review counts) --

export function CompoundPicker() {
  const navigate = useNavigate();
  const { data, isLoading, error } = useQuery({
    queryKey: ["insights-for-compound-picker"],
    queryFn: () => get<{ bucket_placeholder: { compound_tags: { tag_name: string; safe_count: number; review_count: number }[] } }>("/queries/insights"),
  });

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;

  const entries = data?.bucket_placeholder.compound_tags ?? [];
  if (entries.length === 0) return <div className="view-placeholder">No compound tags</div>;

  return (
    <div className="resolve-view">
      <h2>Compound Tags</h2>
      <div className="cr-picker-list">
        {entries.map((e) => (
          <div key={e.tag_name} className="cr-picker-group">
            <span className="cr-picker-name">{e.tag_name}</span>
            {e.safe_count > 0 && (
              <span
                className="cr-picker-link"
                onClick={() => navigate(`/resolve/compound-split/${encodeURIComponent(e.tag_name)}`)}
              >
                {e.safe_count} safe
              </span>
            )}
            {e.review_count > 0 && (
              <span
                className="cr-picker-link cr-picker-link--review"
                onClick={() => navigate(`/resolve/compound-review/${encodeURIComponent(e.tag_name)}`)}
              >
                {e.review_count} review
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

// -- Manual Review (shared for RedundantDuplicate, DeployConflict, MetadataDuplicate, SameRecordingDifferentRelease) --

interface ReviewFileEntry { corpus_path: string; inode: number; context: string; }
interface ReviewGroup { label: string; files: ReviewFileEntry[]; signal_key: string | null; }
interface ManualReviewData { groups: ReviewGroup[]; }

export function ManualReview({ kind }: { kind: string }) {
  const { data, isLoading, error } = useQuery({
    queryKey: ["manual-review", kind],
    queryFn: () => get<ManualReviewData>(`/queries/manual-review-data?kind=${kind}`),
  });

  const [groupIdx, setGroupIdx] = useState(0);

  if (isLoading) return <div className="view-loading">Loading...</div>;
  if (error) return <div className="view-error">{(error as Error).message}</div>;
  if (!data || data.groups.length === 0)
    return <div className="view-placeholder">No {kind} groups</div>;

  const group = data.groups[groupIdx]!;

  return (
    <div className="resolve-view">
      <h2>Manual Review: {kind}</h2>
      <div className="cr-nav">
        <button disabled={groupIdx === 0} onClick={() => setGroupIdx(groupIdx - 1)}>Prev</button>
        <span>Group {groupIdx + 1} / {data.groups.length}: {group.label}</span>
        <button disabled={groupIdx >= data.groups.length - 1} onClick={() => setGroupIdx(groupIdx + 1)}>Next</button>
      </div>
      <div className="cr-tracks">
        {group.files.map((f) => (
          <div key={f.inode} className="resolve-row resolve-row--col">
            <span>{f.corpus_path}</span>
            <span className="resolve-muted">{f.context}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
