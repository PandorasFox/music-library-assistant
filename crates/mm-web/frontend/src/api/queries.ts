import { useQuery } from "@tanstack/react-query";
import { get, post } from "./client";
import type {
  InsightsData,
  WitchStatus,
  DirectoryListingEntry,
  SearchResult,
  ExternalMatchesData,
  ReleaseReviewData,
  AcoustidMatchEntry,
  DeployStatus,
  DeployModalData,
  MissingFileModalData,
  MissingDirectoryModalData,
  CorruptFileModalData,
  ArtistNeedsPluralModalData,
  SubparDuplicateModalData,
  LosslessRemuxModalData,
  MovedFileInfo,
  OobFile,
  PackingKnotData,
  DirectoryClusterModalData,
  VaOverrideReview,
  GenreVocabulary,
  UnresolvedGenreObservations,
  GenrePromotionReview,
  GenrePromotionInodeDetail,
  GenreCoverageSummary,
} from "./generated/types";

// -- Query keys --

export const queryKeys = {
  status: ["status"] as const,
  insights: ["insights"] as const,
  directoryListing: (parent: string | null) =>
    ["directory-listing", parent] as const,
  searchCorpus: (query: string) => ["search-corpus", query] as const,
  config: ["config"] as const,
  configKdl: ["config-kdl"] as const,
  dirConfig: (path: string) => ["dir-config", path] as const,
  txDetails: ["tx-details"] as const,
  externalMatches: ["external-matches"] as const,
  acoustidMatches: (confidence: string) =>
    ["acoustid-matches", confidence] as const,
  releaseReview: (filter: string) => ["release-review", filter] as const,
  deployStatus: ["deploy-status"] as const,
  deployData: ["deploy-data"] as const,
  packingKnots: ["packing-knots"] as const,
  missingFiles: ["missing-file-data"] as const,
  missingDirs: ["missing-directory-data"] as const,
  corruptFiles: ["corrupt-file-data"] as const,
  artistNeedsPlural: ["artist-needs-plural-data"] as const,
  subparDupes: ["subpar-duplicate-data"] as const,
  losslessRemux: ["lossless-remux-data"] as const,
  movedFiles: ["moved-files"] as const,
  oobFiles: (bucket: string) => ["oob-files", bucket] as const,
  directoryClusters: ["directory-cluster-data"] as const,
  releaseOverlaps: ["release-overlap-data"] as const,
  vaOverrideReview: ["va-override-review"] as const,
  genreVocabulary: ["genre-vocabulary"] as const,
  unresolvedGenres: ["unresolved-genre-observations"] as const,
  genrePromotionReview: ["genre-promotion-review"] as const,
  genrePromotionInodeDetail: (release_id: string) =>
    ["genre-promotion-inode-detail", release_id] as const,
  genreCoverageSummary: ["genre-coverage-summary"] as const,
} as const;

// -- Hooks --

export function useStatus() {
  return useQuery({
    queryKey: queryKeys.status,
    queryFn: () => get<WitchStatus>("/status"),
    refetchInterval: false,
  });
}

export function useInsights() {
  return useQuery({
    queryKey: queryKeys.insights,
    queryFn: () => get<InsightsData>("/queries/insights"),
    refetchInterval: false,
  });
}

export function useDirectoryListing(parent: string | null) {
  const params = new URLSearchParams({ zone: "Corpus" });
  if (parent) params.set("parent", parent);

  return useQuery({
    queryKey: queryKeys.directoryListing(parent),
    queryFn: () =>
      get<DirectoryListingEntry[]>(`/queries/directory-listing?${params}`),
  });
}

export function useSearchCorpus(query: string, limit: number = 200) {
  return useQuery({
    queryKey: queryKeys.searchCorpus(query),
    queryFn: () =>
      get<SearchResult[]>(
        `/queries/search-corpus?query=${encodeURIComponent(query)}&limit=${limit}`,
      ),
    enabled: query.length > 0,
  });
}

export function useConfig() {
  return useQuery({
    queryKey: queryKeys.config,
    queryFn: () => get<Record<string, unknown>>("/config"),
  });
}

export function useConfigKdl() {
  return useQuery({
    queryKey: queryKeys.configKdl,
    queryFn: () => get<string>("/config-kdl"),
  });
}

export function usePackingKnots() {
  return useQuery({
    queryKey: queryKeys.packingKnots,
    queryFn: () => get<PackingKnotData[]>("/queries/packing-knots"),
  });
}

export function useMissingFileData() {
  return useQuery({
    queryKey: queryKeys.missingFiles,
    queryFn: () => get<MissingFileModalData>("/queries/missing-file-data"),
  });
}

export function useMissingDirectoryData() {
  return useQuery({
    queryKey: queryKeys.missingDirs,
    queryFn: () => get<MissingDirectoryModalData>("/queries/missing-directory-data"),
  });
}

export function useCorruptFileData() {
  return useQuery({
    queryKey: queryKeys.corruptFiles,
    queryFn: () => get<CorruptFileModalData>("/queries/corrupt-file-data"),
  });
}

export function useArtistNeedsPluralData() {
  return useQuery({
    queryKey: queryKeys.artistNeedsPlural,
    queryFn: () => get<ArtistNeedsPluralModalData>("/queries/artist-needs-plural-data"),
  });
}

