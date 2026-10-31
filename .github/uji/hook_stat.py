#!/usr/bin/env python3
"""How much of the backtest sits in pools with a hook: hook_stat.py <pools.csv> <trades.jsonl>"""
import collections
import csv
import json
import sys

hook = {}
for r in csv.reader(open(sys.argv[1])):
    if r and r[0] != "pool_id":
        hook[r[0].lower()] = r[5].lower()
t = [json.loads(x) for x in open(sys.argv[2]) if x.startswith("{")]
akhir = 100 + sum(x["kembali"] - x["pos"] for x in t)
per = collections.defaultdict(lambda: [0, 0.0, 0.0])
for x in t:
    h = hook.get(x["pool"].lower(), "?")
    per[h][0] += 1
    per[h][1] += x["pos"]
    per[h][2] += x["kembali"]
nol = "0x" + "0" * 40
print("pool", len(hook), "ber-hook", sum(1 for v in hook.values() if v != nol), "| trade", len(t), "equity", round(akhir, 2))
for h, (n, p, k) in sorted(per.items(), key=lambda v: -v[1][0])[:15]:
    print(h[:12], "trade", n, "modal", round(p), "kembali", round(k), "untung", round(k - p))
bh = [v for h, v in per.items() if h != nol]
n, k = sum(v[0] for v in bh), sum(v[2] for v in bh)
print(f"BER-HOOK: {n} trade ({100 * n / len(t):.1f}%), kembali ${k:,.0f}; kalau SEMUA tidak bisa dijual, equity kasar ${akhir - k:,.0f} (ukuran beli tidak dihitung ulang)")
h1 = [v for h, v in per.items() if h.startswith("0xd200f692")]
print("hook 0xd200f692:", h1)
