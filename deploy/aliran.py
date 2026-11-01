"""Live view of what the bot is doing, in English: BUY, SELL, the balance after each one.

Read-only. It opens nothing but the files the bot already writes — rust_live/live.log, rust_live/exec/journal.jsonl,
rust_live/status.json, rust_live/exec_status.json — and prints a line the moment the bot acts. It never writes, and
it cannot affect trading. Works in live mode (real transactions) and in shadow mode (orders signed, not broadcast).

Run it with: sudo ./rhc.sh aliran        (Ctrl+C to leave)

Usage: aliran.py <bot-dir> [minutes]    minutes omitted = keep going until Ctrl+C
"""
import json
import os
import re
import sys
import time

BOT = sys.argv[1] if len(sys.argv) > 1 else "/opt/rhc/bot"
MENIT = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0
L = os.path.join(BOT, "rust_live")
LOG = os.path.join(L, "live.log")
JRN = os.path.join(L, "exec", "journal.jsonl")
ST = os.path.join(L, "status.json")
EST = os.path.join(L, "exec_status.json")

REASON = {
    "model bahaya": "danger model turned negative",
    "rug terdeteksi": "RUG detected",
    "dump >=5% LP": "whale dumped >=5% of LP",
    "tanda pucuk": "topping-out signal",
    "trailing 40%": "trailing stop, 40% off peak",
    "stop -40%": "hard stop, -40%",
    "beli gagal": "buy transaction failed",
    "akhir data": "still open when the run ended",
}
VERDICT = {"LOLOS": "filter PASS", "tolak(ml)": "filter reject", "DIKECUALIKAN": "excluded"}
# the gates the bot can name when it refuses a coin, in its own words -> English
WHY = {
    "honeypot (belum ada penjual)": "honeypot: nobody has sold this coin yet",
    "dev pernah rug": "this dev has rugged a coin before",
    "ml": "filter score below the moving threshold",
    "slot penuh": "already holding the maximum number of positions",
    "eksposur penuh": "exposure limit reached",
    "kas kurang": "not enough cash left for a minimum-size order",
    "data basi": "stale data",
    "pemanasan model": "model still warming up",
    "rem harian": "daily loss brake",
    "stop darurat": "emergency stop is on",
    "model belum siap": "model not ready yet",
    "cek telat >10 dtk": "the minute-1 check arrived more than 10 s late",
    "tanpa keputusan": "no decision came back",
    "cek jual: simulasi tidak terjawab": "sell check: the simulation did not answer",
    "cek jual: jawaban simulasi rusak": "sell check: the simulation answer was malformed",
}


def why_en(w):
    """Translate a refusal, keeping any numbers the bot put in it."""
    w = (w or "").strip()
    if w in WHY:
        return WHY[w]
    if w.startswith("cek jual: hasil jual terlalu kecil"):
        return "sell check: the simulated sale brought back too little — " + w.split("(", 1)[-1].rstrip(")")
    if w.startswith("cek jual:") and "gagal di simulasi" in w:
        n = w.split(":", 1)[1].strip().split()[0]
        return "sell check: step %s failed in simulation — coin skipped (a hook may block selling)" % n
    if w.startswith("cek jual:"):
        return "sell check: " + w.split(":", 1)[1].strip()
    return w

SARING = re.compile(r"\[([\d\-T:]+)\] SARING (0x[0-9a-f]+) \| skor ([\d.]+) (LOLOS|tolak\(ml\)|DIKECUALIKAN)"
                    r" \| dev (\S+) \| penjual menit-1 (\d+) \| LP \$(\d+)")
TUTUP = re.compile(r"\[([\d\-T:]+)\] TUTUP (0x[0-9a-f]+) ([+-]\d+)% \| akun \$([\d.]+) -> \$([\d.]+) \(([+-][\d.]+)\)"
                   r" \| ([\d.]+) mnt \| (.+)")
BELI_OK = re.compile(r"\[([\d\-T:]+)\] BELI OK (0x[0-9a-f]+): ([\d.]+) ETH \(\$([\d.]+)\)")
JUAL_OK = re.compile(r"\[([\d\-T:]+)\] JUAL OK (0x[0-9a-f]+): ([\d.]+) ETH \(\$([\d.]+)\)")
BELI_GAGAL = re.compile(r"\[([\d\-T:]+)\] BELI GAGAL (0x[0-9a-f]+): (.+)")
BELI_LEWAT = re.compile(r"\[([\d\-T:]+)\] BELI DILEWATI (0x[0-9a-f]+): (.+)")
SELESAI = re.compile(r"\[([\d\-T:]+)\] POSISI SELESAI (0x[0-9a-f]+): hasil nyata \$([+-][\d.]+)"
                     r" \(gas \$([\d.]+)\) \| total nyata \$([+-][\d.]+)")
