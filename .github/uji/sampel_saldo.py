"""One line per sample of the strategy account, for the 20-minute balance report. Read-only."""
import json
import sys
import time

BOT = sys.argv[1] if len(sys.argv) > 1 else "/opt/rhc/bot"
try:
    d = json.load(open(BOT + "/rust_live/status.json"))
    a = d["akun"]
    print(int(time.time() * 1000), "%.4f" % a["equity_harga_pasar"], "%.4f" % a["kas"], a["posisi_terbuka"],
          a["trade_dibeli"], a["trade_selesai"], a["menang"], "%.3f" % a["drawdown_maks_pct"], d["last_block"])
except Exception:
    pass
