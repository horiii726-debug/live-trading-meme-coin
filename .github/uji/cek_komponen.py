#!/usr/bin/env python3
"""Component check of a bot that ran on the live chain (CI, read-only). Every section ends with LULUS / GAGAL / INFO.

    cek_komponen.py <bot dir, e.g. /opt/rhc/bot> [minimum closed trades, default 10]

1 RPC + block stream      lag, exactly-once log delivery (duplicates / out-of-range / removed logs dropped), 429, safe mode
2 stored chain data       swaps / pools / liquidity rows: numeric, no duplicates, block order, sqrtPrice > 0
3 ETH price               from the ETH/USDG pool, sane range
4 screening               pools scored at minute 1, pass rate of the ML filter, exclusions (dev, asset class, stale)
5 decision data           34 features per coin, no NaN, missing values = -1, scores in (0, 1)
6 adaptive filter         ML CatBoost + DL active, outcomes learned, out-of-sample accuracy, ML retrains + parity
7 trades                  closed trades, win rate, rug rate, exit reasons
8 executor (shadow)       orders built and signed, never broadcast
"""
import collections
import csv
import glob
import json
import math
import os
import re
import sys

bot = sys.argv[1]
MIN_TRADE = int(sys.argv[2]) if len(sys.argv) > 2 else 10
live = os.path.join(bot, "rust_live")
hasil = []


def bagian(n, judul):
    print()
    print("=" * 90)
    print(f"{n}. {judul}")
    print("=" * 90)


def nilai(nama, ok, detail):
    status = "LULUS" if ok is True else ("GAGAL" if ok is False else "INFO ")
    hasil.append((nama, status))
    print(f"[{status}] {nama}: {detail}")


def baca(p):
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            return f.read().splitlines()
    except FileNotFoundError:
        return []


def jsonl(p):
    out = []
    for l in baca(p):
        try:
            out.append(json.loads(l))
        except ValueError:
            pass
    return out


def angka(x):
    try:
        v = float(x)
        return v if math.isfinite(v) else None
    except (TypeError, ValueError):
        return None


log = baca(os.path.join(live, "live.log"))
try:
    st = json.load(open(os.path.join(live, "status.json"), encoding="utf-8"))
except Exception:  # noqa: BLE001
    st = {}

# ---------------------------------------------------------------- 1
bagian(1, "RPC + aliran blok")
lag = st.get("lag_blocks")
nilai("bot mengikuti chain", lag is not None and lag < 600, f"lag {lag} blok (0,1 dtk/blok), blok terakhir {st.get('last_block')}, RPC diam {st.get('rpc_diam_dtk')} dtk")
g = st.get("log_gate", {})
nilai("log tepat sekali & berurutan (loggate)", True if g else None,
      f"dibaca ulang OK {g.get('reread_ok')}, duplikat dibuang {g.get('duplicate_dropped')}, di luar rentang dibuang {g.get('outside_range_dropped')}, "
      f"dihapus chain (reorg) dibuang {g.get('removed_dropped')}, terlambat dikirim {g.get('late_delivered')}, batas rentang {g.get('range_cap_blocks')}")
n429 = sum("429" in l for l in log)
aman = [l for l in log if "MODE AMAN" in l]
nilai("batas laju RPC publik", None, f"{n429} jawaban 429 (RPC publik; RPC berbayar kamu jauh lebih longgar), MODE AMAN {len(aman)}x")

# ---------------------------------------------------------------- 2
bagian(2, "Data chain yang disimpan bot (store/)")
store = os.path.join(live, "store")


def cek_csv(nama, kunci, kolom_angka, syarat=None):
    p = os.path.join(store, nama)
    if not os.path.exists(p):
        nilai(nama, None, "tidak ada (belum ada event jenis ini)")
        return
    n = dup = rusak = mundur = 0
    lihat = set()
    blk_lalu = -1.0
    with open(p, encoding="utf-8", errors="replace") as f:
        r = csv.DictReader(f)
        for row in r:
            n += 1
            k = tuple(row.get(c) for c in kunci)
            if k in lihat:
                dup += 1
            lihat.add(k)
            vals = [angka(row.get(c)) for c in kolom_angka]
            if any(v is None for v in vals) or (syarat and not syarat(row)):
                rusak += 1
            b = angka(row.get("block") or row.get("init_block"))
            if b is not None:
                if b < blk_lalu:
                    mundur += 1
                blk_lalu = max(blk_lalu, b)
    nilai(nama, n > 0 and dup == 0 and rusak == 0 and mundur == 0, f"{n} baris | duplikat {dup} | angka rusak/tidak masuk akal {rusak} | urutan blok mundur {mundur}")


