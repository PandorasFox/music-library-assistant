//! Maximum Independent Set solver for release packing conflict resolution.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use crate::logging::log_general;

/// Entry for the MIS solver: an inode set and associated score.
pub(super) struct MisCandidate {
    pub inode_set: HashSet<i64>,
    pub score: f64,
}

/// Result of solving a Maximum Independent Set problem.
pub(super) struct MisResult {
    /// Which candidates were selected (parallel to input slice).
    pub selected: Vec<bool>,
    /// Total number of selected candidates.
    pub selected_count: usize,
    /// Total coverage: sum of inode_set.len() for selected candidates.
    pub selected_coverage: usize,
}

/// Solve Maximum Independent Set: select candidates whose inode sets are
/// pairwise disjoint, maximizing total coverage (sum of inode set sizes).
/// Tiebreak on total score.
///
/// Uses exhaustive bitmask enumeration for components ≤ 25, branch-and-bound
/// for larger components.
pub(super) fn solve_maximum_independent_set(candidates: &[MisCandidate]) -> MisResult {
    let n = candidates.len();
    if n == 0 {
        return MisResult {
            selected: Vec::new(),
            selected_count: 0,
            selected_coverage: 0,
        };
    }

    // Build conflict adjacency: edge between candidates that share any inode
    // Uses sorted Vec<usize> instead of HashSet for deterministic iteration order.
    let mut inode_to_idx: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, cand) in candidates.iter().enumerate() {
        for &inode in &cand.inode_set {
            inode_to_idx.entry(inode).or_default().push(i);
        }
    }

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for indices in inode_to_idx.values() {
        if indices.len() > 1 {
            for &i in indices {
                for &j in indices {
                    if i != j {
                        adj[i].push(j);
                    }
                }
            }
        }
    }
    for neighbors in &mut adj {
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    drop(inode_to_idx);

    // Find connected components via BFS
    let mut component_id: Vec<Option<usize>> = vec![None; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if component_id[start].is_some() {
            continue;
        }
        let cid = components.len();
        let mut comp = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        component_id[start] = Some(cid);
        while let Some(node) = queue.pop_front() {
            comp.push(node);
            for &neighbor in &adj[node] {
                if component_id[neighbor].is_none() {
                    component_id[neighbor] = Some(cid);
                    queue.push_back(neighbor);
                }
            }
        }
        comp.sort_unstable();
        components.push(comp);
    }

    let mut selected = vec![false; n];
    let mut selected_count = 0usize;
    let mut selected_coverage = 0usize;

    // Log component distribution for visibility
    {
        let isolated = components.iter().filter(|c| c.len() == 1).count();
        let small = components
            .iter()
            .filter(|c| (2..=5).contains(&c.len()))
            .count();
        let bitmask = components
            .iter()
            .filter(|c| (6..=25).contains(&c.len()))
            .count();
        let bnb = components.iter().filter(|c| c.len() > 25).count();
        let bnb_sizes: Vec<usize> = components
            .iter()
            .filter(|c| c.len() > 25)
            .map(|c| c.len())
            .collect();
        log_general(format!(
            "[COMPUTE] MIS: {} components (isolated={}, small={}, bitmask={}, bnb={}{})",
            components.len(),
            isolated,
            small,
            bitmask,
            bnb,
            if bnb_sizes.is_empty() {
                String::new()
            } else {
                format!(" sizes={:?}", bnb_sizes)
            },
        ));
    }

    for (ci, component) in components.iter().enumerate() {
        if component.len() == 1 {
            selected[component[0]] = true;
            selected_count += 1;
            selected_coverage += candidates[component[0]].inode_set.len();
            continue;
        }

        if component.len() <= 25 {
            // Exhaustive bitmask enumeration
            let k = component.len();
            let mut best_mask: u32 = 0;
            let mut best_coverage: usize = 0;
            let mut best_score: f64 = f64::NEG_INFINITY;

            for mask in 1u32..(1u32 << k) {
                let mut claimed: HashSet<i64> = HashSet::new();
                let mut feasible = true;
                let mut coverage = 0usize;
                let mut score = 0.0f64;

                for (bit, &gi) in component.iter().enumerate() {
                    if mask & (1 << bit) == 0 {
                        continue;
                    }
                    if candidates[gi]
                        .inode_set
                        .iter()
                        .any(|inode| claimed.contains(inode))
                    {
                        feasible = false;
                        break;
                    }
                    claimed.extend(&candidates[gi].inode_set);
                    coverage += candidates[gi].inode_set.len();
                    score += candidates[gi].score;
                }

                if feasible
                    && (coverage > best_coverage
                        || (coverage == best_coverage && score > best_score)
                        || (coverage == best_coverage
                            && score == best_score
                            && mask < best_mask))
                {
                    best_coverage = coverage;
                    best_score = score;
                    best_mask = mask;
                }
            }

            for (bit, &gi) in component.iter().enumerate() {
                if best_mask & (1 << bit) != 0 {
                    selected[gi] = true;
                    selected_count += 1;
                    selected_coverage += candidates[gi].inode_set.len();
                }
            }
        } else {
            // Branch-and-bound for larger components
            let comp_size = component.len();
            let comp_edges: usize = component
                .iter()
                .map(|&gi| adj[gi].iter().filter(|&&n| component.contains(&n)).count())
                .sum::<usize>()
                / 2;
            log_general(format!(
                "[COMPUTE] MIS: solving BnB component {}/{} (nodes={}, edges={})",
                ci + 1,
                components.len(),
                comp_size,
                comp_edges,
            ));

            let mut global_to_local: HashMap<usize, usize> = HashMap::new();
            for (li, &gi) in component.iter().enumerate() {
                global_to_local.insert(gi, li);
            }
            let mut local_adj: Vec<Vec<usize>> = vec![Vec::new(); comp_size];
            for (li, &gi) in component.iter().enumerate() {
                for &neighbor in &adj[gi] {
                    if let Some(&ln) = global_to_local.get(&neighbor) {
                        local_adj[li].push(ln);
                    }
                }
                local_adj[li].sort_unstable();
                local_adj[li].dedup();
            }

            let local_scores: Vec<f64> = component.iter().map(|&gi| candidates[gi].score).collect();
            let local_coverages: Vec<usize> = component
                .iter()
                .map(|&gi| candidates[gi].inode_set.len())
                .collect();
            let local_inode_sets: Vec<&HashSet<i64>> = component
                .iter()
                .map(|&gi| &candidates[gi].inode_set)
                .collect();

            let mut best_coverage: usize = 0;
            let mut best_score: f64 = f64::NEG_INFINITY;
            let mut best_selected: Vec<bool> = vec![false; comp_size];

            struct BnBState {
                candidates: Vec<usize>,
                selected: Vec<bool>,
                selected_coverage: usize,
                selected_score: f64,
                claimed_inodes: HashSet<i64>,
            }

            let initial_candidates: Vec<usize> = (0..comp_size).collect();
            let mut stack: Vec<BnBState> = vec![BnBState {
                candidates: initial_candidates,
                selected: vec![false; comp_size],
                selected_coverage: 0,
                selected_score: 0.0,
                claimed_inodes: HashSet::new(),
            }];
            let bnb_start = Instant::now();
            let mut iterations = 0u64;

            while let Some(state) = stack.pop() {
                iterations += 1;
                // Upper bound: current coverage + all remaining candidates' coverage
                let remaining_max_coverage: usize =
                    state.candidates.iter().map(|&c| local_coverages[c]).sum();
                if state.selected_coverage + remaining_max_coverage < best_coverage {
                    continue;
                }
                if state.selected_coverage + remaining_max_coverage == best_coverage {
                    let remaining_max_score: f64 =
                        state.candidates.iter().map(|&c| local_scores[c]).sum();
                    if state.selected_score + remaining_max_score <= best_score {
                        continue;
                    }
                }

                if state.candidates.is_empty() {
                    let dominated = state.selected_coverage > best_coverage
                        || (state.selected_coverage == best_coverage
                            && state.selected_score > best_score)
                        || (state.selected_coverage == best_coverage
                            && state.selected_score == best_score
                            && state.selected < best_selected);
                    if dominated {
                        best_coverage = state.selected_coverage;
                        best_score = state.selected_score;
                        best_selected = state.selected.clone();
                    }
                    continue;
                }

                let pivot = *state
                    .candidates
                    .iter()
                    .max_by_key(|&&c| {
                        let degree = state
                            .candidates
                            .iter()
                            .filter(|&&other| local_adj[c].contains(&other))
                            .count();
                        (degree, std::cmp::Reverse(c))
                    })
                    .unwrap();

                // Branch B: EXCLUDE pivot (push first so INCLUDE is explored first)
                {
                    let new_candidates: Vec<usize> = state
                        .candidates
                        .iter()
                        .copied()
                        .filter(|&c| c != pivot)
                        .collect();
                    stack.push(BnBState {
                        candidates: new_candidates,
                        selected: state.selected.clone(),
                        selected_coverage: state.selected_coverage,
                        selected_score: state.selected_score,
                        claimed_inodes: state.claimed_inodes.clone(),
                    });
                }

                // Branch A: INCLUDE pivot
                {
                    let neighbors: HashSet<usize> = local_adj[pivot].iter().copied().collect();
                    let pivot_inodes = local_inode_sets[pivot];
                    if !pivot_inodes
                        .iter()
                        .any(|i| state.claimed_inodes.contains(i))
                    {
                        let new_candidates: Vec<usize> = state
                            .candidates
                            .iter()
                            .copied()
                            .filter(|&c| c != pivot && !neighbors.contains(&c))
                            .collect();
                        let mut new_selected = state.selected.clone();
                        new_selected[pivot] = true;
                        let mut new_claimed = state.claimed_inodes.clone();
                        new_claimed.extend(pivot_inodes);
                        stack.push(BnBState {
                            candidates: new_candidates,
                            selected: new_selected,
                            selected_coverage: state.selected_coverage + local_coverages[pivot],
                            selected_score: state.selected_score + local_scores[pivot],
                            claimed_inodes: new_claimed,
                        });
                    }
                }
            }

            let bnb_elapsed = bnb_start.elapsed();
            let bnb_selected: usize = best_selected.iter().filter(|&&s| s).count();
            log_general(format!(
                "[COMPUTE] MIS: BnB component done: {} selected, coverage={}, {:.1}s, {} iterations",
                bnb_selected, best_coverage, bnb_elapsed.as_secs_f64(), iterations,
            ));

            for (li, &gi) in component.iter().enumerate() {
                if best_selected[li] {
                    selected[gi] = true;
                    selected_count += 1;
                    selected_coverage += candidates[gi].inode_set.len();
                }
            }
        }
    }

    MisResult {
        selected,
        selected_count,
        selected_coverage,
    }
}
