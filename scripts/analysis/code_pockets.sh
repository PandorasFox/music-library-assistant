#!/bin/bash
# Code Pockets Analysis for MM
# Identifies potentially isolated/dead code clusters, broken down by crate
#
# Looks for:
# - Functions with no external callers
# - pub items only used within their own module
# - Structs/enums with limited usage

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

# All source dirs for cross-crate usage checks
ALL_SRC_DIRS=()
for entry in "${CRATES[@]}"; do
    src="${entry#*:}"
    [ -d "$src" ] && ALL_SRC_DIRS+=("$src")
done

echo "=============================================="
echo "MM Code Pockets Analysis"
echo "Generated: $(date)"
echo "=============================================="
echo
echo "This analysis identifies potentially isolated code that may be"
echo "internally integrated but have limited external usage."
echo

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

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

    # ── Public function usage ──

    echo "### Public Functions with Low Usage (<=2 call sites)"
    echo

    grep -rh '^[[:space:]]*pub fn \|^[[:space:]]*pub(crate) fn \|^[[:space:]]*pub async fn ' "$crate_src" --include="*.rs" 2>/dev/null | \
        sed 's/pub(crate)/pub/' | \
        sed -n 's/.*pub \(async \)\{0,1\}fn \([a-zA-Z_][a-zA-Z0-9_]*\).*/\2/p' | \
        sort | uniq > "$TEMP_DIR/pub_fns.txt"

    low_fn_count=0
    while read fn_name; do
        case "$fn_name" in
            new|default|from|into|clone|fmt|render|update|get|set|build|run|start|stop|init|open|close|read|write|len|is_empty)
                continue ;;
        esac

        # Count usage across ALL crates (not just this one)
        usage_count=0
        for src_dir in "${ALL_SRC_DIRS[@]}"; do
            c=$(grep -r "${fn_name}(" "$src_dir" --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
            usage_count=$((usage_count + c))
        done

        if [ "$usage_count" -le 2 ]; then
            definition=$(grep -rn "pub fn ${fn_name}\|pub(crate) fn ${fn_name}\|pub async fn ${fn_name}" "$crate_src" --include="*.rs" 2>/dev/null | head -1)
            if [ -n "$definition" ]; then
                echo "  ${fn_name}() - $usage_count occurrences"
                echo "    Defined: ${definition#./}"
                echo
                low_fn_count=$((low_fn_count + 1))
            fi
        fi
    done < "$TEMP_DIR/pub_fns.txt"

    echo "Total potentially low-usage functions: $low_fn_count"
    echo

    # ── Type usage ──

    echo "### Types with Low Usage (<=3 occurrences)"
    echo

    grep -rh '^pub struct \|^pub enum \|^pub(crate) struct \|^pub(crate) enum ' "$crate_src" --include="*.rs" 2>/dev/null | \
        sed 's/pub(crate)/pub/' | \
        sed -n 's/.*pub struct \([A-Z][a-zA-Z0-9_]*\).*/\1/p; s/.*pub enum \([A-Z][a-zA-Z0-9_]*\).*/\1/p' | \
        sort | uniq > "$TEMP_DIR/pub_types.txt"

    low_type_count=0
    while read type_name; do
        case "$type_name" in
            Error|Result|Config|State|Options|Builder|Context|Handle|Entry|Item|Node|Event|Action|Kind|Type|Status|Info|Data|Key|Value)
                continue ;;
        esac

        # Count usage across ALL crates
        usage_count=0
        for src_dir in "${ALL_SRC_DIRS[@]}"; do
            c=$(grep -rw "$type_name" "$src_dir" --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
            usage_count=$((usage_count + c))
        done

        if [ "$usage_count" -le 3 ]; then
            definition=$(grep -rn "pub struct ${type_name}\|pub enum ${type_name}" "$crate_src" --include="*.rs" 2>/dev/null | head -1)
            if [ -n "$definition" ]; then
                echo "  $type_name - $usage_count occurrences"
                echo "    Defined: ${definition#./}"
                echo
                low_type_count=$((low_type_count + 1))
            fi
        fi
    done < "$TEMP_DIR/pub_types.txt"

    echo "Total potentially low-usage types: $low_type_count"
    echo

    # ── Module isolation ──

    echo "### Module Isolation (submodules with few external refs)"
    echo

    for module_dir in "$crate_src"/*/; do
        [ -d "$module_dir" ] || continue
        module_name=$(basename "$module_dir")

        pub_items=$(grep -r '^pub ' "$module_dir" --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
        [ "$pub_items" -eq 0 ] && continue

        # External references from within the same crate
        external_refs=$(grep -r "${module_name}::" "$crate_src" --include="*.rs" 2>/dev/null | \
            grep -v "^${module_dir}" | wc -l | tr -d ' ')
        use_refs=$(grep -r "use.*${module_name}" "$crate_src" --include="*.rs" 2>/dev/null | \
            grep -v "^${module_dir}" | wc -l | tr -d ' ')

        total_refs=$((external_refs + use_refs))
        if [ "$total_refs" -lt 5 ]; then
            echo "  $module_name: $pub_items pub items, $total_refs external refs"
        fi
    done

    echo
done

# ── Files with many imports (all crates) ──

echo "=============================================="
echo "## Files with Many Imports (>20 use statements)"
echo "=============================================="
echo

for entry in "${CRATES[@]}"; do
    crate_name="${entry%%:*}"
    crate_src="${entry#*:}"
    [ -d "$crate_src" ] || continue

    find "$crate_src" -name "*.rs" -type f -print0 | while IFS= read -r -d '' file; do
        use_count=$(grep -c '^use \|^    use ' "$file" 2>/dev/null || echo "0")
        if [ "$use_count" -gt 20 ] 2>/dev/null; then
            rel_path="${file#./}"
            echo "  [$crate_name] $rel_path: $use_count use statements"
        fi
    done
done

echo
echo "=============================================="
echo "Note: This is heuristic analysis. Manual review recommended."
echo "=============================================="
