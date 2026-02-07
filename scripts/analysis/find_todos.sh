#!/bin/bash
# TODO/Stub/Technical Debt Finder for MLA
# Surfaces incomplete implementations and markers

set -e

PROJECT_ROOT="${1:-$(dirname "$0")/../..}"
cd "$PROJECT_ROOT"

echo "=============================================="
echo "MLA Technical Debt Analysis"
echo "Generated: $(date)"
echo "=============================================="

echo
echo "## todo!() Macro Usage"
echo "======================"
echo "These are intentional panic points marking incomplete implementations."
echo

grep -rn 'todo!(' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## unimplemented!() Macro Usage"
echo "==============================="
echo

grep -rn 'unimplemented!(' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## TODO Comments"
echo "================"
echo

grep -rn '// TODO\|// todo\|//TODO\|//todo\|# TODO\|# todo' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## FIXME Comments"
echo "================="
echo

grep -rn '// FIXME\|// fixme\|//FIXME\|//fixme' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## HACK Comments"
echo "================"
echo

grep -rn '// HACK\|// hack\|//HACK\|//hack' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## #[allow(dead_code)] Annotations"
echo "==================================="
echo "Per CLAUDE.md: These should NEVER exist. Each is a policy violation."
echo

grep -rn '#\[allow(dead_code)\]' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found - good!)"

echo
echo "## #[allow(unused_*)] Annotations"
echo "=================================="
echo "These may indicate dead code or incomplete integration."
echo

grep -rn '#\[allow(unused' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## Unreachable Patterns (potential dead code)"
echo "=============================================="
echo

grep -rn 'unreachable!(' ./src --include="*.rs" 2>/dev/null | while IFS=: read file line content; do
    rel_path="${file#./src/}"
    printf "%s:%s\n  %s\n\n" "$rel_path" "$line" "$(echo "$content" | sed 's/^[[:space:]]*//')"
done || echo "(none found)"

echo
echo "## Summary Statistics"
echo "====================="
echo

todo_count=$(grep -r 'todo!(' ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
unimpl_count=$(grep -r 'unimplemented!(' ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
todo_comment=$(grep -r '// TODO\|// todo\|//TODO\|//todo' ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
fixme_count=$(grep -r '// FIXME\|// fixme\|//FIXME\|//fixme' ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
allow_dead=$(grep -r '#\[allow(dead_code)\]' ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')
allow_unused=$(grep -r '#\[allow(unused' ./src --include="*.rs" 2>/dev/null | wc -l | tr -d ' ')

echo "todo!() macros:           $todo_count"
echo "unimplemented!() macros:  $unimpl_count"
echo "TODO comments:            $todo_comment"
echo "FIXME comments:           $fixme_count"
echo "#[allow(dead_code)]:      $allow_dead (should be 0!)"
echo "#[allow(unused_*)]:       $allow_unused"
echo
echo "Total technical debt markers: $((todo_count + unimpl_count + todo_comment + fixme_count))"
