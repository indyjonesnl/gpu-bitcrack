#!/usr/bin/env bash
# bench/sweep.sh — batch-size sweep & summary for gpu-keyspace-search
# Requires: hyperfine, jq, bc
#
# Examples:
#   bench/sweep.sh --mode miss
#   bench/sweep.sh --mode hit --runs 7 --warmup 2 --batches "262144 524288 750000 1000000"
#   bench/sweep.sh --mode custom --range 01000000:013fffff --target 1FeexV6bAHb8ybZjqQMjJrcCrHGW9sb6uF
#
set -euo pipefail

# Defaults
MODE="miss"                               # miss|hit|custom
RANGE_MISS="100000:2fffff"
TARGET_MISS="1FeexV6bAHb8ybZjqQMjJrcCrHGW9sb6uF"
RANGE_HIT="200000:3fffff"
TARGET_HIT="1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv"
BATCHES="550000 60000 650000 700000 750000 800000"
RUNS=7
WARMUP=2
OUT_DIR="bench/out/$(date +%Y%m%d-%H%M%S)"
BIN="./target/release/gpu-bitcrack"
BUILD=0

# Parse args
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) MODE="$2"; shift 2;;
    --range) RANGE_CUSTOM="$2"; shift 2;;
    --target) TARGET_CUSTOM="$2"; shift 2;;
    --batches) BATCHES="$2"; shift 2;;
    --runs) RUNS="$2"; shift 2;;
    --warmup) WARMUP="$2"; shift 2;;
    --out) OUT_DIR="$2"; shift 2;;
    --bin) BIN="$2"; shift 2;;
    --build) BUILD=1; shift;;
    -h|--help)
      cat <<EOF2
Usage: bench/sweep.sh [options]

Options:
  --mode <miss|hit|custom>   Which preset to use (default: miss)
  --range <START:END-hex>    Custom range when --mode custom
  --target <P2PKH>           Custom target address when --mode custom
  --batches "<list>"         Batch sizes to test (default: "$BATCHES")
  --runs <N>                 Hyperfine repetitions (default: $RUNS)
  --warmup <N>               Hyperfine warmup runs (default: $WARMUP)
  --out <dir>                Output directory (default: $OUT_DIR)
  --bin <path>               Path to binary (default: $BIN)
  --build                    Build the binary with cargo (release)
  -h, --help                 Show this help

Examples:
  bench/sweep.sh --mode hit --batches "262144 524288 1000000"
  bench/sweep.sh --mode custom --range 01000000:013fffff --target 1AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
EOF2
      exit 0;;
    *) echo "Unknown option: $1" >&2; exit 1;;
  esac
done

# Tool checks
for t in hyperfine jq bc; do
  command -v "$t" >/dev/null 2>&1 || { echo "$t not found. Please install it."; exit 1; }
done

# Resolve range/target
if [[ "$MODE" == "miss" ]]; then
  RANGE="$RANGE_MISS"; TARGET="$TARGET_MISS"
elif [[ "$MODE" == "hit" ]]; then
  RANGE="$RANGE_HIT"; TARGET="$TARGET_HIT"
elif [[ "$MODE" == "custom" ]]; then
  RANGE="${RANGE_CUSTOM:-}"; TARGET="${TARGET_CUSTOM:-}"
  if [[ -z "$RANGE" || -z "$TARGET" ]]; then
    echo "--mode custom requires --range and --target" >&2; exit 1
  fi
else
  echo "Invalid --mode: $MODE (expected miss|hit|custom)"; exit 1
fi

# Optionally build
if [[ $BUILD -eq 1 ]]; then
  echo "Building release binary..."
  cargo build --release
fi

# Ensure output dir
mkdir -p "$OUT_DIR"

# Compute inclusive range size as integer (supports odd-length hex by padding left)
hex_to_dec() {
  local h="$1"
  h="${h#0x}"; h="${h#0X}"
  (( ${#h} % 2 == 1 )) && h="0$h"
  [[ -z "$h" ]] && h="0"
  printf "ibase=16;%s\n" "$(echo "$h" | tr '[:lower:]' '[:upper:]')" | bc
}

START_HEX="${RANGE%%:*}"
END_HEX="${RANGE##*:}"
START_DEC=$(hex_to_dec "$START_HEX")
END_DEC=$(hex_to_dec "$END_HEX")
SIZE_DEC=$(echo "$END_DEC - $START_DEC + 1" | bc)  # inclusive

# Save environment snapshot
{
  echo "Commit: $(git rev-parse --short HEAD 2>/dev/null || echo 'N/A')"
  echo "Date:   $(date -Iseconds)"
  echo "Mode:   $MODE"
  echo "Range:  $RANGE  (size=$SIZE_DEC)"
  echo "Target: $TARGET"
  echo "Batches: $BATCHES"
  echo "Binary: $BIN"
  echo "Rust:   $(rustc --version 2>/dev/null || echo 'N/A')"
  echo "Cargo:  $(cargo --version 2>/dev/null || echo 'N/A')"
} | tee "$OUT_DIR/env.txt"

echo "# Summary (range size: $SIZE_DEC keys)" | tee "$OUT_DIR/summary.md"
echo "" | tee -a "$OUT_DIR/summary.md"
echo "| Batch | Mean (s) | Stddev (s) | Keys/sec | Command |" | tee -a "$OUT_DIR/summary.md"
echo "|------:|---------:|-----------:|---------:|:--------|" | tee -a "$OUT_DIR/summary.md"

# Run sweep
for B in $BATCHES; do
  JSON="$OUT_DIR/bench_batch_${B}.json"
  CMD="$BIN $RANGE $TARGET --batch $B"

  echo "==> Batch=$B"
  echo "Command: $CMD"
  hyperfine -w "$WARMUP" -r "$RUNS" --export-json "$JSON" "$CMD"

  MEAN=$(jq -r '.results[0].mean' "$JSON")
  STD=$(jq -r '.results[0].stddev' "$JSON")
  KPS=$(awk -v size="$SIZE_DEC" -v mean="$MEAN" 'BEGIN { if (mean>0) printf "%.0f", size/mean; else print "NaN"; }')

  printf "| %6d | %8.3f | %10.3f | %8s | \`%s\` |\n" "$B" "$MEAN" "$STD" "$KPS" "$CMD" | tee -a "$OUT_DIR/summary.md"
done

echo ""
echo "Done. Outputs in: $OUT_DIR"
echo "Open $OUT_DIR/summary.md for a Markdown summary."
