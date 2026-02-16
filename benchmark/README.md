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

- Synthetic baseline file: `benchmark/baseline.json`
- Optional real-mode baseline file: `benchmark/baseline.real.json`
- Default allowed regression margin: `+20%`
- Tight margins for key metrics are configured in `scripts/compare_benchmarks.py`
- Hard caps also exist for critical latency goals

When a legitimate optimization or architecture change happens, update baseline with:

```bash
cp benchmark/results/latest.full.json benchmark/baseline.json
```

For real-mode runs:

```bash
CRABCLAW_BENCH_MODE=real \
CRABCLAW_BENCH_BASELINE=benchmark/baseline.real.json \
bash scripts/benchmark_ci.sh
```

If baseline file does not exist, the script still produces results + summary and skips regression gate.

### Real mode env vars

- `CRABCLAW_BENCH_MODE=real`
- `CRABCLAW_BENCH_REAL_PROVIDER_URL`
- `CRABCLAW_BENCH_REAL_PROVIDER_API_KEY`
- `CRABCLAW_BENCH_REAL_PROVIDER_MODEL` (optional, default `gpt-4o-mini`)
- `CRABCLAW_BENCH_REAL_CHANNEL_WEBHOOK_URL` (optional)
- `CRABCLAW_BENCH_REAL_TOOL_COMMAND` (optional)

Then commit baseline update with an explanation in PR notes.
