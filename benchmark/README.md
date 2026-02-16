# CrabClaw Benchmark Suite

This directory contains baseline files and outputs for CrabClaw performance/cost regression checks.

## What is measured

CrabClaw records latency as **median / p90 / p95** and stores raw samples for debugging.

| Area | Metrics |
|---|---|
| Cold start | `cold_start.median_ms`, `cold_start.p90_ms`, `cold_start.p95_ms`, `cold_start.avg_ms` |
| TTFT proxy | `ttft.median_ms`, `ttft.p90_ms`, `ttft.p95_ms` |
| Provider latency | `provider.fast.*`, `provider.normal.*` |
| Channel latency | `channel.send.*` |
| Tool latency | `tool.exec.*` |
| Memory recall latency/quality | `memory.recall.*`, `memory.recall.hit_at_k`, `memory.recall.precision_proxy` |
| Cost (synthetic reference task) | `cost.per_task_usd`, `cost.input_tokens`, `cost.output_tokens`, `cost.input_rate_per_m`, `cost.output_rate_per_m` |
| Real/synthetic mode flags | `bench.mode.real`, `bench.real_provider_used`, `bench.real_channel_used`, `bench.real_tool_used` |
| Provider reliability diagnostics | `provider.retry_count`, `provider.timeout_rate`, `provider.cache.hit_rate`, `provider.circuit.reject_rate`, `provider.coalesced_wait_count`, `provider.hedge_launch_count`, `provider.hedge_win_count` |
| Circuit breaker diagnostics | `circuitbreaker.state`, `circuitbreaker.open_count`, `circuitbreaker.reject_count`, `circuitbreaker.half_open_count`, `circuitbreaker.close_count` |
| Circuit aliases | `cb.state`, `cb.open_count`, `cb.reject_count` |
| Cache diagnostics | `cache.response.hit_rate` |
| Real HTTP breakdown (when real provider URL is set) | `http.dns_ms`, `http.connect_ms`, `http.ttfb_ms` |

> `circuitbreaker.state`: `0 = closed`, `1 = open`

## Run locally

```bash
bash scripts/benchmark_ci.sh
```

- Full output: `benchmark/results/latest.full.json`
- Raw samples: `raw_samples_ms` inside `latest.full.json`

## Baseline policy

- Default synthetic baseline: `benchmark/baseline.json`
- OS-specific baselines:
  - `benchmark/baseline.linux.json`
  - `benchmark/baseline.macos.json`
  - `benchmark/baseline.windows.json`
- Optional real baseline: `benchmark/baseline.real.json`
- Default regression margin: `+20%`
- Tighter per-metric margins and hard limits: `scripts/compare_benchmarks.py`

Update baseline after legitimate architecture/performance changes:

```bash
cp benchmark/results/latest.full.json benchmark/baseline.json
```

Run with real-mode baseline:

```bash
CRABCLAW_BENCH_MODE=real \
CRABCLAW_BENCH_BASELINE=benchmark/baseline.real.json \
bash scripts/benchmark_ci.sh
```

Baseline selection order:

1. `CRABCLAW_BENCH_BASELINE` (if set)
2. OS-specific baseline (`baseline.linux/macos/windows.json`)
3. Fallback `benchmark/baseline.json`

If the selected baseline file is missing:

- Strict mode (`CRABCLAW_BENCH_STRICT=1`, CI default) → **fail gate**
- Non-strict mode (`CRABCLAW_BENCH_STRICT=0`, local default) → output + summary, skip regression compare

## Real mode env vars

- `CRABCLAW_BENCH_MODE=real`
- `CRABCLAW_BENCH_REAL_PROVIDER_URL`
- `CRABCLAW_BENCH_REAL_PROVIDER_API_KEY`
- `CRABCLAW_BENCH_REAL_PROVIDER_MODEL` (optional, default `gpt-4o-mini`)
- `CRABCLAW_BENCH_REAL_CHANNEL_WEBHOOK_URL` (optional)
- `CRABCLAW_BENCH_REAL_TOOL_COMMAND` (optional)
- `CRABCLAW_BENCH_REAL_REQUIRED=true` (optional, fail-fast if real dependencies are missing)

## Refresh baseline automation

- Local helper:

```bash
scripts/refresh_benchmark_baseline.sh [synthetic|real]
```

- GitHub Actions:
  - **Benchmark Baseline Refresh** (`workflow_dispatch`)
    - supports `mode=synthetic|real`
    - can auto-open a PR with updated baseline
  - **Benchmark Matrix (Cross-OS)** (`workflow_dispatch`)
    - runs benchmark gate on `ubuntu`, `macos`, `windows`
    - uses OS-specific synthetic baselines automatically

Commit baseline changes with context in PR notes.
