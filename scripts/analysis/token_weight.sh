#!/bin/bash
# Token/Context Weight Estimation for MM
# Estimates cognitive load and LLM context usage per file, broken down by crate
#
# Approximation: ~4 characters per token (conservative for code)
# Also factors in: imports, type definitions, function count

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

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
echo "MM Token/Context Weight Analysis"
echo "Generated: $(date)"
echo "=============================================="
echo
echo "Estimation basis: ~4 chars/token (conservative for code)"
echo "Weights: base tokens + complexity factors"
echo

# ── Per-crate file tables ──

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    crate_src="${entry#*:}"

    [ -d "$crate_src" ] || continue

    file_count=$(find "$crate_src" -name "*.rs" -type f | wc -l | tr -d ' ')
    [ "$file_count" -eq 0 ] && continue

    echo "=============================================="
    echo "## Crate: $crate_name"
    echo "=============================================="
    echo

    printf "%-55s %8s %8s %8s %8s\n" "File" "Lines" "~Tokens" "Funcs" "Weight"
    printf "%-55s %8s %8s %8s %8s\n" "----" "-----" "-------" "-----" "------"

    find "$crate_src" -name "*.rs" -type f | sort | while read file; do
        rel_path="${file#./}"

        chars=$(wc -c < "$file" | tr -d ' ')
        lines=$(wc -l < "$file" | tr -d ' ')
        base_tokens=$((chars / 4))

        func_count=$(grep -c 'fn ' "$file" || true)
        struct_count=$(grep -c '^struct \|^pub struct \|^pub(crate) struct ' "$file" || true)
        enum_count=$(grep -c '^enum \|^pub enum \|^pub(crate) enum ' "$file" || true)
        impl_count=$(grep -c '^impl ' "$file" || true)

        func_count=${func_count:-0}
        struct_count=${struct_count:-0}
        enum_count=${enum_count:-0}
        impl_count=${impl_count:-0}

        complexity_bonus=$(( (struct_count + enum_count) * 50 + impl_count * 30 ))
        weight=$((base_tokens + complexity_bonus))

        printf "%-55s %8d %8d %8d %8d\n" "$rel_path" "$lines" "$base_tokens" "$func_count" "$weight"

        echo "$weight $crate_name $rel_path" >> "$TEMP_FILE"
    done

    echo
done

# ── Top 25 heaviest files across all crates ──

echo "=============================================="
echo "## Top 25 Heaviest Files (all crates)"
echo "=============================================="
echo

printf "%-55s %8s\n" "File" "Weight"
printf "%-55s %8s\n" "----" "------"

sort -rn "$TEMP_FILE" | head -25 | while read weight crate file; do
    printf "%-55s %8d\n" "$file" "$weight"
done

echo

# ── Crate-level context weight ──

echo "=============================================="
echo "## Crate-Level Context Weight"
echo "=============================================="
echo

total_weight=$(awk '{sum+=$1} END {print sum}' "$TEMP_FILE")

printf "%-15s %10s %10s %8s\n" "Crate" "Weight" "% of Total" "Files"
printf "%-15s %10s %10s %8s\n" "-----" "------" "----------" "-----"

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    c_weight=$(awk -v c="$crate_name" '$2==c {sum+=$1} END {print sum+0}' "$TEMP_FILE")
    c_files=$(awk -v c="$crate_name" '$2==c' "$TEMP_FILE" | wc -l | tr -d ' ')
    [ "$c_files" -eq 0 ] && continue
    pct=$((100 * c_weight / total_weight))
    printf "%-15s %10d %9d%% %8d\n" "$crate_name" "$c_weight" "$pct" "$c_files"
done

echo "------"
printf "%-15s %10d %10s\n" "TOTAL" "$total_weight" "100%"
echo
echo "Estimated full-codebase context: ~$total_weight tokens"
