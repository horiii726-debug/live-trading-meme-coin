"""Live stream of what the bot does, in English, while a shadow run is going.

Prints a line the moment the bot acts: BUY with its size, SELL with profit or loss and the new equity, SKIP with the
gate that refused the coin, SCORE for every coin the filter graded. Every 60 s a HEARTBEAT line carries equity, cash,
open positions, the Kelly fraction and the chain position. Everything is read from what the bot already writes
(rust_live/live.log, rust_live/exec/journal.jsonl, rust_live/status.json); nothing is computed or assumed here.

Usage: aliran.py <bot-dir> <minutes>
"""
import json
import os
import re
import sys
import time

BOT = sys.argv[1] if len(sys.argv) > 1 else "/opt/rhc/bot"
MENIT = float(sys.argv[2]) if len(sys.argv) > 2 else 290.0
L = BOT + "/rust_live"
LOG = L + "/live.log"
JRN = L + "/exec/journal.jsonl"
ST = L + "/status.json"

ALASAN = {
    "model bahaya": "danger model turned negative",
    "rug terdeteksi": "RUG detected",
    "dump >=5% LP": "whale dumped >=5% of LP",
    "tanda pucuk": "topping-out signal",
    "trailing 40%": "trailing stop 40% off peak",
    "stop -40%": "hard stop -40%",
    "beli gagal": "buy transaction failed",
    "akhir data": "run ended while still open",
}
SARING = re.compile(
    r"\[([\d\-T:]+)\] SARING (0x[0-9a-f]+) \| skor ([\d.]+) (LOLOS|tolak\(ml\)|DIKECUALIKAN)"
    r" \| dev (\S+) \| penjual menit-1 (\d+) \| LP \$(\d+)"
)
TUTUP = re.compile(
    r"\[([\d\-T:]+)\] TUTUP (0x[0-9a-f]+) ([+-]\d+)% \| akun \$([\d.]+) -> \$([\d.]+) \(([+-][\d.]+)\)"
    r" \| ([\d.]+) mnt \| (.+)"
)


def status():
    try:
        return json.load(open(ST))
    except Exception:
        return None


def jam(iso):
    return iso[11:] if len(iso) > 11 else iso


def pendek(p):
    return p[:10]


class Ekor:
    """Follow a growing text file from the end of what already exists."""

    def __init__(self, path):
        self.path = path
        self.pos = os.path.getsize(path) if os.path.exists(path) else 0

    def baru(self):
        if not os.path.exists(self.path):
            return []
        n = os.path.getsize(self.path)
        if n < self.pos:  # rotated
            self.pos = 0
        if n == self.pos:
            return []
        with open(self.path, "r", encoding="utf8", errors="replace") as f:
            f.seek(self.pos)
            teks = f.read()
            self.pos = f.tell()
        return teks.splitlines()


def main():
    print("stream start — BUY / SELL / SKIP / SCORE as they happen, HEARTBEAT every 60 s", flush=True)
    d = status()
    if d:
        a = d["akun"]
        print("mode: %s" % d["mode"], flush=True)
        print("account: start $%.2f | equity $%.2f | cash $%.2f | kelly %.4f | max open %s"
              % (a["modal_awal"], a["equity_harga_pasar"], a["kas"], a["kelly"], d["setelan"]["MAX_POS"]), flush=True)

    log, jrn = Ekor(LOG), Ekor(JRN)
    habis = time.time() + MENIT * 60
    denyut = 0.0
    ukuran = {}      # pool -> size in USD, from the shadow_buy journal entry
    skor = {}        # pool -> score, from the SARING line
    n_buy = n_sell = n_skip = 0

    while time.time() < habis:
        for l in jrn.baru():
            try:
                e = json.loads(l)
            except Exception:
                continue
            ev, dt = e.get("event"), e.get("data") or {}
            t = time.strftime("%H:%M:%S", time.localtime((e.get("ms") or 0) / 1000)) if e.get("ms") else "--:--:--"
            pool = pendek(dt.get("pool", "?"))
            if ev == "shadow_buy":
                usd = dt.get("usd") or 0.0
                ukuran[dt.get("pool", "")] = usd
                st = status()
                eq = st["akun"]["equity_harga_pasar"] if st else 0.0
                kl = st["akun"]["kelly"] if st else 0.0
                op = st["akun"]["posisi_terbuka"] if st else 0
                n_buy += 1
                print("%s BUY   %s  size $%.2f  (= %.2f%% of equity $%.2f, kelly 1/4 = %.4f)  open %d  score %s"
                      % (t, pool, usd, 100 * usd / eq if eq else 0, eq, kl, op, skor.get(dt.get("pool", ""), "-")),
                      flush=True)
            elif ev == "shadow_sell":
                print("%s SELL  %s  tokens out, eth back %.6f  (all=%s)"
                      % (t, pool, dt.get("eth_out") or 0.0, dt.get("all")), flush=True)
            elif ev == "buy_skipped":
                n_skip += 1
                print("%s SKIP  %s  %s" % (t, pool, dt.get("why")), flush=True)
            elif ev in ("sell_presim_reverted", "tx_dropped", "nonce_resynced", "reapprove", "buy_pause_released"):
                print("%s NOTE  %s  %s %s" % (t, pool, ev, json.dumps(dt)[:110]), flush=True)

        for l in log.baru():
            m = TUTUP.search(l)
            if m:
                t, pool, pct, dari, ke, pnl, mnt, alasan = m.groups()
                pnl = float(pnl)
                n_sell += 1
                size = ukuran.get(pool, 0.0)
                print("%s CLOSE %s  %+d%%  %s %+.2f  size $%.2f -> back $%.2f  EQUITY $%s  held %.0f min  reason: %s"
                      % (jam(t), pendek(pool), int(pct), "PROFIT" if pnl > 0 else "LOSS", pnl,
                         size, size + pnl, ke, float(mnt), ALASAN.get(alasan.strip(), alasan.strip())), flush=True)
                continue
            m = SARING.search(l)
            if m:
                t, pool, s, verdikt, dev, jual, lp = m.groups()
                skor[pool] = s
                v = {"LOLOS": "filter PASS", "tolak(ml)": "filter reject", "DIKECUALIKAN": "excluded"}[verdikt]
                print("%s SCORE %s  %s  %s  seller-in-minute-1 %s  LP $%s  dev %s"
                      % (jam(t), pendek(pool), s, v, jual, f"{int(lp):,}", dev), flush=True)
                continue
            if "KILL SWITCH" in l or "STOP DARURAT" in l or "MODE AMAN" in l:
                print("ALERT %s" % l.strip()[:150], flush=True)

        if time.time() - denyut >= 60:
            denyut = time.time()
            d = status()
            if d:
                a = d["akun"]
                print("%s HEARTBEAT  equity $%.2f  cash $%.2f  open %d  kelly %.4f  buys %d  closed %d  wins %d"
                      "  maxDD %.1f%%  block %d  lag %d  safe-mode %s"
                      % (time.strftime("%H:%M:%S"), a["equity_harga_pasar"], a["kas"], a["posisi_terbuka"],
                         a["kelly"], a["trade_dibeli"], a["trade_selesai"], a["menang"], a["drawdown_maks_pct"],
                         d["last_block"], d["lag_blocks"], d.get("mode_aman")), flush=True)
                if a["stop_darurat"]:
                    print("ALERT emergency stop is ON — no more buys", flush=True)
        time.sleep(1)

    print("stream end — buys %d, closes %d, skips %d" % (n_buy, n_sell, n_skip), flush=True)


if __name__ == "__main__":
    main()