JUAL_REVERT = re.compile(r"\[([\d\-T:]+)\] JUAL REVERT (0x[0-9a-f]+) \(percobaan (\d+)\)")


def muat(p):
    try:
        return json.load(open(p))
    except Exception:
        return None


def jam(iso):
    return iso[11:19] if len(iso) > 18 else iso


def p10(p):
    return p[:10]


class Ekor:
    """Follow a growing file from the end of what is already there."""

    def __init__(self, path, dari_awal=False):
        self.path = path
        self.pos = 0 if dari_awal else (os.path.getsize(path) if os.path.exists(path) else 0)

    def baru(self):
        if not os.path.exists(self.path):
            return []
        n = os.path.getsize(self.path)
        if n < self.pos:
            self.pos = 0
        if n == self.pos:
            return []
        with open(self.path, "r", encoding="utf8", errors="replace") as f:
            f.seek(self.pos)
            teks = f.read()
            self.pos = f.tell()
        return teks.splitlines()


def kepala():
    d = muat(ST)
    print("=" * 100)
    print("LIVE VIEW — every buy, every sell, the balance after each one.  Read-only; Ctrl+C to leave.")
    if d:
        a, s = d["akun"], d.get("setelan", {})
        print("mode     : %s" % d["mode"])
        print("account  : start $%.2f | equity $%.2f | cash $%.2f | open %d"
              % (a["modal_awal"], a["equity_harga_pasar"], a["kas"], a["posisi_terbuka"]))
        print("sizing   : quarter Kelly = %.4f of CURRENT equity (compounding) -> next order about $%.2f"
              % (a["kelly"], a["kelly"] * a["equity_harga_pasar"]))
        print("limits   : max open %s | cap %s of pool liquidity | threshold = quantile %s of last 100 scores"
              % (s.get("MAX_POS"), s.get("LP_CAP"), s.get("QTHR")))
        print("trades   : %d bought, %d closed, %d won | max drawdown %.1f%% | emergency stop %s"
              % (a["trade_dibeli"], a["trade_selesai"], a["menang"], a["drawdown_maks_pct"], a["stop_darurat"]))
    e = muat(EST)
    if e:
        t = e.get("total") or {}
        print("wallet   : %.6f ETH | realised $%+.2f | gas $%.3f | closed %s, won %s, failed buys %s"
              % (e.get("eth_balance") or 0, t.get("realized_usd") or 0, t.get("gas_usd") or 0,
                 t.get("closed"), t.get("wins"), t.get("buys_failed")))
    print("=" * 100)


def denyut():
    d = muat(ST)
    if not d:
        return
    a = d["akun"]
    baris = ("%s BALANCE   equity $%.2f  cash $%.2f  open %d  size-next $%.2f  bought %d  closed %d  won %d"
             "  maxDD %.1f%%  block %d  lag %d"
             % (time.strftime("%H:%M:%S"), a["equity_harga_pasar"], a["kas"], a["posisi_terbuka"],
                a["kelly"] * a["equity_harga_pasar"], a["trade_dibeli"], a["trade_selesai"], a["menang"],
                a["drawdown_maks_pct"], d["last_block"], d["lag_blocks"]))
    e = muat(EST)
    if e:
        t = e.get("total") or {}
        baris += "  | WALLET %.6f ETH  realised $%+.2f  gas $%.3f" % (
            e.get("eth_balance") or 0, t.get("realized_usd") or 0, t.get("gas_usd") or 0)
    print(baris, flush=True)
    if a["stop_darurat"]:
        print("ALERT     emergency stop is ON — the bot buys nothing more; open positions are still sold", flush=True)
    if d.get("kill_switch", {}).get("aktif"):
        print("ALERT     kill switch is ON: %s" % d["kill_switch"].get("alasan"), flush=True)


