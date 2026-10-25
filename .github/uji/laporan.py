#!/usr/bin/env python3
"""Summarise what the installed bot did during an observation run (CI, read-only).

    laporan.py <bot dir, e.g. /opt/rhc/bot>

Reads rust_live/live.log, rust_live/status.json and the executor's rust_live/exec*/journal.jsonl +
exec_status.json, and prints: how many new pools were screened and how many passed, how many real buy
orders the strategy handed to the executor (and how old they were when the executor saw them), why buys
were skipped, what the shadow simulations returned, RPC health (429s, safe mode) and errors.
"""
import collections
import glob
import json
import os
import re
import statistics
import sys

bot = sys.argv[1]
live = os.path.join(bot, "rust_live")


def lines(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read().splitlines()
    except FileNotFoundError:
        return []


log = lines(os.path.join(live, "live.log"))
saring = collections.Counter()
for l in log:
    m = re.search(r"SARING \S+ \| skor [0-9.]+ (\S+)", l)
    if m:
        saring[m.group(1)] += 1
n429 = sum("429" in l for l in log)
aman = [l for l in log if "MODE AMAN" in l]
tutup = [l for l in log if "] TUTUP " in l]
errors = collections.Counter(re.sub(r"0x[0-9a-f]+|\d+", "#", l.split("] ", 1)[-1])[:110] for l in log if "] error:" in l)
execl = [l for l in log if "] EXEC:" in l]

print("=" * 78)
print("RINGKASAN BOT (live.log)")
print("=" * 78)
print(f"baris log            : {len(log)}")
print(f"pool baru disaring   : {sum(saring.values())}  -> {dict(saring)}")
sar = [l for l in log if "] SARING " in l]
dev_q = [l for l in sar if "| dev ? |" in l]
dev_gagal = [l for l in sar if "dev tidak diketahui" in l]
print(f"dev pembuat ditemukan: {len(sar) - len(dev_q)} dari {len(sar)} | tanpa dev: {len(dev_q)} "
      f"(pencarian gagal -> TIDAK dibeli: {len(dev_gagal)}; sisanya memang tanpa mint di rentang)")
for l in [l for l in log if "aturan dev" in l][-2:]:
    print("   ", l[:160])
print(f"posisi ditutup (TUTUP): {len(tutup)}")
for l in tutup[-10:]:
    print("   ", l[:160])
print(f"jawaban 429 RPC      : {n429}")
print(f"MODE AMAN            : {len(aman)}")
for l in aman[-6:]:
    print("   ", l[:160])
print("error terbanyak      :")
for e, c in errors.most_common(6):
    print(f"   {c:5d}x {e}")
print("baris EXEC           :")
for l in execl[-12:]:
    print("   ", l[:180])

try:
    st = json.load(open(os.path.join(live, "status.json"), encoding="utf-8"))
    a = st.get("akun", {})
    print(f"status.json          : mode={st.get('mode')} blok={st.get('last_block')} lag={st.get('lag_blocks')} "
          f"mode_aman={st.get('mode_aman')} rpc_diam_dtk={st.get('rpc_diam_dtk')}")
    print(f"akun strategi        : dibeli={a.get('trade_dibeli')} selesai={a.get('trade_selesai')} menang={a.get('menang')} "
          f"equity={a.get('equity_harga_pasar')}")
    print(f"saringan             : {st.get('saringan')}")
except Exception as e:  # noqa: BLE001 - report, never crash the summary
    print(f"status.json          : tidak terbaca ({e})")

print()
print("=" * 78)
print("EXECUTOR (journal.jsonl)")
print("=" * 78)
for d in sorted(glob.glob(os.path.join(live, "exec*"))):
    j = [json.loads(x) for x in lines(os.path.join(d, "journal.jsonl")) if x.startswith("{")]
    ev = collections.Counter(x["event"] for x in j)
    print(f"[{os.path.basename(d)}] events: {dict(ev)}")
    kinds = collections.Counter(x["data"].get("kind", "?").split(" ")[0].split("{")[0] for x in j if x["event"] == "order")
    print(f"   order kinds        : {dict(kinds)}")
    ages = [x["data"]["age_ms"] for x in j if x["event"] == "order" and str(x["data"].get("kind", "")).startswith("Buy")]
    if ages:
        print(f"   umur order beli ms : n={len(ages)} median={statistics.median(ages)} max={max(ages)} "
              f"(> 1500 ms dilewati: {sum(a > 1500 for a in ages)})")
    skips = collections.Counter(x["data"].get("why") for x in j if x["event"] == "buy_skipped")
    print(f"   beli dilewati      : {dict(skips)}")
    for x in [x for x in j if x["event"] == "shadow_buy"][-10:]:
        dd = x["data"]
        print(f"   shadow_buy {dd.get('pool','')[:10]} usd={dd.get('usd')} status={dd.get('status')} "
              f"tokens={dd.get('tokens')} error={str(dd.get('error'))[:90]}")
    for x in [x for x in j if x["event"] == "buy_sent"][-10:]:
        dd = x["data"]
        print(f"   buy_sent {dd.get('pool','')[:10]} usd={dd.get('usd')} accepted={dd.get('accepted')} error={str(dd.get('error'))[:90]}")
    try:
        es = json.load(open(os.path.join(d, "exec_status.json"), encoding="utf-8"))
        print(f"   exec_status        : counters={es.get('counters')} last_error={str(es.get('last_error'))[:120]}")
    except Exception as e:  # noqa: BLE001
        print(f"   exec_status        : tidak terbaca ({e})")
