#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

MODE="${1:-synthetic}"

case "$MODE" in
  synthetic)
    BASELINE_PATH="benchmark/baseline.json"
    ;;
  real)
    BASELINE_PATH="benchmark/baseline.real.json"
    ;;
  *)
    echo "Usage: $0 [synthetic|real]" >&2
    exit 2
    ;;
esac

bash scripts/benchmark_ci.sh

cp benchmark/results/latest.full.json "$BASELINE_PATH"
echo "[bench] baseline refreshed: $BASELINE_PATH"
