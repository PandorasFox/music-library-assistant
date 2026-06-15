// Hand-written TypeScript types matching serde JSON serialization of mm-meta Rust types.
// These will be replaced by ts-rs codegen output once the pipeline is wired up.

// -- Fieldless enums (serde serializes as strings) --

export type WitchStartupState =
  | "AwaitingSetup"
  | "Reconciling"
  | "Vacuuming"
  | "Ready";

export type ReasoningLevel = "None" | "Inodes" | "Full";

export type WatcherState = "NotStarted" | "InitialScan" | "Watching" | "Polling";

export type WorkStateSnapshot = "Idle" | "Working" | "Done";

export type DecisionKeyKind =
  | "OobResolution"
  | "MovedFile"
  | "MissingFile"
  | "MissingDirectory"
  | "CorruptFile"
  | "LosslessRemux"
  | "SubparDuplicate";

export type ConflictBucket = "MtimeOnly" | "DbOnly" | "DiskOnly" | "Conflict";

// -- Structs (serde serializes as objects) --

export interface WorkStatus {
  state: WorkStateSnapshot;
  pending: number;
  total_processed: number;
  session_queued: number;
  pending_by_label: Record<string, number>;
}

export interface SourceProgress {
  total: number;
  processed: number;
  matched: number;
  no_match: number;
  retries: number;
}

export interface FetchProgress {
  acoustid: SourceProgress;
  mb: SourceProgress;
  discogs: SourceProgress;
  acoustid_rps: number;
  mb_rps: number;
  discogs_rps: number;
  discogs_enabled: boolean;
}

export interface CoverArtProgress {
  total_releases: number;
  processed: number;
  images_written: number;
  images_skipped: number;
  images_upgraded: number;
}

export interface DeezerProgress {
  total_dirs: number;
  processed: number;
  images_written: number;
  isrc_not_found: number;
  errors: number;
}

export interface TransactionSnapshot {
  label: string;
  decision_count: number;
  mutation_count: number;
  decision_keys: DecisionKey[];
  decision_labels: [DecisionKey, string][];
}

export interface WitchStatus {
  startup_state: WitchStartupState;
  work: WorkStatus;
  reasoning_level: ReasoningLevel;
  has_pending: boolean;
  is_initial_scanning: boolean;
  db_queue_depth: number;
  transaction: TransactionSnapshot | null;
  handled_decision_kinds: DecisionKeyKind[];
  is_external_fetch_active: boolean;
  external_fetch_progress: FetchProgress | null;
  has_acoustid_api_key: boolean;
  is_cover_art_fetch_active: boolean;
  cover_art_progress: CoverArtProgress | null;
  is_deezer_fetch_active: boolean;
  deezer_progress: DeezerProgress | null;
  mutations_generation: number;
  computations_generation: number;
  last_error: string | null;
  error_generation: number;
  config_generation: number;
}

// -- Externally-tagged enums (serde default: unit variants as strings, struct variants as objects) --

export type WitchEvent = { StatusChanged: WitchStatus };

export type DecisionKey =
  | { TagCanonicity: { tag_name: string; cluster_index: number } }
  | { CompoundSplitSafe: { tag_name: string; cluster_index: number } }
  | { CompoundSplitReview: { tag_name: string; cluster_index: number } }
  | "Deploy"
  | "DeploySidecars"
  | { TagEdit: { key_item: string } }
  | { OobResolution: { bucket: ConflictBucket } }
  | "MovedFile"
  | "MissingFile"
  | "MissingDirectory"
  | "CorruptFile"
  | "LosslessRemux"
  | "SubparDuplicate"
  | { DirectoryCluster: { cluster_index: number } }
  | { MissingAlbum: { group_index: number } }
  | { ManualReview: { group_index: number } }
  | { DiscExtraction: { group_index: number } }
  | { EditReversal: { session_label: string } }
  | "ConfigEdit"
  | { DirConfigEdit: { source_path: string } }
  | { MbReleaseApproval: { release_id: string } }
  | { VaOverrideApplication: { release_id: string } };

// -- Insights view types --

export interface DirectoryBreakdownEntry {
  _directory: string;
  _count: number;
}

