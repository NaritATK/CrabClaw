# CrabClaw Benchmark Suite

This directory contains the baseline and outputs for CrabClaw performance/cost regression checks.

## What is measured

- `cold_start.p95_ms` and `cold_start.avg_ms`
- `ttft.p95_ms` (synthetic first-token latency proxy)
- `provider.fast.p95_ms`, `provider.normal.p95_ms`
- `channel.send.p95_ms`
- `tool.exec.p95_ms`
- `memory.recall.p95_ms`, `memory.recall.avg_ms`
- `cost.per_task_usd` (synthetic reference task)

## Run locally

```bash
bash scripts/benchmark_ci.sh
```

Results are written to `benchmark/results/latest.full.json`.

## Baseline policy

- Baseline file: `benchmark/baseline.json`
- Default allowed regression margin: `+20%`
- Tight margins for key metrics are configured in `scripts/compare_benchmarks.py`
- Hard caps also exist for critical latency goals

When a legitimate optimization or architecture change happens, update baseline with:

```bash
cp benchmark/results/latest.full.json benchmark/baseline.json
```

Then commit baseline update with an explanation in PR notes.
