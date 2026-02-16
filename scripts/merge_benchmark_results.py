#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--main', required=True)
    ap.add_argument('--cold', required=True)
    ap.add_argument('--cold-samples')
    ap.add_argument('--out', required=True)
    args = ap.parse_args()

    main_data = json.loads(Path(args.main).read_text())
    cold_data = json.loads(Path(args.cold).read_text())

    metrics = main_data.get('metrics', {})
    metrics.update(cold_data)
    main_data['metrics'] = metrics

    raw = main_data.get('raw_samples_ms', {})
    if args.cold_samples and Path(args.cold_samples).exists():
        cold_raw = json.loads(Path(args.cold_samples).read_text()).get('raw_samples_ms', {})
        raw.update(cold_raw)
    main_data['raw_samples_ms'] = raw

    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(main_data, indent=2))
    print(f"wrote merged benchmark report: {args.out}")
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
