#!/usr/bin/env bash
# Clean slate before switching EXEC_MODE to live, so the live record contains real trades only.
#
#   sudo deploy/bersihkan-live.sh              # clears the execution ledger only (the usual case)
#   sudo deploy/bersihkan-live.sh --reset-akun # also restarts the strategy account from START_USD
#
# Everything is backed up to <bot>/rust_live_backup_<stamp>.tar.gz first. Models, training labels and
# the adaptive filter's state (belajar.json) are never touched.
set -euo pipefail

BOT=${BOT:-/opt/rhc/bot}
LIVE=$BOT/rust_live
RESET_AKUN=no
[ "${1:-}" = --reset-akun ] && RESET_AKUN=yes

say(){ printf '%s\n' "$*"; }
[ -d "$LIVE" ] || { say "tidak ada $LIVE — belum pernah jalan, sudah bersih."; exit 0; }

# refuse to wipe anything while the bot is running: it would write its state back immediately
running=no
if command -v systemctl >/dev/null && systemctl is-active --quiet rhc-trader 2>/dev/null; then running=yes; fi
if command -v supervisorctl >/dev/null && supervisorctl status rhc-trader 2>/dev/null | grep -q RUNNING; then running=yes; fi
if [ "$running" = yes ]; then
    say "GAGAL: rhc-trader masih jalan. Hentikan dulu:"
    say "  systemctl stop rhc-trader     (atau)     supervisorctl stop rhc-trader"
    exit 1
fi

# an open real position must not be orphaned by wiping the ledger that tracks it
POS=$LIVE/exec/positions.json
# statuses are stored in snake_case ("buying", "open", "stuck"): parse the file; unreadable = refuse to wipe
if [ -s "$POS" ]; then
    if ! command -v python3 >/dev/null || python3 -c 'import json,sys
try: d = json.load(open(sys.argv[1]))
except Exception: sys.exit(0)
sys.exit(0 if any(p.get("status") in ("buying", "open", "stuck") for p in d.get("positions", {}).values()) else 1)' "$POS"; then
        say "GAGAL: masih ada posisi terbuka di $POS (atau berkasnya tidak terbaca)."
        say "Jual dulu (touch $LIVE/exec/FLATTEN_ALL, jalankan bot sampai kosong), baru bersihkan."
        exit 1
    fi
fi

STAMP=$(date -u +%Y%m%d_%H%M%S)
BAK=$BOT/rust_live_backup_$STAMP.tar.gz
say "1. cadangan -> $BAK"
tar -czf "$BAK" -C "$BOT" rust_live
say "   $(du -h "$BAK" | cut -f1)"

say "2. mengosongkan ledger eksekusi (catatan transaksi nyata)"
rm -rf "$LIVE/exec" "$LIVE/exec_shadow"
install -d -m 755 "$LIVE/exec"
say "   $LIVE/exec kosong"

if [ "$RESET_AKUN" = yes ]; then
    say "3. menyetel ulang akun strategi ke START_USD"
    # account snapshot, replay checkpoint, block cursor and the stored event log: without the store the
    # bot starts at the current chain tip with a fresh account instead of replaying old history
    rm -f "$LIVE/account_rust.json" "$LIVE/checkpoint.json" "$LIVE/state.json"
    rm -rf "$LIVE/store"
    say "   akun, checkpoint, state dan store dihapus — bot mulai dari blok terkini"
    say "   (pemanasan radar diulang: butuh waktu sampai koin terskor lagi)"
else
    say "3. akun strategi DIBIARKAN (pakai --reset-akun untuk menyetel ulang)"
fi

say ""
say "DIPERTAHANKAN: model (filter_cat/danger/pnaik), labels.jsonl, belajar.json, radar"
say "DICADANGKAN : $BAK"
say ""
say "Berikutnya:"
say "  1. pastikan EXEC_MODE=live dan EXEC_LIVE_CONFIRM terisi di $BOT/.env"
say "  2. rhc-trader exec selftest --env $BOT/.env      # wajib SELFTEST OK"
say "  3. start servicenya"
say ""
say "Mulai saat itu setiap baris di $LIVE/exec/ adalah transaksi nyata."