export interface DirectoryBreakdown {
  _entries: DirectoryBreakdownEntry[];
}

export interface CorpusFilesBucket {
  oob_tag_sync: number;
  oob_tag_conflict: number;
  mtime_only_mismatch: number;
  files_in_corpus: number;
  files_indexed: number;
  files_unindexed: number;
  files_missing: number;
  directories_missing: number;
  files_relocated: number;
  corrupt_files: number;
  lossless_remux_candidates: number;
  images_in_corpus: number;
  file_type_breakdown: [string, number][];
  _directory_breakdown: DirectoryBreakdown;
}

export interface TagSquashEntry {
  tag_name: string;
  cluster_count: number;
  _total_tracks: number;
}

export interface CompoundTagEntry {
  tag_name: string;
  safe_count: number;
  review_count: number;
}

export interface TagSquashBucket {
  cross_source_overlap_count: number;
  release_overlap_count: number;
  subpar_duplicate_count: number;
  redundant_duplicate_count: number;
  tag_canonicity: TagSquashEntry[];
  inconsistent_album_artist_count: number;
  compound_tags: CompoundTagEntry[];
  missing_album_single_count: number;
  disc_extraction_count: number;
  path_tag_mismatch_count: number;
  same_recording_different_release_count: number;
  artist_needs_plural_count: number;
}

export interface OtherSignalEntry {
  signal_type: string;
  display_label: string;
  count: number;
  affected_count: number | null;
}

export interface OtherSignalsBucket {
  entries: OtherSignalEntry[];
}

export interface InsightsData {
  bucket_corpus: CorpusFilesBucket;
  bucket_placeholder: TagSquashBucket;
  bucket_other: OtherSignalsBucket;
}

// -- Directory listing --

export interface DirectoryListingEntry {
  name: string;
  path: string;
  is_dir: boolean;
  file_count: number;
  inode: number | null;
  duration_ms: number | null;
  bitrate_kbps: number | null;
}

// -- Search --

export interface SearchResult {
  inode: number;
  path: string;
  artist: string | null;
  album: string | null;
  title: string | null;
}

// -- External matches --

export interface ExternalMatchReviewEntry {
  path: string;
  confidence: number;
  recording_id: string;
}

export interface ConfidenceBucket {
  tier: string;
  total: number;
  entries: ExternalMatchReviewEntry[];
}

export interface ExternalMatchesData {
  untagged_entries: ExternalMatchReviewEntry[];
  confidence_buckets: ConfidenceBucket[];
  packing_perfect_count: number;
  packing_full_match_count: number;
  packing_singles_count: number;
  packing_incomplete_count: number;
  packing_low_confidence_count: number;
  packing_knots_count: number;
  unsolved_conflict_count: number;
  unsolved_no_release_count: number;
  unsolved_no_match_count: number;
  va_override_count: number;
  pinned_conflict_count: number;
  pinned_releases_stale: boolean;
}

export interface ReviewableTrack {
  position: number;
  medium_position: number;
  mb_title: string;
  mb_artist: string;
  recording_id: string;
  matched_inode: number | null;
  matched_display_name: string | null;
  confidence: number | null;
}

export interface ReviewableRelease {
  release_id: string;
  title: string;
  artist: string;
  track_count: number;
  matched_count: number;
  avg_confidence: number;
  category: string;
  tracks: ReviewableTrack[];
}

export interface ReleaseReviewData {
  releases: ReviewableRelease[];
}

export interface AcoustidMatchEntry {
  inode: number;
  display_name: string;
  confidence: number;
  recording_id: string;
  recording_title: string | null;
  recording_artist: string | null;
  recording_length_ms: number | null;
}

// -- Deploy --

export interface DeploySignalFile {
  library_name: string;
  corpus_path: string;
  deploy_path: string;
}

export interface StaleSignalFile {
  library_name: string;
  library_path: string;
  expected_path: string;
}

export interface LeftoverSignalFile {
  library_name: string;
  library_path: string;
}

export interface ConflictGroup {
  deploy_path: string;
  conflicting_files: [string, number][];
}

export interface SidecarConflictGroup {
  deploy_path: string;
  library_name: string;
  conflicting_files: [string, number][];
}

