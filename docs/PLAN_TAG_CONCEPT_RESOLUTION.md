# Plan: Tag Concept Resolution Architecture

## Problem Statement

Music files use inconsistent tag naming conventions. The same semantic information can appear under many different tag names:

| Concept | Observed Variants |
|---------|-------------------|
| Catalog Number | `catalog_number`, `CATALOGNUMBER`, `cataloguenumber`, `Catalog`, `catalog#` |
| Album Artist | `album_artist`, `ALBUMARTIST`, `AlbumArtist`, `Album Artist` |
| Track Number | `track_number`, `tracknumber`, `TRACKNUMBER`, `track` |

**Current state:**
- Lofty reads tags and maps them to ItemKey variants (e.g., `ItemKey::CatalogNumber`)
- When writing to DB, we use Lofty's string representation (e.g., `catalog_number`)
- Queries hardcode specific tag names (e.g., `WHERE tag_name = 'catalognumber'`)
- This causes mismatches - DB has `catalog_number`, query looks for `catalognumber`

**Root causes:**
1. No normalization at write time
2. No fuzzy matching at query time
3. SQL queries can't use application-level Levenshtein logic

## Existing Infrastructure

We have `mm-utils::tag_names` with:
- `normalize_tag_name()` - Strips separators, lowercases
- `levenshtein_distance()` - Edit distance for spelling variants
- `tag_names_match()` - Returns `Exact`, `LikelyVariant`, or `NoMatch`
- `find_tag_in_map()` - Fuzzy lookup in HashMap

These work for application-level lookups but not for SQL queries.

## Design Goals

1. **Data-driven** - Don't hardcode all variants; discover what's in the corpus
2. **Configurable** - Allow user to define custom concept mappings (future)
3. **SQL-compatible** - Support efficient SQL queries with fuzzy matching
4. **Gradual migration** - Can be adopted incrementally
5. **Lofty-aligned defaults** - Default canonical names match Lofty's output

## Proposed Architecture

### Layer 1: Tag Concept Definitions

Define core semantic concepts with their canonical forms. These should align with Lofty's `ItemKey` string representations as defaults:

| Concept | Lofty ItemKey | Default Canonical | Max Edit Distance |
|---------|---------------|-------------------|-------------------|
| Album | `ItemKey::AlbumTitle` | `album` | 0 |
| AlbumArtist | `ItemKey::AlbumArtist` | `album_artist` | 0 |
| Artist | `ItemKey::TrackArtist` | `artist` | 0 |
| CatalogNumber | `ItemKey::CatalogNumber` | `catalog_number` | 2 (catalog/catalogue) |
| DiscNumber | `ItemKey::DiscNumber` | `disc_number` | 0 |
| Genre | `ItemKey::Genre` | `genre` | 0 |
| Isrc | `ItemKey::Isrc` | `isrc` | 0 |
| Title | `ItemKey::TrackTitle` | `title` | 0 |
| TrackNumber | `ItemKey::TrackNumber` | `track_number` | 0 |
| Year | `ItemKey::Year` | `year` | 0 |

**Note:** The canonical form is what we compare against after normalization (separator stripping). So `catalog_number` normalizes to `catalognumber` for comparison.

### Layer 2: Tag Name Discovery & Caching (Witch-Level)

The tag concept → actual tag names mapping should be:

1. **Computed during Awake stage** - Possibly as the very first awakening computation
2. **Cached in the Witch** - As a pure Rust structure holding `{Concept => Vec<String>}`
3. **ARC-shared for reads** - All other computations and UI code can read the cached mapping

```rust
// In Witch or a dedicated TagConceptCache
pub struct TagConceptCache {
    /// Maps each concept to the actual tag names found in the corpus
    concept_to_names: HashMap<TagConcept, Vec<String>>,
    /// All distinct tag names in the corpus
    all_tag_names: Vec<String>,
    /// When this cache was last computed
    computed_at: Instant,
}
```

Query to populate:
```sql
SELECT DISTINCT tag_name FROM track_tags;
```

This cache is populated once per awakening cycle, then read by all subsequent computations.

### Layer 3: Concept Resolution

Given a concept and available tag names, find all matching names using:

1. Normalize candidate name (strip separators `[_-. ]`, lowercase)
2. Compare against concept's canonical normalized form
3. If exact match → include
4. If within max edit distance → include (for regional variants)

