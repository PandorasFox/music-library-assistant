// Static field definitions for the config editor.
// Mirrors crates/mm-ui/src/config_editor/build.rs.

export type FieldType =
  | "bool"
  | "float"
  | "uint"
  | "int"
  | "optional-uint"
  | "string"
  | "string-list"
  | "string-set"
  | "duration"
  | "enum"
  | "string-list-map"
  | "bool-grid";

export interface FieldDef {
  label: string;
  description: string;
  path: string; // dot-separated path into config JSON (under "opinions")
  type: FieldType;
  defaultValue: unknown;
  options?: string[]; // enum options
  boolGridColumns?: string[];
  restartRequired?: boolean;
}

export interface GroupDef {
  name: string;
  collapsed: boolean;
  fields: FieldDef[];
}

export const CONFIG_GROUPS: GroupDef[] = [
  {
    name: "Startup",
    collapsed: false,
    fields: [
      {
        label: "Force check all files at startup",
        description: "Bypass mtime optimization, verify all indexed files",
        path: "startup.force_check_all_files_at_startup",
        type: "bool",
        defaultValue: false,
      },
      {
        label: "Vacuum threshold",
        description:
          "Free-page ratio threshold for DB compaction prompt (0.0 disables)",
        path: "startup.vacuum_threshold",
        type: "float",
        defaultValue: 0.1,
      },
      {
        label: "Default view",
        description: "View to open after startup progress completes",
        path: "startup.default_view",
        type: "enum",
        options: ["Health", "Search", "Browser", "ExtAuthorities"],
        defaultValue: "Health",
      },
    ],
  },
  {
    name: "Health Detection",
    collapsed: false,
    fields: [
      {
        label: "Required tags",
        description: "Tags that must be present on every track",
        path: "health_detection.required_tags",
        type: "string-list",
        defaultValue: ["title", "album", "artist", "album_artist"],
      },
      {
        label: "Album artist only if compilation",
        description: "Only require album_artist on multi-artist albums",
        path: "health_detection.album_artist_only_required_if_compilation",
        type: "bool",
        defaultValue: true,
      },
      {
        label: "Single album suffix",
        description: "Suffix appended when tagging as single",
        path: "health_detection.single_album_suffix",
        type: "string",
        defaultValue: "",
      },
    ],
  },
  {
    name: "Canonicalization",
    collapsed: false,
    fields: [
      {
        label: "Strip album format suffixes",
        description: "Normalize EP/LP suffixes during album collision detection",
        path: "canonicalization.strip_album_format_suffixes",
        type: "bool",
        defaultValue: false,
      },
    ],
  },
  {
    name: "Duplicate Analysis",
    collapsed: false,
    fields: [
      {
        label: "Fingerprint similarity threshold",
        description: "Pairs below this similarity (0-100) are not duplicates",
        path: "duplicate_analysis.fingerprint_similarity_threshold",
        type: "float",
        defaultValue: 95.0,
      },
      {
        label: "Duration tolerance (ms)",
        description:
          "Tracks with duration diff above this are clustered separately",
        path: "duplicate_analysis.duration_tolerance_ms",
        type: "int",
        defaultValue: 2000,
      },
      {
        label: "Elide variant titles",
        description:
          "Skip dupe pairs where titles differ and contain remix/live/etc.",
        path: "duplicate_analysis.elide_variant_titles",
        type: "bool",
        defaultValue: true,
      },
    ],
  },
  {
    name: "Tag Splitting",
    collapsed: false,
    fields: [
      {
        label: "Collaboration keywords",
        description: "Keywords like feat, ft, vs for artist collabs",
        path: "tag_splitting.collaboration_keywords",
        type: "string-set",
        defaultValue: ["feat", "featuring", "ft", "with", "vs"],
      },
      {
        label: "Tag separators",
        description: "Per-tag separator strings",
        path: "tag_splitting.tag_separators",
        type: "string-list-map",
        defaultValue: { ARTIST: [";"], GENRE: [";"] },
      },
    ],
  },
  {
    name: "Release Packing",
    collapsed: false,
    fields: [
      {
        label: "Duration tolerance %",
        description:
          "Discard recording matches with duration diff above this fraction (0.0-1.0)",
        path: "release_packing.duration_tolerance_pct",
        type: "float",
        defaultValue: 0.15,
      },
      {
        label: "Min AcoustID confidence",
        description:
          "Discard recording matches below this confidence (0.0-1.0)",
        path: "release_packing.min_confidence",
        type: "float",
        defaultValue: 0.3,
      },
      {
        label: "Title pre-assign threshold",
        description:
          "Title similarity threshold for elimination pre-assignment (0.0-1.0)",
        path: "release_packing.title_preassign_threshold",
        type: "float",
        defaultValue: 0.95,
      },
      {
        label: "Packing knot ratio",
        description:
          "Proposals/inodes ratio threshold for knot extraction (0 to disable)",
        path: "release_packing.packing_knot_ratio",
        type: "float",
        defaultValue: 3.0,
      },
      {
        label: "Packing knot size limit",
        description: "Max component size before knot extraction (0 to disable)",
        path: "release_packing.packing_knot_size_limit",
        type: "uint",
        defaultValue: 50,
      },
      {
        label: "Singles before incompletes",
        description: "Run single-track MIS round before incompletes",
        path: "release_packing.singles_before_incompletes",
        type: "bool",
        defaultValue: true,
      },
      {
        label: "Resolve knots with discographies",
        description:
          "Reduce knots to covering proposals (discography releases) when possible",
        path: "release_packing.allow_resolve_knots_with_discographies",
        type: "bool",
        defaultValue: true,
      },
      {
        label: "Low confidence max AcoustID ratio",
        description:
          "Max AcoustID-matched fraction to trigger low-confidence downgrade (0.0-1.0)",
        path: "release_packing.low_confidence_max_acoustid_ratio",
        type: "float",
        defaultValue: 0.25,
      },
      {
        label: "Low confidence max album match",
        description:
          "Max avg album_match score to trigger low-confidence downgrade (0.0-1.0)",
        path: "release_packing.low_confidence_max_album_match",
        type: "float",
        defaultValue: 0.3,
      },
    ],
  },
  {
    name: "Packing: Candidate Weights",
    collapsed: true,
    fields: [
      {
        label: "AcoustID confidence",
        description: "",
        path: "release_packing.candidate_weights.acoustid_confidence",
        type: "float",
        defaultValue: 0.3,
      },
      {
        label: "Duration match",
        description: "",
        path: "release_packing.candidate_weights.duration_match",
        type: "float",
        defaultValue: 0.3,
      },
      {
        label: "Title match",
        description: "",
        path: "release_packing.candidate_weights.title_match",
        type: "float",
        defaultValue: 0.1,
      },
      {
        label: "Artist match",
        description: "",
        path: "release_packing.candidate_weights.artist_match",
        type: "float",
        defaultValue: 0.05,
      },
      {
        label: "Album match",
        description: "",
        path: "release_packing.candidate_weights.album_match",
        type: "float",
        defaultValue: 0.05,
      },
      {
        label: "Track number match",
        description: "",
        path: "release_packing.candidate_weights.track_number_match",
        type: "float",
        defaultValue: 0.2,
      },
    ],
  },
  {
    name: "Packing: Elimination Weights",
    collapsed: true,
    fields: [
      {
        label: "AcoustID confidence",
        description: "",
        path: "release_packing.elimination_weights.acoustid_confidence",
        type: "float",
        defaultValue: 0.0,
      },
      {
        label: "Duration match",
        description: "",
        path: "release_packing.elimination_weights.duration_match",
        type: "float",
        defaultValue: 0.25,
      },
      {
        label: "Title match",
        description: "",
        path: "release_packing.elimination_weights.title_match",
        type: "float",
        defaultValue: 0.3,
      },
      {
        label: "Artist match",
        description: "",
        path: "release_packing.elimination_weights.artist_match",
        type: "float",
        defaultValue: 0.0,
      },
      {
        label: "Album match",
        description: "",
        path: "release_packing.elimination_weights.album_match",
        type: "float",
        defaultValue: 0.05,
      },
      {
        label: "Track number match",
        description: "",
        path: "release_packing.elimination_weights.track_number_match",
        type: "float",
        defaultValue: 0.4,
      },
    ],
  },
  {
    name: "External Matching",
    collapsed: false,
    fields: [
      {
        label: "AcoustID API key",
        description:
          "API key for AcoustID fingerprint lookups (empty = disabled)",
        path: "external_matching.acoustid_api_key",
        type: "string",
        defaultValue: "",
      },
      {
        label: "Requests per second",
        description: "Rate limit for AcoustID API calls",
        path: "external_matching.requests_per_second",
        type: "uint",
        defaultValue: 3,
      },
      {
        label: "Auto-enrich on match",
        description:
          "Auto-trigger MB enrichment when AcoustID matches arrive",
        path: "external_matching.auto_enrich_on_match",
        type: "bool",
        defaultValue: true,
      },
      {
        label: "MB cache TTL (days)",
        description: "Days before re-fetching MusicBrainz cache entries",
        path: "external_matching.mb_cache_ttl_days",
        type: "uint",
        defaultValue: 30,
      },
      {
        label: "MB requests per second",
        description:
          "Rate limit ceiling for MusicBrainz API (adaptive backoff)",
        path: "external_matching.mb_requests_per_second",
        type: "uint",
        defaultValue: 25,
      },
      {
        label: "MB base URL",
        description:
          "MusicBrainz API base URL (use a local mirror to bypass rate limits)",
        path: "external_matching.mb_base_url",
        type: "string",
        defaultValue: "https://musicbrainz.org/ws/2",
      },
      {
        label: "Preferred locales",
        description:
          "Locale preference for artist name resolution (e.g. en, ja)",
        path: "external_matching.preferred_locales",
        type: "string-list",
        defaultValue: [],
      },
      {
        label: "Cover art types",
        description: 'CAA image types to fetch (e.g. Front, Back)',
        path: "external_matching.cover_art_types",
        type: "string-list",
        defaultValue: ["Front", "Back"],
      },
    ],
  },
  {
    name: "MB Tag Names",
    collapsed: true,
    fields: [
      {
        label: "Recording tag",
        description: "Vorbis Comment tag name for recording MBID",
        path: "external_matching.mb_tag_names.recording",
        type: "string",
        defaultValue: "MUSICBRAINZ_RECORDING",
      },
      {
        label: "Release tag",
        description: "Vorbis Comment tag name for release MBID",
        path: "external_matching.mb_tag_names.release",
        type: "string",
        defaultValue: "MUSICBRAINZ_RELEASE",
      },
      {
        label: "Track tag",
        description: "Vorbis Comment tag name for track-on-release MBID",
        path: "external_matching.mb_tag_names.track",
        type: "string",
        defaultValue: "MUSICBRAINZ_TRACK",
      },
      {
        label: "Picard-compat aliases",
        description:
          "Also write Picard-style aliases for Navidrome",
        path: "external_matching.mb_tag_names.picard_compat",
        type: "bool",
        defaultValue: false,
      },
    ],
  },
  {
    name: "Credit Routing",
    collapsed: true,
    fields: [
      {
        label: "Feat format",
        description:
          "Template for vocalist title suffix ({artists} is replaced)",
        path: "external_matching.credit_routing.feat_format",
        type: "string",
        defaultValue: "feat. {artists}",
      },
      {
        label: "Max feat credits",
        description:
          "Cap on artist names in feat suffix (auto = unlimited)",
        path: "external_matching.credit_routing.max_feat_credits",
        type: "optional-uint",
        defaultValue: null,
      },
      {
        label: "Relation routing",
        description: "Route recording credits to artist/title/composer tags",
        path: "external_matching.credit_routing.routing",
        type: "bool-grid",
        boolGridColumns: ["artist", "title", "composer"],
        defaultValue: {
          performer: { to_artist: true, to_title: false, to_composer: false },
          vocal: { to_artist: true, to_title: true, to_composer: false },
          instrument: { to_artist: true, to_title: false, to_composer: false },
          remixer: { to_artist: false, to_title: true, to_composer: false },
        },
      },
    ],
  },
  {
    name: "Disc Extraction",
    collapsed: false,
    fields: [
      {
        label: "Disc tag name",
        description: "Tag name to write extracted disc identifier into",
        path: "disc_extraction.disc_tag_name",
        type: "string",
        defaultValue: "DISCNUMBER",
      },
      {
        label: "Map letters to numbers",
        description: "Map letter prefixes to numbers (A\u21921, B\u21922, ...)",
        path: "disc_extraction.map_letters_to_numbers",
        type: "bool",
        defaultValue: false,
      },
    ],
  },
  {
    name: "Album Art",
    collapsed: false,
    fields: [
      {
        label: "Sidecar deploy mode",
        description: "Deploy sidecar cover images alongside audio files",
        path: "album_art.sidecar_deploy_mode",
        type: "enum",
        options: ["Disabled", "PrimaryCover", "All"],
        defaultValue: "PrimaryCover",
      },
    ],
  },
  {
    name: "Performance",
    collapsed: false,
    fields: [
      {
        label: "Worker threads",
        description: "Number of worker threads (auto = 2x logical cores)",
        path: "performance.worker_threads",
        type: "optional-uint",
        defaultValue: null,
        restartRequired: true,
      },
      {
        label: "DB cache (MB)",
        description: "SQLite page cache size per connection in MB",
        path: "performance.db_cache_mb",
        type: "uint",
        defaultValue: 256,
        restartRequired: true,
      },
    ],
  },
  {
    name: "Advanced",
    collapsed: false,
    fields: [
      {
        label: "Leave transactions open",
        description:
          "Keep one open transaction; adds Transaction tab to view ring",
        path: "leave_transactions_open",
        type: "bool",
        defaultValue: false,
      },
      {
        label: "Watcher poll interval",
        description:
          "Filesystem poll interval when inotify is unavailable",
        path: "watcher_poll_interval_secs",
        type: "duration",
        defaultValue: 900,
      },
      {
        label: "Session lifetime (days)",
        description: "Session expiry in days (auto = 30)",
        path: "session_lifetime_days",
        type: "optional-uint",
        defaultValue: 30,
      },
    ],
  },
];
