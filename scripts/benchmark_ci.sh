#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

mkdir -p benchmark/results

PYTHON_BIN="${PYTHON_BIN:-python3}"

bench_log_header() {
  echo
  echo "============================================================"
  echo "[bench] $1"
  echo "============================================================"
}

resolve_cold_start_samples() {
  local bench_iterations
  bench_iterations="${CRABCLAW_BENCH_ITERATIONS:-60}"
  if [ "$bench_iterations" -lt 60 ]; then
    echo 60
  else
    echo "$bench_iterations"
  fi
}

resolve_baseline_path() {
  if [ -n "${CRABCLAW_BENCH_BASELINE:-}" ]; then
    echo "$CRABCLAW_BENCH_BASELINE"
    return
  fi

  local os_name
  os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"

  case "$os_name" in
    linux*)
      echo "benchmark/baseline.linux.json"
      ;;
    darwin*)
      echo "benchmark/baseline.macos.json"
      ;;
    msys* | mingw* | cygwin*)
      echo "benchmark/baseline.windows.json"
      ;;
    *)
      echo "benchmark/baseline.json"
      ;;
  esac
}

resolve_bench_strict() {
  local strict
  strict="${CRABCLAW_BENCH_STRICT:-}"
  if [ -n "$strict" ]; then
    echo "$strict"
    return
  fi

  if [ -n "${CI:-}" ]; then
    echo 1
  else
    echo 0
  fi
}

measure_cold_start() {
  local samples="$1"

  bench_log_header "measuring cold start (${samples} samples)"

  "$PYTHON_BIN" - <<PY
import json
import subprocess
import time
from pathlib import Path

samples = int(${samples})
vals = []
for _ in range(samples):
    t0 = time.perf_counter()
    subprocess.run(
        [
            "target/release/crabclaw",
            "--help",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=True,
    )
    vals.append((time.perf_counter() - t0) * 1000.0)

vals_sorted = sorted(vals)

def pct(p):
    i = round((len(vals_sorted) - 1) * p)
    return vals_sorted[i]

p50 = pct(0.50)
p90 = pct(0.90)
p95 = pct(0.95)
avg = sum(vals_sorted) / len(vals_sorted)

Path("benchmark/results/cold_start.json").write_text(
    json.dumps(
        {
            "cold_start.median_ms": p50,
            "cold_start.p90_ms": p90,
            "cold_start.p95_ms": p95,
            "cold_start.avg_ms": avg,
            "samples": len(vals_sorted),
        },
        indent=2,
    )
)

Path("benchmark/results/cold_start_samples.json").write_text(
    json.dumps(
        {
            "raw_samples_ms": {
                "cold_start": vals,
            }
        },
        indent=2,
    )
)

print(f"[bench] cold_start p90={p90:.2f}ms p95={p95:.2f}ms avg={avg:.2f}ms")
PY
}

run_compare() {
  local baseline_path="$1"
  local bench_strict="$2"

  COMPARE_EXIT=0

  if [ -f "$baseline_path" ]; then
    if [ "$bench_strict" = "1" ]; then
      if ! "$PYTHON_BIN" scripts/compare_benchmarks.py \
        --baseline "$baseline_path" \
        --current benchmark/results/latest.full.json \
        --summary-out benchmark/results/summary.md \
        --strict; then
        COMPARE_EXIT=$?
      fi
    else
      if ! "$PYTHON_BIN" scripts/compare_benchmarks.py \
        --baseline "$baseline_path" \
        --current benchmark/results/latest.full.json \
        --summary-out benchmark/results/summary.md; then
        COMPARE_EXIT=$?
      fi
    fi
    return
  fi

  if [ "$bench_strict" = "1" ]; then
    echo "[bench][ERROR] baseline not found at $baseline_path (strict mode)"
    cat >benchmark/results/summary.md <<'MD'
## ❌ Benchmark baseline missing (strict mode)

Regression comparison failed because no baseline file was found.
Set baseline via env var `CRABCLAW_BENCH_BASELINE` or add the expected baseline file.
MD
    printf '\nBaseline path: `%s`\n' "$baseline_path" >>benchmark/results/summary.md
    COMPARE_EXIT=1
    return
  fi

  echo "[bench] baseline not found at $baseline_path; skipping regression gate"
  cat >benchmark/results/summary.md <<'MD'
## ⚠️ Benchmark baseline not found

Regression comparison was skipped for this run because no baseline file was found.
Set baseline via env var `CRABCLAW_BENCH_BASELINE`.
MD
  printf '\nBaseline path: `%s`\n' "$baseline_path" >>benchmark/results/summary.md
  COMPARE_EXIT=0
}

bench_log_header "building release binaries"
cargo build --release --locked --bin crabclaw --bin benchmarks

COLD_START_SAMPLES="$(resolve_cold_start_samples)"
measure_cold_start "$COLD_START_SAMPLES"

bench_log_header "running synthetic benchmark suite"
CRABCLAW_BENCH_MODE=synthetic target/release/benchmarks --output benchmark/results/latest.json

"$PYTHON_BIN" scripts/merge_benchmark_results.py \
  --main benchmark/results/latest.json \
  --cold benchmark/results/cold_start.json \
  --cold-samples benchmark/results/cold_start_samples.json \
  --out benchmark/results/latest.full.json

BASELINE_PATH="$(resolve_baseline_path)"
BENCH_STRICT="$(resolve_bench_strict)"
run_compare "$BASELINE_PATH" "$BENCH_STRICT"

echo "[bench] done"
exit "$COMPARE_EXIT"
