#!/usr/bin/env python3
"""Create a deterministic, synthetic capture to explore zenmon without a router.

python3 scripts/demo.py --output demo.ndjson
zenmon tui --trace demo.ndjson
"""
import argparse
import base64
import datetime
import json
import math
from pathlib import Path


def write_demo(path):
    start = datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)
    # Exclusive creation protects existing captures.
    with path.open('x', encoding='utf-8') as stream:
        for tick in range(300):
            for key, payload in [
                ('demo/temperature', {'celsius': round(22 + 3 * math.sin(tick / 30), 2)}),
                ('demo/power', {'watts': round(120 + 20 * math.cos(tick / 20), 1), 'enabled': tick < 240}),
                ('demo/status', {'state': 'warming' if tick < 80 else 'ready', 'sequence': tick}),
            ]:
                raw = json.dumps(payload, separators=(',', ':')).encode()
                record = {
                    'schema_version': 2, 'key_expr': key,
                    'payload_base64': base64.b64encode(raw).decode(),
                    'encoding': 'application/json',
                    'attachment_base64': base64.b64encode(b'{"source":"synthetic-demo"}').decode(),
                    'received_at': (start + datetime.timedelta(milliseconds=tick * 100)).isoformat(timespec='milliseconds').replace('+00:00', 'Z'),
                    'received_offset_ms': tick * 100, 'kind': 'PUT',
                }
                stream.write(json.dumps(record, separators=(',', ':')) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('demo.ndjson'))
    args = parser.parse_args()
    try:
        write_demo(args.output)
    except OSError as exc:
        parser.exit(1, f'{exc}\n')
    print(f'Created 900 synthetic messages: {args.output}')
    print(f'Open with: zenmon tui --trace "{args.output}"')


if __name__ == '__main__':
    main()
