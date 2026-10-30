#!/usr/bin/env bash
# Checks after the one-line install, on the CI VM.
#   periksa.sh <expected DOH_URL value, may be empty> <home dir of the user who ran it>
set -uo pipefail
want_doh=$1; home=$2
E=/opt/rhc/bot/.env; K=/opt/rhc/secrets/wallet.key; L=/opt/rhc/bot/rust_live/live.log
fails=0
ok(){ printf 'OK     %s\n' "$*"; }
bad(){ printf 'GAGAL  %s\n' "$*"; fails=$((fails + 1)); }
chk(){ local what=$1; shift; if "$@"; then ok "$what"; else bad "$what"; fi; }

chk "wallet.key milik rhc, mode 600"      [ "$(sudo stat -c '%U %a' $K)" = "rhc 600" ]
chk "wallet.key berisi 64 karakter hex"   sudo sh -c "tr -d '\n' < $K | grep -Eqx '[0-9a-fA-F]{64}'"
chk ".env milik rhc, mode 600"            [ "$(sudo stat -c '%U %a' $E)" = "rhc 600" ]
chk "folder secrets milik rhc"            [ "$(sudo stat -c '%U' /opt/rhc/secrets)" = rhc ]
for kv in EXEC_MODE=live EXEC_LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK EXEC_KEY_FILE=$K START_USD=1000 \
          EXEC_MAX_ORDER_USD=0 EXEC_DAILY_LOSS_LIMIT_USD=0 EXEC_MAX_OPEN_POSITIONS=6 \
          "RPC_URLS=${UJI_RPC_URLS:-https://rpc.mainnet.chain.robinhood.com}" "DOH_URL=$want_doh" \
          KELLY_SEED=/opt/rhc/bot/kelly_seed_backtest_last100.json LAT_BLOCKS=3 BELAJAR=1 CATBOOST_BIN=/opt/rhc/bin/catboost; do
    chk ".env: $kv" sudo grep -qx -- "$kv" $E
done
repo=$home/live-trading-meme-coin
chk "private key dikosongkan dari formulir" sudo grep -qx 'WALLET_PRIVATE_KEY=' "$repo/config/isi-disini.env"
owner=$(stat -c %U "$repo")
chk "tidak ada berkas git yang berubah" [ -z "$(sudo -u "$owner" git -C "$repo" status --porcelain)" ]
chk "model + riwayat dev + data dasar ada di /opt/rhc/bot" sudo test -s /opt/rhc/bot/dev_history.json -a -s /opt/rhc/bot/filter_cat.json -a -s /opt/rhc/bot/latih_dasar.csv
chk "service menyala otomatis saat boot" [ "$(systemctl is-enabled rhc-trader)" = enabled ]
chk "kill switch TIDAK aktif pada bot sehat" sudo test ! -e /opt/rhc/bot/rust_live/KILL_SWITCH
chk "peringatan kill switch saat login SSH terpasang" test -s /etc/profile.d/rhc-kill-switch.sh
chk "program CatBoost (C++) terpasang dan asli" sh -c 'echo "478dc57f4c19de205b19b709fd4c6af93f79753dc462b6bb49d33c920dddec75  /opt/rhc/bin/catboost" | sha256sum -c --status'
chk "tidak ada sisa Python lama" [ ! -e /opt/rhc/venv ] && [ ! -e /etc/systemd/system/rhc-retrain.timer ]
chk "filter adaptif aktif di log bot" sudo grep -q "filter adaptif AKTIF" /opt/rhc/bot/rust_live/live.log

echo "... bot dibiarkan jalan 120 dtk"
n0=$(systemctl show -p NRestarts --value rhc-trader); sleep 120; n1=$(systemctl show -p NRestarts --value rhc-trader)
chk "service masih jalan setelah 2 menit" systemctl is-active --quiet rhc-trader
chk "tidak crash/restart (NRestarts $n0 -> $n1)" [ "$n0" = "$n1" ]
chk "executor live menyala (live.log)" sudo grep -q 'EXEC: executor started: mode=live' $L
chk "aturan \"dev pernah rug\" aktif dengan riwayat bawaan" sudo grep -Eq 'aturan dev AKTIF: [0-9]{3,} dev dari dev_history.json, [0-9]{2,} dev tercatat pernah rug' $L
chk "status.json ditulis" sudo test -s /opt/rhc/bot/rust_live/status.json
chk "rhc.sh status jalan" sudo "$repo/rhc.sh" status

echo "----------------------------------------------------------------- live.log (40 baris terakhir)"
sudo tail -n 40 $L
echo "----------------------------------------------------------------- journal (20 baris terakhir)"
sudo journalctl -u rhc-trader -n 20 --no-pager
echo "-----------------------------------------------------------------"
[ $fails = 0 ] && echo "SEMUA CEK LULUS" || { echo "$fails CEK GAGAL"; exit 1; }
