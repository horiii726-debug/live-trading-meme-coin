#!/usr/bin/env python3
"""Profit report of `rhc-trader sim` runs on recent chain data (CI, read-only).

    laporan_untung.py <rentang.json> <name>=<summary.txt>:<trades.jsonl> ...

rentang.json: {"from": block, "to": block, "t_from": unix, "t_to": unix}. Every trade line is
{"pool","pos","kembali","ret","alasan","masuk","keluar"} in USD of the strategy (paper) account.
"""
import json
import sys
from datetime import datetime, timezone

r = json.load(open(sys.argv[1]))
bps = (r["to"] - r["from"]) / max(1, r["t_to"] - r["t_from"])  # blocks per second


def jam(blk):
    t = r["t_from"] + (blk - r["from"]) / bps
    return datetime.fromtimestamp(t, timezone.utc).strftime("%d %b %H:%M")


print("=" * 86)
print(f"DATA BARU (tidak pernah dilihat backtest): {jam(r['from'])} s/d {jam(r['to'])} UTC, blok {r['from']}..{r['to']}")
print("=" * 86)
for arg in sys.argv[2:]:
    name, files = arg.split("=", 1)
    summ, trades_p = files.split(":")
    lines = open(summ, encoding="utf-8", errors="replace").read().splitlines()
    head = [l for l in lines if l.startswith("SIM ")]
    tolak = [l for l in lines if l.startswith("tolak:")]
    tr = [json.loads(x) for x in open(trades_p, encoding="utf-8") if x.startswith("{")]
    print(f"\n### {name}")
    print("   " + (head[-1].split(" | ", 1)[-1] if head else "(tidak ada ringkasan)"))
    if tolak:
        print("   " + tolak[-1][:300])
    if not tr:
        print("   tidak ada trade")
        continue
    menang = [t for t in tr if t["kembali"] > t["pos"]]
    untung = sum(t["kembali"] - t["pos"] for t in tr)
    print(f"   trade: {len(tr)} | menang: {len(menang)} ({100 * len(menang) / len(tr):.0f}%) | untung bersih akun strategi: ${untung:+.2f}")
    rets = sorted(t["kembali"] / t["pos"] - 1 for t in tr if t["pos"] > 0)
    print(f"   hasil per trade: terburuk {100 * rets[0]:+.0f}% | median {100 * rets[len(rets) // 2]:+.0f}% | terbaik {100 * rets[-1]:+.0f}%")
    print(f"   {'masuk (UTC)':<14}{'lama':>7}  {'hasil':>7}  {'ukuran':>9}  alasan keluar")
    for t in tr:
        lama = (t["keluar"] - t["masuk"]) / bps / 60
        print(f"   {jam(t['masuk']):<14}{lama:>5.0f}mn  {100 * (t['kembali'] / t['pos'] - 1):>+6.0f}%  ${t['pos']:>8.2f}  {t['alasan']}")