export interface SidecarDeployEntry {
  corpus_image_path: string;
  library_name: string;
  library_album_dir: string;
  filename: string;
  format: string;
  width: number;
  height: number;
  role: string;
}

export interface DirectoryAggregate {
  directory: string;
  count: number;
  sidecar_count: number;
}

export interface LibrarySummary {
  library_name: string;
  healthy_count: number;
  new_count: number;
  leftover_count: number;
  stale_count: number;
  replaced_count: number;
}

export interface DeployModalData {
  healthy: DeploySignalFile[];
  new: DeploySignalFile[];
  new_by_dir: DirectoryAggregate[];
  conflicts: ConflictGroup[];
  leftover: LeftoverSignalFile[];
  leftover_by_dir: DirectoryAggregate[];
  stale: StaleSignalFile[];
  per_library: LibrarySummary[];
  replaced_count: number;
  sidecars: SidecarDeployEntry[];
  sidecar_conflicts: SidecarConflictGroup[];
}

export interface DeployStatus {
  needs_action: boolean;
  library_file_counts: [string, number][];
}

// -- Health resolution modal data --

export interface RestorableMissingFile {
  corpus_path: string;
  library_path: string;
  inode: number;
}

export interface NonRestorableMissingFile {
  corpus_path: string;
  inode: number | null;
}

export interface MissingFileModalData {
  restorable: RestorableMissingFile[];
  non_restorable: NonRestorableMissingFile[];
}

export interface MissingDirectoryModalData {
  directories: string[];
}

export interface CorruptFileEntry {
  corpus_path: string;
  inode: number;
}

export interface CorruptFileModalData {
  files: CorruptFileEntry[];
}

export interface ArtistNeedsPluralData {
  needs_artist: boolean;
  needs_album_artist: boolean;
  artist_values: string[];
  album_artist_values: string[];
}

export interface ArtistNeedsPluralEntry {
  inode: number;
  corpus_path: string;
  data: ArtistNeedsPluralData;
}

export interface ArtistNeedsPluralModalData {
  files: ArtistNeedsPluralEntry[];
}

export interface SubparFileEntry {
  corpus_path: string;
  inode: number;
  reason: string;
  superior_path: string;
  similarity_score: number;
}

export interface SubparDuplicateModalData {
  files: SubparFileEntry[];
}

export interface RemuxCandidateEntry {
  corpus_path: string;
  inode: number;
  file_type: string;
}

export interface LosslessRemuxModalData {
  files: RemuxCandidateEntry[];
  file_counts: Record<string, number>;
}

export interface MovedFileInfo {
  inode: number;
  old_path: string;
  new_path: string;
  old_zone: string;
  new_zone: string;
}

export interface TagMismatchEntry {
  field: string;
  db_value: string | null;
  disk_value: string | null;
}

export interface OobFile {
  inode: number;
  path: string;
  bucket: ConflictBucket;
  mismatches: TagMismatchEntry[];
}

// -- Directory cluster resolution --

export interface FileMetaSummary {
  file_type: string;
  duration_ms: number | null;
  bitrate_kbps: number | null;
  sample_rate: number | null;
  file_size: number;
  has_pictures: boolean;
  tags: [string, string][];
}

export interface DirectoryGroupEntry {
  path_suffix: string;
  inodes: number[];
  paths: string[];
  format_summary: string;
  can_stash_dupes: boolean;
}

export interface DirectoryClusterEntry {
  cluster_key: string;
  directories: DirectoryGroupEntry[];
  overlap_count: number;
}

export interface DirectoryClusterModalData {
  clusters: DirectoryClusterEntry[];
  file_meta_cache: Record<string, FileMetaSummary>;
}

// -- Packing knots --

export interface KnotAssignment {
  inode: number;
  recording_id: string;
  medium_pos: number;
  track_pos: number;
  track_title: string;
  score: number;
  match_method: number;
}

export interface KnotProposalEntry {
  release_id: string;
  release_title: string;
  release_artist: string;
  total_tracks: number;
  total_score: number;
  selected: boolean;
  assignments: KnotAssignment[];
}

