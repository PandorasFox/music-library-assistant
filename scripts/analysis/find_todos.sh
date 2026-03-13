#!/bin/bash
# TODO/Stub/Technical Debt Finder for MM
# Surfaces incomplete implementations and markers, broken down by crate

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

CRATES=(
    "mm:src"
    "mm-tui:crates/mm-tui/src"
    "mm-meta:crates/mm-meta/src"
    "mm-utils:crates/mm-utils/src"
    "mm-derive:crates/mm-derive/src"
)

echo "=============================================="
echo "MM Technical Debt Analysis"
echo "Generated: $(date)"
echo "=============================================="

# Helper: search a pattern in a crate, emit formatted results
# Args: $1=crate_src $2=pattern $3=label (for "(none found)")
search_pattern() {
    local crate_src="$1" pattern="$2"
    local found=0
    grep -rn "$pattern" "$crate_src" --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
        rel_path="${file#./}"
        printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
        found=1
    done
    # Check if grep found anything (subshell means $found doesn't propagate)
    if ! grep -rq "$pattern" "$crate_src" --include="*.rs" 2>/dev/null; then
        echo "(none)"
    fi
}

# Helper: count pattern occurrences in a crate
count_pattern() {
    local crate_src="$1" pattern="$2"
    grep -r "$pattern" "$crate_src" --include="*.rs" 2>/dev/null | wc -l | tr -d ' '
}

MARKERS=(
    'todo!(:todo!() macros'
    'unimplemented!(:unimplemented!() macros'
    '// TODO\|// todo\|//TODO\|//todo:TODO comments'
    '// FIXME\|// fixme\|//FIXME\|//fixme:FIXME comments'
    '// HACK\|// hack\|//HACK\|//hack:HACK comments'
    '#\[allow(dead_code)\]:#[allow(dead_code)] (policy: macro false-positives only)'
    '#\[allow(unused:#[allow(unused_*)]'
    'unreachable!(:unreachable!() patterns'
)

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    crate_src="${entry#*:}"

    [ -d "$crate_src" ] || continue

    file_count=$(find "$crate_src" -name "*.rs" -type f | wc -l | tr -d ' ')
    [ "$file_count" -eq 0 ] && continue

    echo
    echo "=============================================="
    echo "## Crate: $crate_name"
    echo "=============================================="

    for marker in "${MARKERS[@]}"; do
        pattern="${marker%%:*}"
        label="${marker#*:}"
        echo
        echo "### $label"
        echo
        search_pattern "$crate_src" "$pattern"
    done
done

# ── Cross-crate summary table ──

echo
echo "=============================================="
echo "## Summary Statistics"
echo "=============================================="
echo

printf "%-15s %6s %6s %6s %6s %6s %6s %6s\n" \
    "Crate" "todo!" "unimp" "TODO" "FIXME" "dead" "unused" "Total"
printf "%-15s %6s %6s %6s %6s %6s %6s %6s\n" \
    "-----" "-----" "-----" "-----" "-----" "-----" "------" "-----"

grand_total=0

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    crate_src="${entry#*:}"

    [ -d "$crate_src" ] || continue

    t=$(count_pattern "$crate_src" 'todo!(')
    u=$(count_pattern "$crate_src" 'unimplemented!(')
    c=$(count_pattern "$crate_src" '// TODO\|// todo\|//TODO\|//todo')
    f=$(count_pattern "$crate_src" '// FIXME\|// fixme\|//FIXME\|//fixme')
    d=$(count_pattern "$crate_src" '#\[allow(dead_code)\]')
    n=$(count_pattern "$crate_src" '#\[allow(unused')
    row_total=$((t + u + c + f))

    [ "$row_total" -eq 0 ] && [ "$d" -eq 0 ] && [ "$n" -eq 0 ] && continue

    printf "%-15s %6d %6d %6d %6d %6d %6d %6d\n" \
        "$crate_name" "$t" "$u" "$c" "$f" "$d" "$n" "$row_total"

    grand_total=$((grand_total + row_total))
done

echo "------"
echo "Total technical debt markers: $grand_total"
