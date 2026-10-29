#!/usr/bin/env python3
"""Would the shadow orders really have filled, against the other bots on the chain?

    realita.py <bot dir> <sampler.txt> > realita.json

Shadow mode builds and signs every order but never sends it, so nothing tells us whether a real one would have landed
in time. The bot stores every swap on the pool (store/swaps.csv), all of them from other traders since we never
traded. For each order: the price the bot saw when it decided (the block it had processed then), against the price in
the block the transaction would land in (chain head when the executor got the order + k blocks of network latency).
Everyone who swapped in between got there first. A buy reverts when the price ran away more than the 10% buy slippage;
a sell's first try allows 15% (later tries 50%, 100%, so a sell always goes through eventually, at a worse price).

sampler.txt: one line every second while the bot runs: <unix ms> <last processed block> <lag blocks>.
"""
import bisect
import json
import re
import statistics
import sys
from pathlib import Path

BOT, SAMP = Path(sys.argv[1]), sys.argv[2]
L = BOT / "rust_live"
ETH = "0x" + "0" * 40
BUY_TOL, SELL_TOL = 0.10, 0.15
LAT = {"VPS cepat (~0,2 dtk)": 2, "VPS biasa (~0,5 dtk)": 5, "lambat (~1 dtk)": 10}

ms_s, blk_s, head_s = [], [], []
for ln in open(SAMP):
    p = ln.split()
    if len(p) == 3:
        try:
            m, b, g = int(p[0]), float(p[1]), float(p[2])
        except ValueError:
            continue
        if b > 0 and (not blk_s or b >= blk_s[-1]):
            ms_s.append(m); blk_s.append(b); head_s.append(b + max(g, 0))


def at(ms, ys):
    i = bisect.bisect_left(ms_s, ms)
    if i <= 0:
        return ys[0]
    if i >= len(ms_s):
        return ys[-1]
    a, b = ms_s[i - 1], ms_s[i]
    return ys[i - 1] + (ys[i] - ys[i - 1]) * (ms - a) / max(b - a, 1)


eth0 = {}
for ln in open(L / "store" / "pools.csv"):
    p = ln.strip().split(",")
    if len(p) >= 2 and p[0].startswith("0x"):
        eth0[p[0].lower()] = p[1].lower() == ETH
sw = {}
for ln in open(L / "store" / "swaps.csv"):
    p = ln.strip().split(",")
    if len(p) >= 7 and p[0].startswith("0x"):
        try:
            sw.setdefault(p[0].lower(), []).append((float(p[1]), int(p[2]), float(p[6])))
        except ValueError:
            pass
for v in sw.values():
    v.sort()


def tok_per_eth(pool, blk):
    """price in the pool after every swap up to and including block blk (None before the first swap)"""
    v = sw.get(pool, [])
    i = bisect.bisect_right(v, (blk, 1 << 62, 0.0))
    if i == 0:
        return None
    r = (v[i - 1][2] / 2 ** 96) ** 2
    return r if eth0.get(pool, True) else 1 / r


def ahead(pool, b0, b1):
    v = sw.get(pool, [])
    return bisect.bisect_right(v, (b1, 1 << 62, 0.0)) - bisect.bisect_right(v, (b0, 1 << 62, 0.0))


jr = []
for f in sorted(L.glob("exec*/journal.jsonl")):
    for ln in open(f):
        try:
            jr.append(json.loads(ln))
        except ValueError:
            pass
orders = [j for j in jr if j.get("event") == "order"]
skipped = [j for j in jr if j.get("event") == "buy_skipped"]

tutup = {}
for ln in open(L / "live.log", errors="replace"):
    m = re.search(r"TUTUP (0x[0-9a-f]+) ([+-]\d+)% \| akun \$([\d.]+) -> \$([\d.]+)", ln)
    if m:
        tutup.setdefault(m[1], []).append((int(m[2]) / 100, float(m[3])))

out = {"order_beli": 0, "beli_dilewati_telat": len(skipped), "skenario": {}}
ages = [j["data"].get("age_ms", 0) for j in orders if j["data"].get("kind", "").startswith("Buy")]
out["umur_order_beli_ms"] = {"median": statistics.median(ages) if ages else None, "maks": max(ages) if ages else None}
for nama, k in LAT.items():
    beli, jual = [], {}
    for j in orders:
        d = j["data"]
        pool = d["pool"].lower()
        kind = d.get("kind", "")
        t_send = j["ms"]
        b_seen = at(t_send - d.get("age_ms", 0), blk_s)
        b_land = at(t_send, head_s) + k
        p0, p1 = tok_per_eth(pool, b_seen), tok_per_eth(pool, b_land)
        if not p0 or not p1:
            continue
        if kind.startswith("Buy"):
            r = p1 / p0  # tokens we get per ETH, landing vs what the bot priced
            beli.append({"pool": pool, "rasio": r, "terisi": r >= 1 - BUY_TOL, "didahului": ahead(pool, b_seen, b_land)})
        elif kind.startswith(("Exit", "TakeProfit")) and pool not in jual:
            r = p0 / p1  # ETH we get per token, landing vs what the bot priced
            jual[pool] = {"rasio": r, "dalam_15": r >= 1 - SELL_TOL, "didahului": ahead(pool, b_seen, b_land)}
    # rough account effect: each closed trade's return scaled by the buy and sell price ratios
    d_pnl, n_ganti, menang_bot, menang_nyata, n_t = 0.0, 0, 0, 0, 0
    for b in beli:
        tr = tutup.get(b["pool"][:10])
        if not tr:
            continue
        ret, size = tr[0]
        n_t += 1
        menang_bot += ret > 0
        if not b["terisi"]:
            d_pnl -= size * ret  # the buy reverts: no trade at all
            n_ganti += 1
            continue
        rs = jual.get(b["pool"], {}).get("rasio", 1.0)
        nyata = (1 + ret) * b["rasio"] * rs - 1
        menang_nyata += nyata > 0
        d_pnl += size * (nyata - ret)
    rb = [b["rasio"] for b in beli]
    rj = [v["rasio"] for v in jual.values()]
    out["order_beli"] = len(beli)
    out["skenario"][nama] = {
        "blok_latensi": k,
        "beli_terisi": sum(b["terisi"] for b in beli), "beli_gagal_harga_lari": sum(not b["terisi"] for b in beli),
        "harga_beli_lebih_mahal_median_pct": (1 / statistics.median(rb) - 1) * 100 if rb else None,
        "harga_beli_lebih_mahal_p90_pct": (1 / sorted(rb)[int(0.1 * (len(rb) - 1))] - 1) * 100 if rb else None,
        "beli_didahului_bot_lain_median": statistics.median([b["didahului"] for b in beli]) if beli else None,
        "beli_tanpa_didahului": sum(b["didahului"] == 0 for b in beli),
        "jual_n": len(rj), "jual_dalam_15pct": sum(v["dalam_15"] for v in jual.values()),
        "harga_jual_lebih_murah_median_pct": (1 - statistics.median(rj)) * 100 if rj else None,
        "trade_dinilai": n_t, "menang_versi_bot": menang_bot, "menang_versi_nyata": menang_nyata, "beli_batal": n_ganti,
        "selisih_pnl_usd": d_pnl,
    }
st = json.load(open(L / "status.json"))
out["equity_bot"] = st.get("akun", {}).get("equity")
json.dump(out, sys.stdout, indent=1)
print()
