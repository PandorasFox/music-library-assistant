#!/bin/bash
# Lines of Code Analysis for MLA
# Analyzes .rs files for LoC metrics at file and module level

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

echo "=============================================="
echo "MLA Lines of Code Analysis"
echo "Generated: $(date)"
echo "=============================================="
echo

echo "## Per-File Line Counts (sorted by LoC, descending)"
echo "---------------------------------------------------"
echo

# Get line counts for all .rs files, sorted by count
find ./src -name "*.rs" -type f -exec wc -l {} \; | sort -rn | head -150

echo
echo "=============================================="
echo "## Top 25 Largest Files"
echo "=============================================="
echo

find ./src -name "*.rs" -type f -exec wc -l {} \; | sort -rn | head -25

echo
echo "=============================================="
echo "## Module-Level Aggregation"
echo "=============================================="
echo

printf "%-20s %8s %8s %8s\n" "Module" "Files" "Total" "Mean"
printf "%-20s %8s %8s %8s\n" "------" "-----" "-----" "----"

# Root level files
root_files=$(find ./src -maxdepth 1 -name "*.rs" -type f | wc -l | tr -d ' ')
root_lines=$(find ./src -maxdepth 1 -name "*.rs" -type f -exec cat {} \; | wc -l | tr -d ' ')
if [ "$root_files" -gt 0 ]; then
    root_mean=$((root_lines / root_files))
    printf "%-20s %8d %8d %8d\n" "(root)" "$root_files" "$root_lines" "$root_mean"
fi

# Each top-level module
for module_dir in ./src/*/; do
    if [ -d "$module_dir" ]; then
        module_name=$(basename "$module_dir")
        module_files=$(find "$module_dir" -name "*.rs" -type f | wc -l | tr -d ' ')
        if [ "$module_files" -gt 0 ]; then
            module_lines=$(find "$module_dir" -name "*.rs" -type f -exec cat {} \; | wc -l | tr -d ' ')
            module_mean=$((module_lines / module_files))
            printf "%-20s %8d %8d %8d\n" "$module_name" "$module_files" "$module_lines" "$module_mean"
        fi
    fi
done

echo
echo "=============================================="
echo "## Quartile Analysis (All Files)"
echo "=============================================="
echo

# Get all line counts into a temp file, sorted
TEMP_FILE=$(mktemp)
trap "rm -f $TEMP_FILE" EXIT
find ./src -name "*.rs" -type f -exec wc -l {} \; | awk '{print $1}' | sort -n > "$TEMP_FILE"

total_files=$(wc -l < "$TEMP_FILE" | tr -d ' ')
q1_line=$((total_files / 4))
q2_line=$((total_files / 2))
q3_line=$((3 * total_files / 4))

min_val=$(head -1 "$TEMP_FILE")
max_val=$(tail -1 "$TEMP_FILE")
q1_val=$(sed -n "${q1_line}p" "$TEMP_FILE")
q2_val=$(sed -n "${q2_line}p" "$TEMP_FILE")
q3_val=$(sed -n "${q3_line}p" "$TEMP_FILE")

echo "Total files: $total_files"
echo
echo "Min:      $min_val lines"
echo "Q1 (25%): $q1_val lines"
echo "Q2 (50%): $q2_val lines (median)"
echo "Q3 (75%): $q3_val lines"
echo "Max:      $max_val lines"
echo

# Calculate distribution buckets
small=$(awk '$1 < 100' "$TEMP_FILE" | wc -l | tr -d ' ')
medium=$(awk '$1 >= 100 && $1 < 500' "$TEMP_FILE" | wc -l | tr -d ' ')
large=$(awk '$1 >= 500 && $1 < 1000' "$TEMP_FILE" | wc -l | tr -d ' ')
xlarge=$(awk '$1 >= 1000' "$TEMP_FILE" | wc -l | tr -d ' ')

echo "## Size Distribution"
echo "-------------------"
printf "Small   (<100):     %3d files (%2d%%)\n" "$small" "$((100 * small / total_files))"
printf "Medium  (100-500):  %3d files (%2d%%)\n" "$medium" "$((100 * medium / total_files))"
printf "Large   (500-1000): %3d files (%2d%%)\n" "$large" "$((100 * large / total_files))"
printf "X-Large (>1000):    %3d files (%2d%%)\n" "$xlarge" "$((100 * xlarge / total_files))"

echo
echo "=============================================="
echo "## Total Project Statistics"
echo "=============================================="
echo

total_lines=$(cat "$TEMP_FILE" | awk '{sum+=$1} END {print sum}')
echo "Total .rs files: $total_files"
echo "Total lines:     $total_lines"
echo "Average LoC:     $((total_lines / total_files))"
