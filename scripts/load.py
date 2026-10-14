#!/usr/bin/env python3
"""Publish bounded demo traffic. Each child owns one key; only our children stop.

Example: python3 scripts/load.py --endpoint tcp/127.0.0.1:7447 --keys 100
Requires a running router. Every key publishes a JSON object at the requested rate.
"""
import argparse
import json
import math
import subprocess
import time


def positive(value):
    n = float(value)
    if not math.isfinite(n) or n <= 0:
        raise argparse.ArgumentTypeError('must be finite and greater than zero')
    return n


def run(args):
    children = []
    deadline = time.monotonic() + args.duration + 10
    try:
        for i in range(args.keys):
            children.append(subprocess.Popen([
                args.zenmon, '-e', args.endpoint, '--connect-timeout', '5s',
                'pub', f'{args.prefix}/{i:04}/value',
                json.dumps({'value': i, 'unit': 'demo'}),
                '--rate', str(args.rate), '--duration', f'{args.duration}s',
            ], stdout=subprocess.DEVNULL))
        while any(child.poll() is None for child in children):
            if any(child.poll() not in (None, 0) for child in children):
                raise RuntimeError('a publisher failed; stopping this run')
            if time.monotonic() >= deadline:
                raise RuntimeError('publishers exceeded the run deadline')
            time.sleep(0.05)
        if any(child.returncode != 0 for child in children):
            raise RuntimeError('a publisher failed')
        return 0
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
        for child in children:
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--zenmon', default='zenmon')
    parser.add_argument('--endpoint', default='tcp/127.0.0.1:7447')
    parser.add_argument('--prefix', default='demo/sensors')
    parser.add_argument('--keys', type=int, default=6)
    parser.add_argument('--rate', type=positive, default=5)
    parser.add_argument('--duration', type=positive, default=30)
    args = parser.parse_args()
    if not 1 <= args.keys <= 256:
        parser.error('--keys must be between 1 and 256 (one process per key)')
    if not args.prefix or any(c in args.prefix for c in '*?#$') or any(
            not part for part in args.prefix.split('/')):
        parser.error('--prefix must be a concrete key without empty segments')
    try:
        return run(args)
    except KeyboardInterrupt:
        return 130
    except (OSError, RuntimeError) as exc:
        parser.exit(1, f'{exc}\n')


if __name__ == '__main__':
    raise SystemExit(main())
