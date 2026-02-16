#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

mkdir -p benchmark/results

echo "[bench] building release binaries"
cargo build --release --locked --bin crabclaw --bin benchmarks

COLD_START_SAMPLES=20

echo "[bench] measuring cold start ($COLD_START_SAMPLES samples)"
python3 - <<'PY'
import json
import subprocess
import time
from pathlib import Path

samples = 20
vals = []
for _ in range(samples):
    t0 = time.perf_counter()
    subprocess.run([
        'target/release/crabclaw',
        '--help'
    ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    vals.append((time.perf_counter() - t0) * 1000.0)

vals_sorted = sorted(vals)
idx = round((len(vals_sorted)-1)*0.95)
p95 = vals_sorted[idx]
avg = sum(vals_sorted)/len(vals_sorted)
Path('benchmark/results/cold_start.json').write_text(json.dumps({
    'cold_start.p95_ms': p95,
    'cold_start.avg_ms': avg,
    'samples': len(vals_sorted)
}, indent=2))
print(f"[bench] cold_start p95={p95:.2f}ms avg={avg:.2f}ms")
PY

echo "[bench] running synthetic benchmark suite"
target/release/benchmarks --output benchmark/results/latest.json

python3 scripts/merge_benchmark_results.py \
  --main benchmark/results/latest.json \
  --cold benchmark/results/cold_start.json \
  --out benchmark/results/latest.full.json

python3 scripts/compare_benchmarks.py \
  --baseline benchmark/baseline.json \
  --current benchmark/results/latest.full.json \
  --summary-out benchmark/results/summary.md

echo "[bench] done"
