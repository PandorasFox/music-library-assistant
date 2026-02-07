#!/bin/bash
# Code Pockets Analysis for MLA
# Identifies potentially isolated/dead code clusters
#
# Looks for:
# - Functions with no external callers
# - pub items only used within their own module
# - Structs/enums with limited usage

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

echo "=============================================="
echo "MLA Code Pockets Analysis"
echo "Generated: $(date)"
echo "=============================================="
echo
echo "This analysis identifies potentially isolated code that may be"
echo "internally integrated but have limited external usage."
echo

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "## Public Functions Usage Analysis"
echo "==================================="
echo
echo "Finding pub fn declarations and checking their usage across the codebase..."
echo

# Extract all pub fn names and their locations
grep -rh '^[[:space:]]*pub fn \|^[[:space:]]*pub(crate) fn \|^[[:space:]]*pub async fn ' ./src --include="*.rs" 2>/dev/null | \
    sed 's/pub(crate)/pub/' | \
    sed -n 's/.*pub \(async \)\{0,1\}fn \([a-zA-Z_][a-zA-Z0-9_]*\).*/\2/p' | \
    sort | uniq > "$TEMP_DIR/pub_fns.txt"

echo "Found $(wc -l < "$TEMP_DIR/pub_fns.txt" | tr -d ' ') public functions."
echo
echo "### Functions with potentially low usage (<=2 call sites):"
echo

low_usage_count=0
while read fn_name; do
    # Skip very common/generic names
    case "$fn_name" in
        new|default|from|into|clone|fmt|render|update|get|set|build|run|start|stop|init|open|close|read|write|len|is_empty)
            continue
            ;;
    esac

    # Count occurrences
    usage_count=$(grep -r "${fn_name}(" ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')

    if [ "$usage_count" -le 2 ]; then
        definition=$(grep -rn "pub fn ${fn_name}\|pub(crate) fn ${fn_name}\|pub async fn ${fn_name}" ./src --include="*.rs" 2>/dev/null | head -1)
        if [ -n "$definition" ]; then
            echo "  ${fn_name}() - $usage_count occurrences"
            echo "    Defined: ${definition#./src/}"
            echo
            low_usage_count=$((low_usage_count + 1))
        fi
    fi
done < "$TEMP_DIR/pub_fns.txt"

echo "Total potentially low-usage functions: $low_usage_count"
echo

echo "## Struct/Enum Usage Analysis"
echo "=============================="
echo
echo "Finding type definitions with limited usage..."
echo

# Extract pub struct/enum names
grep -rh '^pub struct \|^pub enum \|^pub(crate) struct \|^pub(crate) enum ' ./src --include="*.rs" 2>/dev/null | \
    sed 's/pub(crate)/pub/' | \
    sed -n 's/.*pub struct \([A-Z][a-zA-Z0-9_]*\).*/\1/p; s/.*pub enum \([A-Z][a-zA-Z0-9_]*\).*/\1/p' | \
    sort | uniq > "$TEMP_DIR/pub_types.txt"

echo "Found $(wc -l < "$TEMP_DIR/pub_types.txt" | tr -d ' ') public types."
echo
echo "### Types with potentially low usage (<=3 occurrences):"
echo

low_type_count=0
while read type_name; do
    # Skip common names
    case "$type_name" in
        Error|Result|Config|State|Options|Builder|Context|Handle|Entry|Item|Node|Event|Action|Kind|Type|Status|Info|Data|Key|Value)
            continue
            ;;
    esac

    usage_count=$(grep -rw "$type_name" ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')

    if [ "$usage_count" -le 3 ]; then
        definition=$(grep -rn "pub struct ${type_name}\|pub enum ${type_name}" ./src --include="*.rs" 2>/dev/null | head -1)
        if [ -n "$definition" ]; then
            echo "  $type_name - $usage_count occurrences"
            echo "    Defined: ${definition#./src/}"
            echo
            low_type_count=$((low_type_count + 1))
        fi
    fi
done < "$TEMP_DIR/pub_types.txt"

echo "Total potentially low-usage types: $low_type_count"
echo

echo "## Module-Level Isolation Analysis"
echo "==================================="
echo
echo "Checking for modules with few exports used elsewhere..."
echo

for module_dir in ./src/corpus ./src/ui ./src/witch; do
    if [ -d "$module_dir" ]; then
        module_name=$(basename "$module_dir")
        echo "### Module: $module_name"

        pub_items=$(grep -r '^pub ' "$module_dir" --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
        external_refs=$(grep -r "${module_name}::" ./src --include="*.rs" 2>/dev/null | \
            grep -v "^${module_dir}" | wc -l | tr -d ' ')

        echo "  Public items: $pub_items"
        echo "  External references: $external_refs"
        echo
    fi
done

echo "## Submodule Isolation (within ui/)"
echo "===================================="
echo
echo "Checking UI feature modules for external usage..."
echo

for feature_dir in ./src/ui/*/; do
    if [ -d "$feature_dir" ]; then
        feature_name=$(basename "$feature_dir")

        if [ "$feature_name" = "widgets" ]; then
            continue
        fi

        external_refs=$(grep -r "${feature_name}::" ./src/ui --include="*.rs" 2>/dev/null | \
            grep -v "^${feature_dir}" | wc -l | tr -d ' ')

        use_refs=$(grep -r "use.*${feature_name}" ./src/ui --include="*.rs" 2>/dev/null | \
            grep -v "^${feature_dir}" | wc -l | tr -d ' ')

        total_refs=$((external_refs + use_refs))

        if [ "$total_refs" -lt 5 ]; then
            echo "  $feature_name: $total_refs external references (potential isolation)"
        fi
    fi
done

echo
echo "## Files with Many Imports (potential unused import candidates)"
echo "================================================================"
echo

find ./src -name "*.rs" -type f -print0 | while IFS= read -r -d '' file; do
    use_count=$(grep -c '^use \|^    use ' "$file" 2>/dev/null || echo "0")
    # Ensure it's a valid number
    if [ "$use_count" -gt 20 ] 2>/dev/null; then
        rel_path="${file#./src/}"
        echo "  $rel_path: $use_count use statements"
    fi
done

echo
echo "=============================================="
echo "Note: This is heuristic analysis. Manual review recommended."
echo "=============================================="
