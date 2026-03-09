# Release Packing Numbers

Tracking packing pipeline output across commits to measure the impact of changes.

## Baseline Comparison

| Metric | beta 9 (a2621b3) | +dir-constrained (32bf38c) | +threshold/sibling fix (b39ee07) | +singles-before-incompletes |
|---|---|---|---|---|
| Perfect | — | 1117 | 1101 | 1101 |
| Full | 1510 | 380 | 466 | 468 |
| **Complete total** | **1510** | **1497** | **1567** | **1569** |
| Singles | 1682 | 1756 | 1765 | 2272 |
| Incomplete | 1744 (+6 near-miss) | 2279 | 2164 | 1681 |
| Unmatched | 13477 | 12446 | 12011 | 12132 |

### Notes

- beta 9 had no perfect/full split — all complete releases were "full"
- beta 9 had 6 near-misses (folded into incomplete count for comparison)
- Dir-constrained commit: replaced `directory_cohesion` scoring gradient with structural directory constraint, removed `Scattered` category
- Threshold/sibling fix: removed `ELIMINATION_SCORE_THRESHOLD` (0.35), replaced strict bijection with density-based sibling dir mapping, added per-medium elimination affinity
- ~1931 of the 2164 incompletes are single-file packings — candidates for singles reordering