cek_csv("swaps.csv", ["pool_id", "block", "log_idx"], ["block", "log_idx", "a0", "a1", "sqrt_price", "liquidity", "tick", "fee"],
        lambda r: float(r["sqrt_price"]) > 0 and float(r["liquidity"]) >= 0)
cek_csv("pools.csv", ["pool_id"], ["fee", "tick_spacing", "init_block", "log_idx"])
cek_csv("liq.csv", ["pool_id", "block", "log_idx"], ["block", "log_idx", "tick_lo", "tick_hi", "liq_delta"])

# ---------------------------------------------------------------- 3
bagian(3, "Harga ETH (dari pool ETH/USDG di chain)")
eth = st.get("eth_usd")
nilai("harga ETH masuk akal", isinstance(eth, (int, float)) and 500 < eth < 20000, f"ETH = ${eth}")

# ---------------------------------------------------------------- 4
bagian(4, "Penyaringan koin di menit ke-1")
sar = [l for l in log if "] SARING " in l]
hasil_sar = collections.Counter()
for l in sar:
    m = re.search(r"SARING \S+ \| skor [0-9.]+ (\S+)", l)
    if m:
        hasil_sar[m.group(1)] += 1
lolos = hasil_sar.get("LOLOS", 0)
dinilai = lolos + hasil_sar.get("tolak(ml)", 0)
nilai("koin dinilai di menit ke-1", len(sar) > 0, f"{len(sar)} koin -> {dict(hasil_sar)}")
nilai("porsi lolos filter ML (target ±20% = QTHR 0,80)", None if dinilai < 30 else (0.08 < lolos / dinilai < 0.35), f"{lolos}/{dinilai} = {100 * lolos / max(dinilai, 1):.0f}%")
sg = st.get("saringan", {})
nilai("alasan tolak (akun strategi)", None, json.dumps(sg.get("tolak", {}), ensure_ascii=False))
nilai("cek gagal / data basi", None, f"cek gagal {sg.get('cek_gagal')}, data basi {sg.get('data_basi')}")

# ---------------------------------------------------------------- 5
bagian(5, "Data keputusan (fitur menit-1 & skor) — bersih?")
dec = jsonl(os.path.join(live, "decisions.jsonl"))
if dec:
    nf = collections.Counter(len(d.get("feats") or []) for d in dec)
    nan = sum(1 for d in dec for v in (d.get("feats") or []) if angka(v) is None)
    hilang = sum(1 for d in dec for v in (d.get("feats") or []) if v == -1.0)
    tot = sum(len(d.get("feats") or []) for d in dec)
    skor = [d.get("score") for d in dec]
    skor_ok = sum(1 for s in skor if angka(s) is not None and 0.0 < s < 1.0)
    pools = [d.get("pool") for d in dec]
    nilai("34 fitur per koin", set(nf) == {34}, f"jumlah fitur per keputusan: {dict(nf)}")
    nilai("tidak ada NaN / angka rusak di fitur", nan == 0, f"{nan} nilai rusak dari {tot}")
    nilai("nilai hilang ditandai -1 (sama dengan data latih)", None, f"{hilang} dari {tot} ({100 * hilang / max(tot, 1):.1f}%)")
    nilai("skor di antara 0 dan 1", skor_ok == len(skor), f"{skor_ok}/{len(skor)}")
    nilai("satu keputusan per koin", len(pools) == len(set(pools)), f"{len(pools)} keputusan, {len(set(pools))} koin")
else:
    nilai("decisions.jsonl", False, "kosong")

# ---------------------------------------------------------------- 6
bagian(6, "Filter adaptif ML (CatBoost, latih ulang C++) + DL (Rust)")
akt = [l for l in log if "filter adaptif AKTIF" in l]
nilai("filter adaptif aktif saat bot menyala", bool(akt), akt[-1].split("] ", 1)[-1][:200] if akt else "baris 'filter adaptif AKTIF' tidak ada")
fl = st.get("filter", {})
nilai("hasil koin dipelajari", (fl.get("hasil_koin_dipelajari") or 0) > 0, f"{fl.get('hasil_koin_dipelajari')} hasil koin, {fl.get('koin_dinilai')} koin dinilai")
acc = fl.get("akurasi_data_belum_dilihat_500")
nilai("akurasi di data yang belum dilihat (AUC; 0,5 = acak)", None if not acc else (acc.get("dl_auc", 0) > 0.55 or acc.get("ml_auc", 0) > 0.55),
      f"{acc} | DL aktif: {fl.get('dl_aktif')}, dimatikan pengaman {fl.get('dl_dimatikan_pengaman')}x" if acc else "belum 100 hasil koin")
