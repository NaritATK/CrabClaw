#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

mkdir -p benchmark/results

echo "[bench] building release binaries"
cargo build --release --locked --bin crabclaw --bin benchmarks

BENCH_ITERATIONS="${CRABCLAW_BENCH_ITERATIONS:-60}"
if [ "$BENCH_ITERATIONS" -lt 60 ]; then
  COLD_START_SAMPLES=60
else
  COLD_START_SAMPLES="$BENCH_ITERATIONS"
fi

PYTHON_BIN="${PYTHON_BIN:-python3}"

echo "[bench] measuring cold start ($COLD_START_SAMPLES samples)"
"$PYTHON_BIN" - <<PY
import json
import subprocess
import time
from pathlib import Path

samples = int(${COLD_START_SAMPLES})
vals = []
for _ in range(samples):
    t0 = time.perf_counter()
    subprocess.run([
        'target/release/crabclaw',
        '--help'
    ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    vals.append((time.perf_counter() - t0) * 1000.0)

vals_sorted = sorted(vals)
def pct(p):
    i = round((len(vals_sorted)-1)*p)
    return vals_sorted[i]
p50 = pct(0.50)
p90 = pct(0.90)
p95 = pct(0.95)
avg = sum(vals_sorted)/len(vals_sorted)
Path('benchmark/results/cold_start.json').write_text(json.dumps({
    'cold_start.median_ms': p50,
    'cold_start.p90_ms': p90,
    'cold_start.p95_ms': p95,
    'cold_start.avg_ms': avg,
    'samples': len(vals_sorted)
}, indent=2))
Path('benchmark/results/cold_start_samples.json').write_text(json.dumps({
    'raw_samples_ms': {
        'cold_start': vals
    }
}, indent=2))
print(f"[bench] cold_start p90={p90:.2f}ms p95={p95:.2f}ms avg={avg:.2f}ms")
PY

echo "[bench] running synthetic benchmark suite"
target/release/benchmarks --output benchmark/results/latest.json

"$PYTHON_BIN" scripts/merge_benchmark_results.py \
  --main benchmark/results/latest.json \
  --cold benchmark/results/cold_start.json \
  --cold-samples benchmark/results/cold_start_samples.json \
  --out benchmark/results/latest.full.json

if [ -n "${CRABCLAW_BENCH_BASELINE:-}" ]; then
  BASELINE_PATH="$CRABCLAW_BENCH_BASELINE"
else
  OS_NAME="$(uname -s | tr '[:upper:]' '[:lower:]')"
  case "$OS_NAME" in
    linux*)
      BASELINE_PATH="benchmark/baseline.linux.json"
      ;;
    darwin*)
      BASELINE_PATH="benchmark/baseline.macos.json"
      ;;
    msys*|mingw*|cygwin*)
      BASELINE_PATH="benchmark/baseline.windows.json"
      ;;
    *)
      BASELINE_PATH="benchmark/baseline.json"
      ;;
  esac
fi

COMPARE_EXIT=0

if [ -f "$BASELINE_PATH" ]; then
  if ! "$PYTHON_BIN" scripts/compare_benchmarks.py \
    --baseline "$BASELINE_PATH" \
    --current benchmark/results/latest.full.json \
    --summary-out benchmark/results/summary.md; then
    COMPARE_EXIT=$?
  fi
else
  echo "[bench] baseline not found at $BASELINE_PATH; skipping regression gate"
  cat > benchmark/results/summary.md <<'MD'
## ⚠️ Benchmark baseline not found

Regression comparison was skipped for this run because no baseline file was found.
Set baseline via env var `CRABCLAW_BENCH_BASELINE`.
MD
  printf '\nBaseline path: `%s`\n' "$BASELINE_PATH" >> benchmark/results/summary.md
  COMPARE_EXIT=0
fi

echo "[bench] done"
exit "$COMPARE_EXIT"
