# Release Packing Numbers

Tracking packing pipeline output across commits to measure the impact of changes.

## Baseline Comparison

| Metric | beta 9 (a2621b3) | +dir-constrained (32bf38c) | +threshold/sibling fix (b39ee07) | +singles-before-incompletes | +exhaustive dir scoring (c117aa2) | +discography reduction |
|---|---|---|---|---|---|---|
| Perfect | — | 1117 | 1101 | 1101 | 1402 | 1402 |
| Full | 1510 | 380 | 466 | 468 | 145 | 137 |
| **Complete total** | **1510** | **1497** | **1567** | **1569** | **1547** | **1539** |
| Singles | 1682 | 1756 | 1765 | 2272 | 2250 | 2254 |
| Incomplete | 1744 | 2279 | 2164 | 1681 | 1695 | 1701 |
| Knots | — | — | — | — | 6 | 2 |
| Unmatched | 13477 | 12446 | 12011 | 12132 | 12244 | 12373 |

### Notes

- beta 9 had no perfect/full split — all complete releases were "full"
- beta 9 had 6 near-misses (removed as signal type; folded into incomplete)
- Dir-constrained commit: replaced `directory_cohesion` scoring gradient with structural directory constraint, removed `Scattered` category
- Threshold/sibling fix: removed `ELIMINATION_SCORE_THRESHOLD` (0.35), replaced strict bijection with density-based sibling dir mapping, added per-medium elimination affinity
- ~1931 of the 2164 incompletes are single-file packings — candidates for singles reordering
- Exhaustive dir scoring: replaced heuristic `select_target_directory()` with `score_all_directories()` — runs Hungarian for every candidate directory, picks best by actual assignment score. Massive Perfect uplift (+301) as directories with better actual assignments are now chosen over heuristic-preferred ones. Full matches drop correspondingly as many reclassify to Perfect. Results now deterministic across runs.
- Discography reduction: consolidated packing config into `release-packing` block. Knots with covering proposals (inode superset) are resolved automatically — covering proposal emitted as normal pick, no knot signal. Remaining knots emit signal only (no picks, contested inodes unclaimed). Knots drop from 6→2 as 4 were resolved by discography reduction.