export interface PackingKnotData {
  tier: string;
  knot_id: number;
  classification: string;
  ratio: number;
  contested_inodes: number[];
  proposals: KnotProposalEntry[];
}

// -- VA Override review types --

export type VariousArtistsOverrideSource =
  | "ExactAlternative"
  | "CompetingProposal";

export interface VaOverrideReviewRow {
  release_id: string;
  release_title: string;
  suggested_artist: string;
  source: VariousArtistsOverrideSource;
  packed_inode_count: number;
  current_albumartist: string | null;
  albumartist_uniform: boolean;
}

export interface VaOverrideReview {
  rows: VaOverrideReviewRow[];
}

export interface VaOverrideApplication {
  release_id: string;
  albumartist: string;
}

export interface VaOverrideStageSummary {
  staged_releases: number;
  staged_inodes: number;
  skipped_releases: number;
  already_matching_inodes: number;
}

// -- Genre vocabulary (Phase 2 vocabulary editor) --

export interface GenreNameEntry {
  id: number;
  canonical_name: string;
  display_name: string;
  aliases: string[];
  implies_parents: number[];
  ledger_row_count: number;
}

export interface GenreVocabulary {
  entries: GenreNameEntry[];
}

export interface UnresolvedGenreRow {
  raw_value: string;
  /** GenreSource discriminant (1=FileTagImport, 2=MusicBrainz, 3=Discogs, 4=AudiomuseInferred, 5=Manual, 6=Implied) */
  source: number;
  observation_count: number;
  last_seen_at: number;
}

export interface UnresolvedGenreObservations {
  rows: UnresolvedGenreRow[];
}

/** Mirror of `mm_meta::mutations::genre_vocabulary::GenreVocabularyOp`.
 *  Serde serializes enum-with-data variants as `{ Variant: { ... } }`. */
export type GenreVocabularyOp =
  | { AddName: { canonical_name: string; display_name: string } }
  | { AddAlias: { alias: string; genre_id: number } }
  | { RemoveAlias: { alias: string } }
  | { AddImplication: { child_id: number; parent_id: number } }
  | { RemoveImplication: { child_id: number; parent_id: number } }
  | { MergeGenres: { from_id: number; into_id: number } };

// -- Phase 6: genre promotion --

/** One chip in the promotion review.
 *  `kind`: 0 = Genre, 1 = Style.
 *  `sources_bits`: packed bitset over GenreSource discriminants (bit n-1 set
 *  when source enum value n contributed). 1=FileTagImport, 2=MusicBrainz,
 *  3=Discogs, 4=AudiomuseInferred, 5=Manual. */
export interface GenrePromotionChip {
  genre_id: number;
  canonical_name: string;
  kind: number;
  inode_count: number;
  sources_bits: number;
}

export interface GenrePromotionReviewRow {
  release_id: string;
  release_title: string;
  release_artist: string;
  packed_inode_count: number;
  proposed_chips: GenrePromotionChip[];
  current_genre_summary: string | null;
  current_genre_uniform: boolean;
  sources_bits: number;
}

export interface GenrePromotionReview {
  rows: GenrePromotionReviewRow[];
}

export interface GenrePromotionInodeRow {
  inode: number;
  path: string;
  chips: GenrePromotionChip[];
}

export interface GenrePromotionInodeDetail {
  release_id: string;
  rows: GenrePromotionInodeRow[];
}

export interface GenreCoverageSummary {
  total_audio_inodes: number;
  inodes_with_any_ledger: number;
  inodes_with_discogs_ledger: number;
  inodes_with_filetag_ledger: number;
  inodes_already_promoted: number;
  pending_promotion: number;
  unresolved_observations: number;
}

/** Wire shape for `POST /tx/promote-genres`. */
export interface GenrePromotionApplication {
  release_id: string;
  inodes: number[];
  /** Pairs of (genre_id, kind) the operator toggled OFF in the chip strip. */
  excluded: [number, number][];
}

export interface GenrePromotionStagingSummary {
  staged_releases: number;
  staged_inodes: number;
  already_matching_inodes: number;
  inodes_without_ledger: number;
  skipped_empty_applications: number;
  skipped_unpacked_inodes: number;
}
