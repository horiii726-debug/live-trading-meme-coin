"""Report of a shadow run: the balance every 20 minutes, and every coin the bot bought, sold, or still holds.

Reads only what the bot wrote: rust_live/live.log (SARING / TUTUP lines), rust_live/status.json, the exec journal, and
the one-minute balance samples the workflow collected. Nothing is simulated or filled in here.
"""
import json
import re
import sys
from collections import OrderedDict

BOT = sys.argv[1] if len(sys.argv) > 1 else "/opt/rhc/bot"
JAM = sys.argv[2] if len(sys.argv) > 2 else "/tmp/jam.txt"
L = BOT + "/rust_live"

SARING = re.compile(
    r"\[([\d\-T:]+)\] SARING (0x[0-9a-f]+) \| skor ([\d.]+) (LOLOS|tolak\(ml\)|DIKECUALIKAN)"
    r" \| dev (\S+) \| penjual menit-1 (\d+) \| LP \$(\d+)"
)
TUTUP = re.compile(
    r"\[([\d\-T:]+)\] TUTUP (0x[0-9a-f]+) ([+-]\d+)% \| akun \$([\d.]+) -> \$([\d.]+) \(([+-][\d.]+)\)"
    r" \| ([\d.]+) mnt \| (.+)"
)


def baca_log():
    try:
        teks = open(L + "/live.log", encoding="utf8", errors="replace").read()
    except OSError:
        return [], []
    return [m.groups() for m in SARING.finditer(teks)], [m.groups() for m in TUTUP.finditer(teks)]


def jam_tabel():
    """One row per 20 minutes from the one-per-minute samples."""
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
        menit = (int(p[0]) - t0) // 60000
        if menit >= batas:
            pilih.append((menit, p))
            batas += 20
    if pilih[-1][1] is not baris[-1]:
        pilih.append(((int(baris[-1][0]) - t0) // 60000, baris[-1]))
    return pilih


def main():
    saring, tutup = baca_log()
    beli = [s for s in saring if s[3] == "LOLOS"]
    dijual = {t[1] for t in tutup}
    hold = [b for b in beli if b[1] not in dijual]

    print("=" * 78)
    print("LAPORAN LIVE BAYANGAN — modal awal $20, mode shadow (order ditandatangani, tidak dikirim)")
    print("=" * 78)

    rows = jam_tabel()
    if rows:
        print("\nSALDO TIAP 20 MENIT")
        print("  menit  |  ekuitas  |     kas   | hold | beli | jual | menang | DD maks")
        print("  " + "-" * 70)
        for menit, p in rows:
            eq, kas, pos, nb, nt, nm, dd = p[1], p[2], p[3], p[4], p[5], p[6], p[7]
            print(f"  {menit:>5}  | ${float(eq):>8.2f} | ${float(kas):>8.2f} | {pos:>4} | {nb:>4} | {nt:>4} |"
                  f" {nm:>6} | {float(dd):>5.1f}%")

    print(f"\nKOIN DINILAI: {len(saring)}  |  DIBELI: {len(beli)}  |  DIJUAL: {len(tutup)}  |  MASIH HOLD: {len(hold)}")

    if beli:
        print("\nMANA KITA BELI  (SARING ... LOLOS — keputusan beli di menit ke-1)")
        print("  waktu     | pool       | skor  | dev      | penjual m1 | LP")
        print("  " + "-" * 70)
        for t, pool, skor, _, dev, jual, lp in beli:
            print(f"  {t[11:]}  | {pool} | {skor} | {dev:<8} | {jual:>10} | ${int(lp):,}")

    if tutup:
        print("\nMANA KITA JUAL  (TUTUP — posisi ditutup)")
        print("  waktu     | pool       | hasil | akun sesudah | menit | alasan")
        print("  " + "-" * 70)
        for t, pool, pct, _, ke, pnl, mnt, alasan in tutup:
            print(f"  {t[11:]}  | {pool} | {pct:>4}% | ${float(ke):>11.2f} | {float(mnt):>5.0f} | {alasan}")
        n = len(tutup)
        menang = sum(1 for x in tutup if float(x[5]) > 0)
        print(f"\n  menang {menang}/{n} = {100 * menang / n:.1f}%"
              f"  |  hasil bersih ${sum(float(x[5]) for x in tutup):+.2f}")

    if hold:
        print("\nMANA KITA HOLD  (sudah dibeli, belum ditutup saat uji berakhir)")
        print("  waktu beli | pool       | skor  | LP")
        print("  " + "-" * 70)
        for t, pool, skor, _, dev, jual, lp in hold:
            print(f"  {t[11:]}   | {pool} | {skor} | ${int(lp):,}")

    tolak = OrderedDict()
    for s in saring:
        if s[3] != "LOLOS":
            tolak[s[3]] = tolak.get(s[3], 0) + 1
    if tolak:
        print("\nYANG DITOLAK")
        for k, v in tolak.items():
            print(f"  {k:<16} {v:>5} koin")

    try:
        d = json.load(open(L + "/status.json"))
        a = d["akun"]
        print("\nSTATUS AKHIR DARI BOT SENDIRI")
        print(f"  mode          : {d['mode']}")
        print(f"  modal awal    : ${a['modal_awal']:.2f}")
        print(f"  ekuitas pasar : ${a['equity_harga_pasar']:.2f}")
        print(f"  kas           : ${a['kas']:.2f}  | posisi terbuka {a['posisi_terbuka']}")
        print(f"  trade         : {a['trade_dibeli']} dibeli, {a['trade_selesai']} selesai, {a['menang']} menang")
        print(f"  drawdown maks : {a['drawdown_maks_pct']:.1f}%  | stop darurat: {a['stop_darurat']}")
        print(f"  fraksi Kelly  : {a['kelly']:.4f}")
        print(f"  kill switch   : {d['kill_switch']}")
        f = d.get("filter", {})
        print(f"  filter        : dinilai {f.get('koin_dinilai')} | hasil dipelajari {f.get('hasil_koin_dipelajari')}"
              f" | ML dilatih ulang {f.get('ml_dilatih_ulang')} | akurasi {f.get('akurasi_data_belum_dilihat_500')}")
    except (OSError, KeyError, ValueError) as e:
        print(f"\nstatus.json tidak terbaca: {e}")

    try:
        n = 0
        for l in open(L + "/exec/journal.jsonl", encoding="utf8"):
            n += 1
        print(f"\n  catatan eksekutor: {n} peristiwa di exec/journal.jsonl (order dibangun & ditandatangani)")
    except OSError:
        pass


if __name__ == "__main__":
    main()
