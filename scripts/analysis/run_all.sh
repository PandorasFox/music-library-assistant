#!/bin/bash
# Master Analysis Runner for MM
# Runs all analysis scripts and generates a combined report

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="${1:-$SCRIPT_DIR/../..}"
OUTPUT_DIR="$PROJECT_ROOT/analysis_reports"

mkdir -p "$OUTPUT_DIR"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
REPORT_FILE="$OUTPUT_DIR/analysis_$TIMESTAMP.txt"

echo "Running MM Codebase Analysis..."
echo "Output: $REPORT_FILE"
echo

{
    echo "################################################################"
    echo "#                    MM CODEBASE ANALYSIS                    #"
    echo "#                    $(date)                    #"
    echo "################################################################"
    echo
    echo

    echo "================================================================"
    echo "PART 1: LINES OF CODE ANALYSIS"
    echo "================================================================"
    echo
    bash "$SCRIPT_DIR/loc_analysis.sh" "$PROJECT_ROOT" || echo "[Error in loc_analysis.sh]"
    echo
    echo

    echo "================================================================"
    echo "PART 2: TOKEN/CONTEXT WEIGHT ANALYSIS"
    echo "================================================================"
    echo
    bash "$SCRIPT_DIR/token_weight.sh" "$PROJECT_ROOT" || echo "[Error in token_weight.sh]"
    echo
    echo

    echo "================================================================"
    echo "PART 3: TECHNICAL DEBT (TODOs, STUBS, etc.)"
    echo "================================================================"
    echo
    bash "$SCRIPT_DIR/find_todos.sh" "$PROJECT_ROOT" || echo "[Error in find_todos.sh]"
    echo
    echo

    echo "================================================================"
    echo "PART 4: CODE POCKETS ANALYSIS"
    echo "================================================================"
    echo
    bash "$SCRIPT_DIR/code_pockets.sh" "$PROJECT_ROOT" || echo "[Error in code_pockets.sh]"
    echo
    echo

    echo "################################################################"
    echo "#                      END OF ANALYSIS                        #"
    echo "################################################################"
} > "$REPORT_FILE" 2>&1

echo "Analysis complete!"
echo "Full report: $REPORT_FILE"
echo
echo "Quick summary:"
echo "--------------"
grep -A 5 "## Total Project Statistics" "$REPORT_FILE" | tail -4 || true
echo
grep -A 6 "## Summary Statistics" "$REPORT_FILE" | tail -7 || true
