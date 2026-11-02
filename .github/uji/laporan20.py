"""Final report of a shadow run: the balance every 20 minutes, and every coin the bot bought, sold or still holds.

Reads only what the bot wrote: rust_live/exec/journal.jsonl (the orders it built and signed, with their size),
rust_live/live.log (the filter's verdict per coin and every close), rust_live/status.json, and the once-a-minute
balance samples the workflow collected. Nothing here is simulated or filled in.

A coin that the filter passed is NOT a coin that was bought: the dev rule, the honeypot rule, the open-position
limit, the exposure limit and the cash floor all come after it. The buys below are the real orders.
"""
import json
import re
import sys

BOT = sys.argv[1] if len(sys.argv) > 1 else "/opt/rhc/bot"
JAM = sys.argv[2] if len(sys.argv) > 2 else "/tmp/jam.txt"
L = BOT + "/rust_live"

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


def log_lines():
    try:
        teks = open(L + "/live.log", encoding="utf8", errors="replace").read()
    except OSError:
        return [], []
    return [m.groups() for m in SARING.finditer(teks)], [m.groups() for m in TUTUP.finditer(teks)]


def journal():
    beli, dilewati, jual = [], [], []
    try:
        for l in open(L + "/exec/journal.jsonl", encoding="utf8", errors="replace"):
            try:
                e = json.loads(l)
            except Exception:
                continue
            ev, d = e.get("event"), e.get("data") or {}
            if ev == "shadow_buy":
                beli.append((e.get("ms"), d.get("pool", "?"), d.get("usd") or 0.0, d.get("status")))
            elif ev == "buy_skipped":
                dilewati.append((e.get("ms"), d.get("pool", "?"), d.get("why", "")))
            elif ev == "shadow_sell":
                jual.append((e.get("ms"), d.get("pool", "?"), d.get("eth_out") or 0.0, d.get("all")))
    except OSError:
        pass
    return beli, dilewati, jual


