"""One 20-minute checkpoint, printed while the shadow run is going. Read-only."""
import json
import sys

BOT = sys.argv[1] if len(sys.argv) > 1 else "/opt/rhc/bot"
d = json.load(open(BOT + "/rust_live/status.json"))
a, f = d["akun"], d.get("filter", {})
print("  saldo    : ekuitas $%.2f | kas $%.2f | hold %d | DD maks %.1f%%"
      % (a["equity_harga_pasar"], a["kas"], a["posisi_terbuka"], a["drawdown_maks_pct"]))
print("  trade    : %d dibeli, %d selesai, %d menang" % (a["trade_dibeli"], a["trade_selesai"], a["menang"]))
print("  rantai   : blok %d | lag %d | mode aman %s | eth $%.0f"
      % (d["last_block"], d["lag_blocks"], d.get("mode_aman"), d["eth_usd"]))
print("  filter   : dinilai %s | hasil dipelajari %s | ML latih ulang %s | akurasi %s"
      % (f.get("koin_dinilai"), f.get("hasil_koin_dipelajari"), f.get("ml_dilatih_ulang"),
         f.get("akurasi_data_belum_dilihat_500")))
print("  kelly    : %.4f | stop darurat %s | kill switch %s"
      % (a["kelly"], a["stop_darurat"], d["kill_switch"]["aktif"]))