def main():
    kepala()
    log, jrn = Ekor(LOG), Ekor(JRN)
    skor, ukuran = {}, {}
    habis = time.time() + MENIT * 60 if MENIT else 0
    tik = 0.0
    while True:
        if habis and time.time() > habis:
            break
        for l in jrn.baru():
            try:
                e = json.loads(l)
            except Exception:
                continue
            ev, dt = e.get("event"), e.get("data") or {}
            t = time.strftime("%H:%M:%S", time.localtime((e.get("ms") or 0) / 1000)) if e.get("ms") else "--:--:--"
            pool = dt.get("pool", "?")
            if ev == "shadow_buy":
                usd = dt.get("usd") or 0.0
                ukuran[p10(pool)] = usd
                d = muat(ST)
                eq = d["akun"]["equity_harga_pasar"] if d else 0.0
                print("%s BUY       %s  size $%.2f  (%.2f%% of equity $%.2f)  score %s  [signed, not broadcast]"
                      % (t, p10(pool), usd, 100 * usd / eq if eq else 0, eq, skor.get(p10(pool), "-")), flush=True)
            elif ev == "shadow_sell":
                print("%s SELL      %s  eth back %.6f  (whole position: %s)  [signed, not broadcast]"
                      % (t, p10(pool), dt.get("eth_out") or 0.0, dt.get("all")), flush=True)
            elif ev == "buy_skipped":
                print("%s SKIP      %s  %s" % (t, p10(pool), why_en(dt.get("why"))), flush=True)
            elif ev in ("sell_presim_reverted", "tx_dropped", "nonce_resynced", "tx_unconfirmed"):
                print("%s NOTE      %s  %s  %s" % (t, p10(pool), ev, json.dumps(dt)[:100]), flush=True)

        for l in log.baru():
            m = BELI_OK.search(l)
            if m:
                t, pool, eth, usd = m.groups()
                ukuran[p10(pool)] = float(usd)
                d = muat(ST)
                eq = d["akun"]["equity_harga_pasar"] if d else 0.0
                print("%s BUY  OK   %s  size $%s  (%.2f%% of equity $%.2f)  %s ETH  score %s"
                      % (jam(t), p10(pool), usd, 100 * float(usd) / eq if eq else 0, eq, eth,
                         skor.get(p10(pool), "-")), flush=True)
                continue
            m = JUAL_OK.search(l)
            if m:
                t, pool, eth, usd = m.groups()
                print("%s SELL OK   %s  got $%s  (%s ETH)" % (jam(t), p10(pool), usd, eth), flush=True)
                continue
            m = SELESAI.search(l)
            if m:
                t, pool, pnl, gas, total = m.groups()
                v = float(pnl)
                print("%s %s  %s  real money %+.2f  gas $%s  running total $%s"
                      % (jam(t), "PROFIT   " if v > 0 else "LOSS     ", p10(pool), v, gas, total), flush=True)
                continue
            m = TUTUP.search(l)
            if m:
                t, pool, pct, dari, ke, pnl, mnt, alasan = m.groups()
                v = float(pnl)
                # the bot's own line labels these two "akun", but they are the POSITION: what went in and what
                # came back. The account total is on the BALANCE line, not here.
                d = muat(ST)
                eq = d["akun"]["equity_harga_pasar"] if d else None
                kas = d["akun"]["kas"] if d else None
                print("%s CLOSE     %s  %+d%%  %s %+.2f  position $%s -> $%s%s  held %.0f min  %s"
                      % (jam(t), p10(pool), int(pct), "PROFIT" if v > 0 else "LOSS", v, dari, ke,
                         ("  | ACCOUNT equity $%.2f cash $%.2f" % (eq, kas)) if eq is not None else "",
                         float(mnt), REASON.get(alasan.strip(), alasan.strip())), flush=True)
                continue
            m = BELI_GAGAL.search(l)
            if m:
                print("%s BUY FAIL  %s  %s" % (jam(m.group(1)), p10(m.group(2)), m.group(3)[:90]), flush=True)
                continue
            m = BELI_LEWAT.search(l)
            if m:
                print("%s SKIP      %s  %s" % (jam(m.group(1)), p10(m.group(2)), why_en(m.group(3))[:115]), flush=True)
                continue
            m = JUAL_REVERT.search(l)
            if m:
                print("%s SELL RTRY %s  sell reverted, attempt %s — retrying"
                      % (jam(m.group(1)), p10(m.group(2)), m.group(3)), flush=True)
                continue
            m = SARING.search(l)
            if m:
                t, pool, s, verdikt, dev, jual, lp = m.groups()
                skor[p10(pool)] = s
                print("%s SCORE     %s  %s  %-13s  seller-in-minute-1 %-3s  LP $%s  dev %s"
                      % (jam(t), p10(pool), s, VERDICT[verdikt], jual, format(int(lp), ","), dev), flush=True)
                continue
            if "KILL SWITCH" in l or "STOP DARURAT" in l or "MODE AMAN" in l or "panic" in l:
                print("ALERT     %s" % l.strip()[:150], flush=True)

        if time.time() - tik >= 60:
            tik = time.time()
            denyut()
        time.sleep(1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nleft the live view. The bot keeps running.")