def tabel_saldo():
    baris = []
    try:
        for l in open(JAM, encoding="utf8"):
            p = l.split()
            if len(p) >= 9:
                baris.append(p)
    except OSError:
        return []
    if not baris:
        return []
    t0 = int(baris[0][0])
    pilih, batas = [], 0
    for p in baris:
        m = (int(p[0]) - t0) // 60000
        if m >= batas:
            pilih.append((m, p))
            batas += 20
    if pilih and pilih[-1][1] is not baris[-1]:
        pilih.append(((int(baris[-1][0]) - t0) // 60000, baris[-1]))
    return pilih


def main():
    saring, tutup = log_lines()
    beli, dilewati, _ = journal()
    size = {p: u for _, p, u, _ in beli}
    skor = {p: s for _, p, s, _, _, _, _ in saring}
    ditutup = {t[1] for t in tutup}
    hold = [b for b in beli if b[1] not in ditutup]

    print("=" * 96)
    print("SHADOW RUN REPORT — start $40, EXEC_MODE=shadow (every order built and signed, none broadcast)")
    print("=" * 96)

    rows = tabel_saldo()
    if rows:
        print("\nBALANCE EVERY 20 MINUTES")
        print("  minute |    equity |      cash | open | buys | closed | wins | max DD")
        print("  " + "-" * 74)
        for m, p in rows:
            print("  %6d | $%8.2f | $%8.2f | %4s | %4s | %6s | %4s | %5.1f%%"
                  % (m, float(p[1]), float(p[2]), p[3], p[4], p[5], p[6], float(p[7])))

    print("\nFILTER: %d coins scored  |  ORDERS: %d bought, %d skipped at a later gate  |  %d closed, %d still held"
          % (len(saring), len(beli), len(dilewati), len(tutup), len(hold)))

    if beli:
        print("\nBUY — real orders the executor built and signed")
        print("  pool       | size   | score | note")
        print("  " + "-" * 74)
        for _, pool, usd, st in beli:
            print("  %s | $%5.2f | %5s | %s" % (pool[:10], usd, skor.get(pool, "-"), st))
        print("  total bought: $%.2f over %d orders, average $%.2f"
              % (sum(x[2] for x in beli), len(beli), sum(x[2] for x in beli) / len(beli)))

    if tutup:
        print("\nSELL — every position closed")
        print("  time     | pool       | result | P/L    | size   | equity after | held | reason")
        print("  " + "-" * 94)
        untung = 0.0
        for t, pool, pct, dari, ke, pnl, mnt, alasan in tutup:
            pnl = float(pnl)
            untung += pnl
            print("  %s | %s | %+5d%% | %s%+.2f | $%5.2f | $%11.2f | %3.0fm | %s"
                  % (t[11:], pool[:10], int(pct), "P" if pnl > 0 else "L", pnl,
                     size.get(pool, 0.0), float(ke), float(mnt), ALASAN.get(alasan.strip(), alasan.strip())))
        n = len(tutup)
        w = sum(1 for x in tutup if float(x[5]) > 0)
        print("\n  closed %d | wins %d = %.1f%% | net P/L $%+.2f" % (n, w, 100 * w / n, untung))
        print("  NOTE: %d closes is far too few to judge the win rate. The 95%% range for %d/%d is wide." % (n, w, n))

    if hold:
        print("\nHOLD — bought, not yet closed when the run ended")
        print("  pool       | size   | score")
        print("  " + "-" * 40)
        for _, pool, usd, _ in hold:
            print("  %s | $%5.2f | %5s" % (pool[:10], usd, skor.get(pool, "-")))

    if dilewati:
        why = {}
        for _, _, w in dilewati:
            k = w.split(",")[0][:60]
            why[k] = why.get(k, 0) + 1
        print("\nSKIPPED BY A GATE AFTER THE FILTER")
        for k, v in sorted(why.items(), key=lambda x: -x[1]):
            print("  %-62s %4d" % (k, v))

    tolak = {}
    for s in saring:
        if s[3] != "LOLOS":
            tolak[s[3]] = tolak.get(s[3], 0) + 1
    if tolak:
        print("\nREJECTED BY THE ML+DL FILTER")
        for k, v in tolak.items():
            print("  %-20s %5d coins" % (k, v))

    try:
        d = json.load(open(L + "/status.json"))
        a, f, s = d["akun"], d.get("filter", {}), d.get("setelan", {})
        print("\nFINAL STATE, AS THE BOT ITSELF REPORTS IT")
        print("  mode          : %s" % d["mode"])
        print("  start / equity: $%.2f -> $%.2f  (%+.2f, %+.1f%%)"
              % (a["modal_awal"], a["equity_harga_pasar"], a["equity_harga_pasar"] - a["modal_awal"],
                 100 * (a["equity_harga_pasar"] / a["modal_awal"] - 1) if a["modal_awal"] else 0))
        print("  cash          : $%.2f | open positions %d" % (a["kas"], a["posisi_terbuka"]))
        print("  trades        : %d bought, %d closed, %d won" % (a["trade_dibeli"], a["trade_selesai"], a["menang"]))
        print("  sizing        : kelly 1/4 = %.4f of equity (compounded: size = fraction x CURRENT equity)" % a["kelly"])
        print("  max drawdown  : %.1f%% | emergency stop: %s" % (a["drawdown_maks_pct"], a["stop_darurat"]))
        print("  kill switch   : %s" % d["kill_switch"])
        print("  settings      : MAX_POS %s | LP_CAP %s | QTHR %s | KILL_DD %s"
              % (s.get("MAX_POS"), s.get("LP_CAP"), s.get("QTHR"), s.get("KILL_DD")))
        print("  filter        : scored %s | outcomes learned %s | ML retrained %s | accuracy %s"
              % (f.get("koin_dinilai"), f.get("hasil_koin_dipelajari"), f.get("ml_dilatih_ulang"),
                 f.get("akurasi_data_belum_dilihat_500")))
        print("  radar         : %s pools known | %s swaps recorded"
              % (d.get("radar", {}).get("pool_dikenal"), d.get("radar", {}).get("swap_tercatat")))
    except (OSError, KeyError, ValueError) as e:
        print("\nstatus.json unreadable: %s" % e)


if __name__ == "__main__":
    main()
