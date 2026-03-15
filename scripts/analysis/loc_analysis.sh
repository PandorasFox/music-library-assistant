#!/bin/bash
# Lines of Code Analysis for MM
# Analyzes .rs files for LoC metrics, broken down by crate

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

# All crate source directories
CRATES=(
    "mm:src"
    "mm-tui:crates/mm-tui/src"
    "mm-ui:crates/mm-ui/src"
    "mm-meta:crates/mm-meta/src"
    "mm-utils:crates/mm-utils/src"
    "mm-derive:crates/mm-derive/src"
    "mm-web:crates/mm-web/src"
    "mm-web-client:crates/mm-web/client/src"
)

TEMP_FILE=$(mktemp)
trap "rm -f $TEMP_FILE" EXIT

echo "=============================================="
echo "MM Lines of Code Analysis"
echo "Generated: $(date)"
echo "=============================================="
echo

# ── Per-crate analysis ──

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    crate_src="${entry#*:}"

    [ -d "$crate_src" ] || continue

    file_count=$(find "$crate_src" -name "*.rs" -type f | wc -l | tr -d ' ')
    [ "$file_count" -eq 0 ] && continue

    total_lines=$(find "$crate_src" -name "*.rs" -type f -exec cat {} + | wc -l | tr -d ' ')

    echo "=============================================="
    echo "## Crate: $crate_name  ($file_count files, $total_lines lines)"
    echo "=============================================="
    echo

    echo "### Top Files (by LoC)"
    echo
    find "$crate_src" -name "*.rs" -type f -exec wc -l {} \; | sort -rn | head -30 | \
        while read count path; do
            printf "%6d  %s\n" "$count" "${path#./}"
        done
    echo

    echo "### Module Breakdown"
    echo
    printf "%-30s %8s %8s %8s\n" "Module" "Files" "Total" "Mean"
    printf "%-30s %8s %8s %8s\n" "------" "-----" "-----" "----"

    # Root-level files in this crate
    root_files=$(find "$crate_src" -maxdepth 1 -name "*.rs" -type f | wc -l | tr -d ' ')
    if [ "$root_files" -gt 0 ]; then
        root_lines=$(find "$crate_src" -maxdepth 1 -name "*.rs" -type f -exec cat {} + | wc -l | tr -d ' ')
        root_mean=$((root_lines / root_files))
        printf "%-30s %8d %8d %8d\n" "(root)" "$root_files" "$root_lines" "$root_mean"
    fi

    for module_dir in "$crate_src"/*/; do
        [ -d "$module_dir" ] || continue
        module_name=$(basename "$module_dir")
        mod_files=$(find "$module_dir" -name "*.rs" -type f | wc -l | tr -d ' ')
        if [ "$mod_files" -gt 0 ]; then
            mod_lines=$(find "$module_dir" -name "*.rs" -type f -exec cat {} + | wc -l | tr -d ' ')
            mod_mean=$((mod_lines / mod_files))
            printf "%-30s %8d %8d %8d\n" "$module_name" "$mod_files" "$mod_lines" "$mod_mean"
        fi
    done

    echo

    # Save per-file counts for the cross-crate summary
    find "$crate_src" -name "*.rs" -type f -exec wc -l {} \; | awk -v c="$crate_name" '{print $1, c}' >> "$TEMP_FILE"
done

# ── Cross-crate summary ──

echo "=============================================="
echo "## Cross-Crate Summary"
echo "=============================================="
echo

printf "%-15s %8s %10s %8s %8s %8s\n" "Crate" "Files" "Lines" "Mean" "Median" "Max"
printf "%-15s %8s %10s %8s %8s %8s\n" "-----" "-----" "-----" "----" "------" "---"

total_files=0
total_lines=0

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    crate_counts=$(mktemp)
    grep " ${crate_name}$" "$TEMP_FILE" | awk '{print $1}' | sort -n > "$crate_counts"

    c_files=$(wc -l < "$crate_counts" | tr -d ' ')
    [ "$c_files" -eq 0 ] && rm -f "$crate_counts" && continue

    c_total=$(awk '{sum+=$1} END {print sum}' "$crate_counts")
    c_mean=$((c_total / c_files))
    c_median=$(sed -n "$((c_files / 2 + 1))p" "$crate_counts")
    c_max=$(tail -1 "$crate_counts")

    printf "%-15s %8d %10d %8d %8d %8d\n" "$crate_name" "$c_files" "$c_total" "$c_mean" "$c_median" "$c_max"

    total_files=$((total_files + c_files))
    total_lines=$((total_lines + c_total))
    rm -f "$crate_counts"
done

echo "------"
printf "%-15s %8d %10d\n" "TOTAL" "$total_files" "$total_lines"
echo

# ── Size distribution (all crates) ──

echo "=============================================="
echo "## Size Distribution (all crates)"
echo "=============================================="
echo

all_counts=$(mktemp)
awk '{print $1}' "$TEMP_FILE" | sort -n > "$all_counts"

small=$(awk '$1 < 100' "$all_counts" | wc -l | tr -d ' ')
medium=$(awk '$1 >= 100 && $1 < 500' "$all_counts" | wc -l | tr -d ' ')
large=$(awk '$1 >= 500 && $1 < 1000' "$all_counts" | wc -l | tr -d ' ')
xlarge=$(awk '$1 >= 1000' "$all_counts" | wc -l | tr -d ' ')

printf "Small   (<100):     %3d files (%2d%%)\n" "$small" "$((100 * small / total_files))"
printf "Medium  (100-500):  %3d files (%2d%%)\n" "$medium" "$((100 * medium / total_files))"
printf "Large   (500-1000): %3d files (%2d%%)\n" "$large" "$((100 * large / total_files))"
printf "X-Large (>1000):    %3d files (%2d%%)\n" "$xlarge" "$((100 * xlarge / total_files))"

rm -f "$all_counts"
