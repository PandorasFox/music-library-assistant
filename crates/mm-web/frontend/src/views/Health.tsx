import { useNavigate } from "react-router-dom";
import { useInsights } from "../api/queries";
import type {
  CorpusFilesBucket,
  TagSquashBucket,
  OtherSignalEntry,
} from "../api/generated/types";

type Severity = "healthy" | "info" | "warning" | "critical";

function severity(count: number, threshold: number = 0): Severity {
  if (count === 0) return "healthy";
  if (count <= threshold) return "info";
  return "warning";
}

function SignalRow({
  label,
  count,
  level,
  link,
}: {
  label: string;
  count: number;
  level: Severity;
  link?: string;
}) {
  const navigate = useNavigate();
  if (count === 0 && level === "healthy") return null;
  return (
    <div
      className={`signal-row signal-row--${level} ${link && count > 0 ? "signal-row--link" : ""}`}
      onClick={link && count > 0 ? () => navigate(link) : undefined}
    >
      <span className="signal-label">{label}</span>
      <span className="signal-count">{count.toLocaleString()}</span>
    </div>
  );
}

function CorpusBucket({ data }: { data: CorpusFilesBucket }) {
  return (
    <section className="insight-bucket">
      <h3 className="bucket-title">Corpus Files</h3>
      <div className="bucket-summary">
        <span className="bucket-stat">
          {data.files_indexed.toLocaleString()} indexed
        </span>
        <span className="bucket-stat bucket-stat--muted">
          / {data.files_in_corpus.toLocaleString()} total
        </span>
      </div>
      <div className="signal-list">
        <SignalRow
          label="OOB tag conflicts"
          count={data.oob_tag_conflict}
          level={data.oob_tag_conflict > 0 ? "critical" : "healthy"}
          link="/resolve/oob-conflict"
        />
        <SignalRow
          label="OOB tag sync needed"
          count={data.oob_tag_sync}
          level={severity(data.oob_tag_sync)}
          link="/resolve/oob-db"
        />
        <SignalRow
          label="Mtime-only mismatches"
          count={data.mtime_only_mismatch}
          level={severity(data.mtime_only_mismatch, 10)}
          link="/resolve/oob-mtime"
        />
        <SignalRow
          label="Unindexed files"
          count={data.files_unindexed}
          level={severity(data.files_unindexed)}
        />
        <SignalRow
          label="Missing files"
          count={data.files_missing}
          level={data.files_missing > 0 ? "critical" : "healthy"}
          link="/resolve/missing-files"
        />
        <SignalRow
          label="Missing directories"
          count={data.directories_missing}
          level={data.directories_missing > 0 ? "critical" : "healthy"}
          link="/resolve/missing-directories"
        />
        <SignalRow
          label="Relocated files"
          count={data.files_relocated}
          level={severity(data.files_relocated)}
          link="/resolve/moved-files"
        />
        <SignalRow
          label="Corrupt files"
          count={data.corrupt_files}
          level={data.corrupt_files > 0 ? "critical" : "healthy"}
          link="/resolve/corrupt-files"
        />
        <SignalRow
          label="Lossless remux candidates"
          count={data.lossless_remux_candidates}
          level={severity(data.lossless_remux_candidates, 20)}
          link="/resolve/lossless-remux"
        />
        <SignalRow
          label="Sidecar images"
          count={data.images_in_corpus}
          level="info"
        />
      </div>
      {data.file_type_breakdown.length > 0 && (
        <div className="file-type-breakdown">
          <h4>File Types</h4>
          {data.file_type_breakdown.map(([ft, count]) => (
            <div key={ft} className="breakdown-row">
              <span>{ft}</span>
              <span>{count.toLocaleString()}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function TagSquashBucketView({ data }: { data: TagSquashBucket }) {
  const totalCanonicity = data.tag_canonicity.reduce(
    (s, e) => s + e.cluster_count,
    0,
  );
  const totalCompound = data.compound_tags.reduce(
    (s, e) => s + e.safe_count + e.review_count,
    0,
  );

  return (
    <section className="insight-bucket">
      <h3 className="bucket-title">Tag Squash</h3>
      <div className="signal-list">
        <SignalRow
          label="Cross-source overlaps"
          count={data.cross_source_overlap_count}
          level={severity(data.cross_source_overlap_count)}
          link="/resolve/directory-clusters"
        />
        <SignalRow
          label="Release overlaps"
          count={data.release_overlap_count}
          level={severity(data.release_overlap_count)}
          link="/resolve/release-overlaps"
        />
        <SignalRow
          label="Subpar duplicates"
          count={data.subpar_duplicate_count}
          level={severity(data.subpar_duplicate_count)}
          link="/resolve/subpar-duplicates"
        />
        <SignalRow
          label="Redundant duplicates"
          count={data.redundant_duplicate_count}
          level={severity(data.redundant_duplicate_count)}
          link="/resolve/manual-review/RedundantDuplicate"
        />
        <SignalRow
          label="Tag canonicity clusters"
          count={totalCanonicity}
          level={severity(totalCanonicity)}
          link="/resolve/tag-canonicity-picker"
        />
        <SignalRow
          label="Inconsistent album artists"
          count={data.inconsistent_album_artist_count}
          level={severity(data.inconsistent_album_artist_count)}
          link="/resolve/tag-canonicity/ALBUM_ARTIST"
        />
        <SignalRow
          label="Compound tags"
          count={totalCompound}
          level={severity(totalCompound)}
          link="/resolve/compound-picker"
        />
        <SignalRow
          label="Missing album singles"
          count={data.missing_album_single_count}
          level={severity(data.missing_album_single_count)}
          link="/resolve/missing-album"
        />
        <SignalRow
          label="Disc extraction needed"
          count={data.disc_extraction_count}
          level={severity(data.disc_extraction_count)}
          link="/resolve/disc-extraction"
        />
        <SignalRow
          label="Path/tag mismatches"
          count={data.path_tag_mismatch_count}
          level={severity(data.path_tag_mismatch_count)}
          link="/resolve/manual-review/MetadataDuplicate"
        />
        <SignalRow
          label="Same recording, different release"
          count={data.same_recording_different_release_count}
          level="info"
          link="/resolve/manual-review/SameRecordingDifferentRelease"
        />
        <SignalRow
          label="Artist tags need pluralizing"
          count={data.artist_needs_plural_count}
          level={severity(data.artist_needs_plural_count)}
          link="/resolve/artist-needs-plural"
        />
      </div>
    </section>
  );
}

function OtherBucket({ entries }: { entries: OtherSignalEntry[] }) {
  if (entries.length === 0) return null;

  return (
    <section className="insight-bucket">
      <h3 className="bucket-title">Other Signals</h3>
      <div className="signal-list">
        {entries.map((entry) => (
          <SignalRow
            key={entry.signal_type}
            label={entry.display_label}
            count={entry.count}
            level={severity(entry.count)}
          />
        ))}
      </div>
    </section>
  );
}

export function Health() {
  const { data, isLoading, error } = useInsights();

  if (isLoading) {
    return <div className="view-loading">Loading insights...</div>;
  }

  if (error) {
    return (
      <div className="view-error">
        Failed to load insights: {error.message}
      </div>
    );
  }

  if (!data) return null;

  return (
    <div className="health-view">
      <CorpusBucket data={data.bucket_corpus} />
      <TagSquashBucketView data={data.bucket_placeholder} />
      <OtherBucket entries={data.bucket_other.entries} />
    </div>
  );
}
