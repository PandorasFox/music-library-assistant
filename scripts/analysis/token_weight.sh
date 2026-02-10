#!/bin/bash
# Token/Context Weight Estimation for MM
# Estimates cognitive load and LLM context usage per file
#
# Approximation: ~4 characters per token (conservative for code)
# Also factors in: imports, type definitions, function count

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

echo "=============================================="
echo "MM Token/Context Weight Analysis"
echo "Generated: $(date)"
echo "=============================================="
echo
echo "Estimation basis: ~4 chars/token (conservative for code)"
echo "Weights: base tokens + complexity factors"
echo

TEMP_FILE=$(mktemp)
trap "rm -f $TEMP_FILE" EXIT

# Header
printf "%-50s %8s %8s %8s %8s %8s\n" "File" "Chars" "Lines" "~Tokens" "Funcs" "Weight"
printf "%-50s %8s %8s %8s %8s %8s\n" "----" "-----" "-----" "-------" "-----" "------"

find ./src -name "*.rs" -type f | sort | while read file; do
    rel_path="${file#./src/}"

    # Basic metrics
    chars=$(wc -c < "$file" | tr -d ' ')
    lines=$(wc -l < "$file" | tr -d ' ')
    base_tokens=$((chars / 4))

    # Complexity factors (handle grep returning 1 on no match)
    func_count=$(grep -c 'fn ' "$file" || true)
    struct_count=$(grep -c '^struct \|^pub struct ' "$file" || true)
    enum_count=$(grep -c '^enum \|^pub enum ' "$file" || true)
    impl_count=$(grep -c '^impl ' "$file" || true)

    # Ensure numbers
    func_count=${func_count:-0}
    struct_count=${struct_count:-0}
    enum_count=${enum_count:-0}
    impl_count=${impl_count:-0}

    # Weight calculation: base + complexity bonus
    complexity_bonus=$(( (struct_count + enum_count) * 50 + impl_count * 30 ))
    weight=$((base_tokens + complexity_bonus))

    # Extract module
    if [[ "$rel_path" == *"/"* ]]; then
        module="${rel_path%%/*}"
    else
        module="(root)"
    fi

    printf "%-50s %8d %8d %8d %8d %8d\n" "$rel_path" "$chars" "$lines" "$base_tokens" "$func_count" "$weight"

    # Save for later aggregation
    echo "$weight $module $rel_path" >> "$TEMP_FILE"
done

echo
echo "=============================================="
echo "## Top 25 Heaviest Files (by context weight)"
echo "=============================================="
echo

printf "%-50s %8s\n" "File" "Weight"
printf "%-50s %8s\n" "----" "------"

sort -rn "$TEMP_FILE" | head -25 | while read weight module file; do
    printf "%-50s %8d\n" "$file" "$weight"
done

echo
echo "=============================================="
echo "## Module-Level Context Weight"
echo "=============================================="
echo

# Calculate total
total_weight=$(awk '{sum+=$1} END {print sum}' "$TEMP_FILE")

printf "%-20s %10s %10s\n" "Module" "Weight" "% of Total"
printf "%-20s %10s %10s\n" "------" "------" "----------"

# Aggregate by module
awk '{weights[$2]+=$1} END {for (m in weights) print weights[m], m}' "$TEMP_FILE" | sort -rn | while read weight module; do
    pct=$((100 * weight / total_weight))
    printf "%-20s %10d %9d%%\n" "$module" "$weight" "$pct"
done

echo
echo "------"
printf "%-20s %10d %10s\n" "TOTAL" "$total_weight" "100%"
echo
echo "Estimated full-codebase context: ~$total_weight tokens"
echo "(This is src/ only - excludes tests, docs, configs)"