export function useSubparDuplicateData() {
  return useQuery({
    queryKey: queryKeys.subparDupes,
    queryFn: () => get<SubparDuplicateModalData>("/queries/subpar-duplicate-data"),
  });
}

export function useLosslessRemuxData() {
  return useQuery({
    queryKey: queryKeys.losslessRemux,
    queryFn: () => get<LosslessRemuxModalData>("/queries/lossless-remux-data"),
  });
}

export function useMovedFiles() {
  return useQuery({
    queryKey: queryKeys.movedFiles,
    queryFn: () => get<MovedFileInfo[]>("/queries/moved-files"),
  });
}

export function useOobFiles(bucket: string) {
  return useQuery({
    queryKey: queryKeys.oobFiles(bucket),
    queryFn: () => get<OobFile[]>(`/queries/oob-files?bucket=${bucket}`),
  });
}

export function useDirectoryClusterData() {
  return useQuery({
    queryKey: queryKeys.directoryClusters,
    queryFn: () => get<DirectoryClusterModalData>("/queries/directory-cluster-data"),
  });
}

export function useReleaseOverlapData() {
  return useQuery({
    queryKey: queryKeys.releaseOverlaps,
    queryFn: () => get<DirectoryClusterModalData>("/queries/release-overlap-data"),
  });
}

export function useDirConfig(path: string | null) {
  return useQuery({
    queryKey: queryKeys.dirConfig(path ?? ""),
    queryFn: () =>
      get<SourceDir | null>(
        `/dir-config?path=${encodeURIComponent(path ?? "")}`,
      ),
    enabled: path !== null,
  });
}

export function useTxDetails() {
  return useQuery({
    queryKey: queryKeys.txDetails,
    queryFn: () => get<DecisionDetail[]>("/tx/details"),
  });
}

export function useExternalMatches() {
  return useQuery({
    queryKey: queryKeys.externalMatches,
    queryFn: () => get<ExternalMatchesData>("/queries/external-matches"),
    refetchInterval: false,
  });
}

export function useAcoustidMatches(confidence: string) {
  return useQuery({
    queryKey: queryKeys.acoustidMatches(confidence),
    queryFn: () =>
      get<AcoustidMatchEntry[]>(
        `/queries/acoustid-matches?confidence=${confidence}`,
      ),
  });
}

export function useReleaseReview(filter: string) {
  return useQuery({
    queryKey: queryKeys.releaseReview(filter),
    queryFn: () =>
      get<ReleaseReviewData>(
        `/queries/release-review?filter=${filter}`,
      ),
  });
}

export function useVaOverrideReview() {
  return useQuery({
    queryKey: queryKeys.vaOverrideReview,
    queryFn: () =>
      get<VaOverrideReview>(`/queries/va-override-review`),
  });
}

export function useGenreVocabulary() {
  return useQuery({
    queryKey: queryKeys.genreVocabulary,
    queryFn: () => get<GenreVocabulary>(`/queries/genre-vocabulary`),
  });
}

export function useUnresolvedGenreObservations() {
  return useQuery({
    queryKey: queryKeys.unresolvedGenres,
    queryFn: () =>
      get<UnresolvedGenreObservations>(`/queries/unresolved-genre-observations`),
  });
}

export function useGenrePromotionReview() {
  return useQuery({
    queryKey: queryKeys.genrePromotionReview,
    queryFn: () =>
      get<GenrePromotionReview>(`/queries/genre-promotion-review`),
  });
}

export function useGenrePromotionInodeDetail(release_id: string | null) {
  return useQuery({
    queryKey: queryKeys.genrePromotionInodeDetail(release_id ?? ""),
    enabled: release_id != null && release_id.length > 0,
    queryFn: () =>
      get<GenrePromotionInodeDetail>(
        `/queries/genre-promotion-inode-detail?release_id=${encodeURIComponent(
          release_id ?? "",
        )}`,
      ),
  });
}

export function useGenreCoverageSummary() {
  return useQuery({
    queryKey: queryKeys.genreCoverageSummary,
    queryFn: () =>
      get<GenreCoverageSummary>(`/queries/genre-coverage-summary`),
    refetchInterval: 30_000,
  });
}

export function useDeployStatus() {
  return useQuery({
    queryKey: queryKeys.deployStatus,
    queryFn: () => get<DeployStatus>("/queries/deploy-status"),
    refetchInterval: false,
  });
}

export function useDeployData() {
  return useQuery({
    queryKey: queryKeys.deployData,
    queryFn: () => get<DeployModalData>("/queries/deploy-data"),
  });
}

// -- Types for queries --

export interface SourceDir {
  path: string;
  libraries: string[];
  can_stash_dupes: boolean | null;
  interior_dupes: boolean | null;
  path_schema: string | null;
  enable_acoustid: boolean | null;
  pinned_release: string | null;
  cover_art_sanctity: string | null;
}

export interface DecisionDetail {
  key: unknown;
  label: string;
  mutations: unknown[];
}

// -- Auth helpers (not TanStack — imperative) --

export async function login(
  username: string,
  password: string,
): Promise<void> {
  // Server sets HttpOnly cookie on success. No token to store client-side.
  await post("/auth/login", { username, password });
}

export async function checkSetup(): Promise<{
  needs_setup: boolean;
  suggested_root: string | null;
}> {
  return post("/setup/check", {});
}

export async function completeSetup(
  root: string,
  username: string,
  password: string,
): Promise<void> {
  await post("/setup/complete", { root, username, password });
}
