#!/usr/bin/env python3
import argparse
import json
from pathlib import Path

# Allowed regression margin per metric (fractional, e.g. 0.20 = +20%)
DEFAULT_MARGIN = 0.20
MARGINS = {
    'cold_start.p95_ms': 0.25,
    'cold_start.p90_ms': 0.25,
    'cold_start.avg_ms': 0.25,
    'cost.per_task_usd': 0.15,
}

# Absolute floor (ms) for latency regressions to reduce flake/noise.
ABSOLUTE_FLOOR_MS = float(
    __import__("os").environ.get("CRABCLAW_BENCH_ABS_FLOOR_MS", "5.0")
)

# Hard upper bounds to protect key goals.
HARD_LIMITS = {
    'cold_start.p90_ms': 120.0,
    'cold_start.p95_ms': 120.0,
    'provider.fast.p90_ms': 120.0,
    'ttft.p95_ms': 120.0,
    'memory.recall.p95_ms': 80.0,
}


def load_metrics(path: str):
    data = json.loads(Path(path).read_text())
    return data.get('metrics', {})


def fmt(v):
    return f"{v:.4f}" if isinstance(v, float) else str(v)


def should_gate_metric(key: str) -> bool:
    if key == 'cost.per_task_usd':
        return True
    # CI gate uses median + p90; p95 is for reporting/visibility.
    if key.endswith('.median_ms') or key.endswith('.p90_ms'):
        return True
    if key in ('cold_start.avg_ms', 'cold_start.p90_ms', 'cold_start.median_ms'):
        return True
    return False


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--baseline', required=True)
    ap.add_argument('--current', required=True)
    ap.add_argument('--summary-out')
    ap.add_argument(
        "--strict",
        action="store_true",
        help="Fail when a gated baseline metric is missing in current output",
    )
    args = ap.parse_args()

    base = load_metrics(args.baseline)
    cur = load_metrics(args.current)

    failed = False
    rows = []
    missing_gated_metrics = []

    print('[bench] comparing current against baseline')
    for key, base_v in sorted(base.items()):
        if key == 'samples':
            continue
        if key not in cur:
            gated = should_gate_metric(key)
            level = 'ERROR' if (args.strict and gated) else 'WARN'
            print(f"[bench][{level}] missing metric in current report: {key}")
            rows.append((key, base_v, None, None, None, 'MISSING', gated))
            if args.strict and gated:
                failed = True
                missing_gated_metrics.append(key)
            continue
        cur_v = cur[key]
        if not isinstance(base_v, (int, float)) or not isinstance(cur_v, (int, float)):
            continue

        margin = MARGINS.get(key, DEFAULT_MARGIN)
        allowed = base_v * (1.0 + margin)
        delta = cur_v - base_v

        status = 'OK'
        if should_gate_metric(key):
            if key.endswith('_ms'):
                if cur_v > allowed and delta > ABSOLUTE_FLOOR_MS:
                    status = 'REGRESSION'
                    failed = True
            else:
                if cur_v > allowed:
                    status = 'REGRESSION'
                    failed = True

        print(
            f"[bench] {key}: baseline={fmt(base_v)} current={fmt(cur_v)} "
            f"allowed<={fmt(allowed)} delta={fmt(delta)} => {status}"
        )
        rows.append(
            (key, base_v, cur_v, allowed, delta, status, should_gate_metric(key))
        )

    hard_fail_rows = []
    for key, limit in HARD_LIMITS.items():
        if key in cur and isinstance(cur[key], (int, float)) and cur[key] > limit:
            print(
                f"[bench][HARD-FAIL] {key}={fmt(cur[key])} exceeds hard limit {fmt(limit)}"
            )
            failed = True
            hard_fail_rows.append((key, cur[key], limit))

    if args.summary_out:
        status_emoji = '❌' if failed else '✅'
        lines = [
            f"## {status_emoji} Benchmark Gate {'Failed' if failed else 'Passed'}",
            '',
            f"Absolute floor for latency regressions: `{ABSOLUTE_FLOOR_MS}ms`",
            '',
            '| Metric | Baseline | Current | Allowed | Delta | Gate? | Status |',
            '|---|---:|---:|---:|---:|---|---|',
        ]
        for key, base_v, cur_v, allowed, delta, status, gated in rows:
            status_cell = '❌ REGRESSION' if status != 'OK' else '✅ OK'
            gate_cell = 'yes' if gated else 'no (report-only)'
            lines.append(
                f"| `{key}` | {fmt(base_v)} | {fmt(cur_v)} | <= {fmt(allowed)} | {fmt(delta)} | {gate_cell} | {status_cell} |"
            )

        if missing_gated_metrics:
            lines.extend(['', '### Missing Gated Metrics (strict mode)'])
            for key in missing_gated_metrics:
                lines.append(f"- `{key}` missing in current benchmark output")

        if hard_fail_rows:
            lines.extend(['', '### Hard Limit Violations'])
            for key, current, limit in hard_fail_rows:
                lines.append(f"- `{key}` = {fmt(current)} (limit {fmt(limit)})")

        Path(args.summary_out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.summary_out).write_text('\n'.join(lines) + '\n')

    if failed:
        print('[bench] benchmark gate failed')
        return 1

    print('[bench] benchmark gate passed')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