mulai = [l for l in log if "LATIH ULANG ML mulai" in l]
pasang = [l for l in log if "MODEL ML BARU dipasang" in l]
gagal = [l for l in log if "latih ulang ML GAGAL" in l or "model ML baru DITOLAK" in l]
nilai("latih ulang ML (program CatBoost C++)", None if not mulai else (len(pasang) > 0 and not gagal),
      f"mulai {len(mulai)}x, dipasang {len(pasang)}x, gagal/ditolak {len(gagal)}x")
for l in (pasang + gagal)[-4:]:
    print("        ", l.split("] ", 1)[-1][:200])
try:
    fc = json.load(open(os.path.join(bot, "filter_cat.json"), encoding="utf-8"))
    ck = json.load(open(os.path.join(bot, "filter_cat_check.json"), encoding="utf-8"))
    arsip = glob.glob(os.path.join(bot, "arsip_model", "*filter_cat.json"))
    nilai("model ML di disk + cek kesamaan", len(ck.get("X", [])) == len(ck.get("p", [])) > 0,
          f"{len(fc.get('trees', []))} pohon, dilatih {fc.get('waktu', 'bawaan')}, alasan {fc.get('alasan', '-')}, selisih Rust vs CatBoost {ck.get('selisih_catboost', 'bawaan')}, arsip {len(arsip)}")
except Exception as e:  # noqa: BLE001
    nilai("model ML di disk", False, str(e))
nilai("keadaan pembelajar tersimpan (belajar.json)", os.path.exists(os.path.join(live, "belajar.json")), "rust_live/belajar.json")
lab = jsonl(os.path.join(live, "labels.jsonl"))
lab_ok = sum(1 for l in lab if angka(l.get("ret")) is not None and l.get("ret") >= 0 and l.get("decide", 0) <= l.get("exit", 0))
nilai("label hasil koin bersih (ret valid, keputusan sebelum keluar)", lab_ok == len(lab) and len(lab) > 0, f"{lab_ok}/{len(lab)}")
peng = [l for l in log if "PENGAMAN" in l]
nilai("pengaman overfit", None, f"{len(peng)} pergantian" + (f": {peng[-1].split('] ', 1)[-1][:160]}" if peng else ""))

# ---------------------------------------------------------------- 7
bagian(7, "Trade akun strategi (¼ Kelly + compounding)")
a = st.get("akun", {})
tr = [t for t in jsonl(os.path.join(live, "trades.jsonl")) if t.get("plan") == "v4"]
tutup = [l for l in log if "] TUTUP " in l]
rets = [angka(t.get("ret")) for t in tr if angka(t.get("ret")) is not None]
menang = sum(1 for r in rets if r > 1)
rug = sum(1 for r in rets if r < 0.1)
nilai(f"minimal {MIN_TRADE} trade selesai", (a.get("trade_selesai") or 0) >= MIN_TRADE,
      f"dibeli {a.get('trade_dibeli')}, selesai {a.get('trade_selesai')}, menang {a.get('menang')}, equity ${a.get('equity_harga_pasar')}")
if rets:
    nilai("win rate / rug (backtest: ±80% / ±12%)", None, f"menang {menang}/{len(rets)} = {100 * menang / len(rets):.0f}%, rug {rug}/{len(rets)} = {100 * rug / len(rets):.0f}% (sampel kecil)")
alasan = collections.Counter(t.get("reason") for t in tr)
nilai("alasan keluar", None, json.dumps(dict(alasan), ensure_ascii=False))
for l in tutup[-12:]:
    print("        ", l.split("] ", 1)[-1][:170])

# ---------------------------------------------------------------- 8
bagian(8, "Eksekutor (mode shadow: order dibuat & ditandatangani, TIDAK dikirim)")
jr = []
for p in glob.glob(os.path.join(live, "exec*", "journal.jsonl")):
    jr += jsonl(p)
jenis = collections.Counter(j.get("event") or j.get("kind") or j.get("jenis") or "?" for j in jr)
nilai("jurnal eksekutor", None, f"{len(jr)} catatan -> {dict(jenis.most_common(8))}")
skip = collections.Counter()
for j in jr:
    why = j.get("why") or j.get("reason") or j.get("alasan")
    if why and ("skip" in json.dumps(j).lower()):
        skip[re.sub(r"\d+", "#", str(why))[:60]] += 1
if skip:
    nilai("alasan beli dilewati eksekutor", None, json.dumps(dict(skip.most_common(6)), ensure_ascii=False))

# ---------------------------------------------------------------- ringkas
print()
print("=" * 90)
gg = [n for n, s in hasil if s == "GAGAL"]
print(f"RINGKAS: {sum(1 for _, s in hasil if s == 'LULUS')} LULUS, {len(gg)} GAGAL, {sum(1 for _, s in hasil if s == 'INFO ')} info")
for n in gg:
    print(f"   GAGAL: {n}")