```rust
fn resolve_tag_names_for_concept(
    concept: &TagConcept,
    available_names: &[String],
) -> Vec<String>
```

### Layer 4: SQL Query Building

For SQL queries needing concept-based matching:

```rust
fn build_tag_name_sql_condition(matching_names: &[String]) -> String
// Returns: "LOWER(tag_name) IN ('catalog_number', 'cataloguenumber')"
```

## Implementation Phases

### Phase 1: Core Infrastructure

1. **Define `TagConcept` enum** in `src/corpus/tag_concepts.rs`
   - All core concepts with canonical forms (aligned with Lofty's ItemKey strings)
   - Max edit distance per concept
   - Methods for resolution

2. **Add `get_distinct_tag_names()` query** to Database
   - Returns all unique tag names in corpus

3. **Create `TagConceptCache` struct**
   - Lives in Witch or dedicated module
   - Holds `{Concept => Vec<String>}` mapping
   - ARC-wrapped for concurrent reads

4. **Add `PopulateTagConceptCache` computation**
   - First computation in Awake stage
   - Queries all distinct tag names
   - Resolves each concept against available names
   - Populates the cache for all subsequent computations

5. **Concept resolution functions**
   - `resolve_tag_names_for_concept(concept, available) -> Vec<String>`
   - Uses `mm-utils::tag_names` primitives internally

6. **SQL condition builder**
   - `build_tag_name_condition(names) -> String`

### Phase 2: Retrofit Computations

Update all computations that query by tag name to use concept resolution:

1. **`get_album_data_for_collision_detection`** (metadata.rs)
   - Uses `CatalogNumber` concept for catalog lookup
   - Uses `AlbumArtist`, `Artist`, `Isrc` concepts

2. **`execute_analyze_fingerprint_overlaps`** (executors.rs)
   - Uses concept resolution for tag lookups
   - Currently uses `find_tag_in_map` - good, but verify it uses concepts

3. **`execute_cluster_directory_overlaps`** (executors.rs)
   - Same pattern as fingerprint overlaps

4. **Collision detection functions** (collision.rs)
   - `get_artist_collisions` → uses `Artist` concept
   - `get_album_artist_collisions` → uses `AlbumArtist` concept
   - `get_album_collisions` → uses `Album` concept
   - `get_genre_collisions` → uses `Genre` concept

5. **Tag search/filter UI**
   - Anywhere users search by tag name should be concept-aware

### Phase 3: Config Integration (Future)

Add `tag-concepts` section to config schema for user customization:

```kdl
tag-concepts {
    catalog-number {
        canonical "catalog_number"
        aliases "catalog#" "cat#" "catno"
        max-edit-distance 2
    }
}
```

### Phase 4: Tag Name Inconsistency Detection (Future)

New computation to detect when corpus has inconsistent tag names:

- e.g., "Found 3414 tracks with 'catalog_number' and 9 with 'Catalog'"
- Emit signals for bulk tag name normalization
- UI modal for reviewing and fixing

---

## Follow-On Work: Lateral-Ring Config Editor

After this plan is complete, a **new plan** will be needed for:

1. **Runtime config editor modal** - Safe, constrained editing of tag concept mappings
2. **UI for computation fuzzing** - Adjust thresholds and behavior at runtime
3. **Preview/dry-run mode** - See what would change before applying

This follow-on UI work is out of scope for this plan and will be drafted separately.

---

## Files to Create/Modify

### New Files
- `src/corpus/tag_concepts.rs` - Core concept definitions and resolution

### Modified Files
- `src/corpus/mod.rs` - Export tag_concepts module
- `src/corpus/db/queries/mod.rs` - Add `get_distinct_tag_names()`
- `src/corpus/db/queries/metadata.rs` - Use concept resolution in queries
- `src/corpus/health/collision.rs` - Use concept resolution
- `src/corpus/computations/awake/executors.rs` - Use concept resolution

## Testing Strategy

1. Unit tests for concept resolution logic
2. Integration test with corpus data containing tag name variants
3. Verify Monstercat files (MCB009 vs MC012) are correctly handled

## Success Criteria

1. `catalog_number` and `cataloguenumber` both resolve to CatalogNumber concept
2. `get_album_data_for_collision_detection` correctly finds catalog numbers
3. Fingerprint duplicate detection correctly identifies compilation variants
4. No hardcoded tag name strings in computation logic (use concepts)
