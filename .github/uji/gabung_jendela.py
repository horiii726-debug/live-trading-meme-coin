#!/usr/bin/env python3
"""Merge consecutive windows into ONE dataset so the bot runs as a single continuous account (no boundary resets).
Same as the research repo's research/merge_windows.py, with the data directory as an argument.

    gabung_jendela.py <data dir> <out name> <window> [<window> ...]

Each pool lives in exactly one window (its birth window, with its full tail), so concatenation is lossless; the
ETH/USDG price pool appears in every window and its duplicates only repeat identical price points.
"""
import json
import shutil
import sys
from pathlib import Path


def main():
    data, out, wins = Path(sys.argv[1]), Path(sys.argv[1]) / sys.argv[2], sys.argv[3:]
    out.mkdir(exist_ok=True)
    for name in ("pools.csv", "swaps_sorted.csv", "liq.csv"):
        with open(out / name, "wb") as fo:
            for i, w in enumerate(wins):
                with open(data / w / name, "rb") as fi:
                    head = fi.readline()
                    if i == 0:
                        fo.write(head)
                    shutil.copyfileobj(fi, fo, 1 << 24)
    with open(out / "trades.jsonl", "wb") as fo:
        for w in wins:
            with open(data / w / "trades.jsonl", "rb") as fi:
                shutil.copyfileobj(fi, fo, 1 << 24)
    for name in ("swaps_m1.json", "transfers.json", "dev_creator.json"):
        merged = {}
        for w in wins:
            d = json.loads((data / w / name).read_text())
            dup = set(d) & set(merged)
            if dup:
                sys.exit(f"{name}: pool in two windows ({len(dup)}), refusing to merge")
            merged.update(d)
        (out / name).write_text(json.dumps(merged))
    print(f"merged {len(wins)} windows into {out}")


if __name__ == "__main__":
    main()
