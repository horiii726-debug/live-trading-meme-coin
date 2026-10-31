#!/usr/bin/env bash
# Test only: every rhc.sh command and the README's emergency files on a real install. The wallet holds no ETH.
set -uo pipefail
R=/root/live-trading-meme-coin
B=/opt/rhc/bot
gagal=0
hasil(){ if [ "$2" = 0 ]; then echo "MENU $1: LULUS"; else echo "MENU $1: GAGAL"; gagal=$((gagal + 1)); fi; }
rhc(){ sudo timeout "${T:-120}" bash -c "cd $R && ./rhc.sh $*" 2>&1; }
cek(){ local nama=$1 pola=$2 out; shift 2; printf '\n=== %s\n' "$nama"; out=$(rhc "$@"); echo "$out" | tail -14 | cut -c1-150; echo "$out" | grep -qE "$pola"; hasil "$nama" $?; }
aktif(){ for _ in $(seq 1 60); do sleep 3; [ "$(systemctl is-active rhc-trader)" = active ] && return 0; done; return 1; }
stat(){ sudo python3 -c "import json;print(json.load(open('$B/rust_live/exec/exec_status.json')).get('$1'))"; }

printf '\n=== service\n'
systemctl is-enabled rhc-trader; [ "$(systemctl is-enabled rhc-trader)" = enabled ]; hasil "menyala lagi setelah reboot (enabled)" $?
cek status 'service   : JALAN' status
cek dompet 'address 0x[0-9a-f]{40}' dompet
T=200 cek cek 'SELFTEST OK' cek
printf '\n=== log\n'; out=$(sudo timeout 6 bash -c "cd $R && ./rhc.sh log" 2>&1); echo "$out" | tail -4 | cut -c1-150; echo "$out" | grep -q 'Ctrl+C'; hasil log $?

cek stop berhenti stop
[ "$(systemctl is-active rhc-trader)" = inactive ]; hasil "stop benar-benar menghentikan service" $?
cek 'status saat berhenti' 'service   : berhenti' status
cek mulai 'rhc-trader' mulai
aktif; hasil "mulai menjalankan service" $?
sleep 20

printf '\n=== STOP_BUYING dari README\n'
sudo touch $B/rust_live/exec/STOP_BUYING; sleep 4; v=$(stat buy_block_reason); echo "$v"; echo "$v" | grep -q STOP_BUYING; hasil "STOP_BUYING memblok beli" $?
sudo rm -f $B/rust_live/exec/STOP_BUYING; sleep 4; v=$(stat buying); echo "buying=$v"; [ "$v" = True ]; hasil "beli normal lagi setelah STOP_BUYING dihapus" $?

printf '\n=== jual-semua\n'
rhc jual-semua --ya | tail -3; aktif; hasil "jual-semua: bot jalan" $?
sudo test -e $B/rust_live/exec/FLATTEN_ALL; hasil "jual-semua: FLATTEN_ALL ada" $?
sleep 15; v=$(stat buy_block_reason); echo "$v"; echo "$v" | grep -q FLATTEN; hasil "jual-semua: beli diblok" $?
rhc lanjut --ya | tail -2; aktif; hasil "lanjut setelah jual-semua: bot jalan" $?
sudo test ! -e $B/rust_live/exec/FLATTEN_ALL; hasil "lanjut menghapus FLATTEN_ALL" $?

printf '\n=== kill switch dari sisi pemilik\n'
echo "$(date -u +%FT%TZ) | uji pemilik" | sudo -u rhc tee $B/rust_live/KILL_SWITCH >/dev/null
sudo systemctl restart rhc-trader; aktif; sleep 20
cek 'status menampilkan kill switch' 'KILL SWITCH AKTIF' status
o=$(sudo bash /etc/profile.d/rhc-kill-switch.sh 2>&1); echo "$o" | head -3 | cut -c1-120; echo "$o" | grep -q 'KILL SWITCH AKTIF'; hasil "banner login menampilkan kill switch" $?
sudo python3 -c "import json;print(json.load(open('$B/rust_live/status.json'))['kill_switch'])" | grep -q "'aktif': True"; hasil "status.json kill switch aktif" $?
rhc lanjut --ya | tail -2; aktif; sleep 20
sudo test ! -e $B/rust_live/KILL_SWITCH; hasil "lanjut membuka kill switch" $?

printf '\n=== atur (tanya-jawab), jawaban bawaan\n'
out=$(printf '\n100\ny\n0\n0\n6\nlive\ny\n' | sudo bash -c "cd $R && ./rhc.sh atur" 2>&1); echo "$out" | tail -12 | cut -c1-150
echo "$out" | grep -q 'Tersimpan'; hasil atur $?
sudo grep -q '^EXEC_MODE=live' $B/.env && sudo grep -q '^EXEC_LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK' $B/.env && sudo grep -q '^LAT_BLOCKS=3' $B/.env; hasil "atur menulis mode live dan LAT_BLOCKS=3" $?
sudo stat -c '%a %U' $B/.env /opt/rhc/secrets/wallet.key
[ "$(sudo stat -c %a $B/.env)" = 600 ] && [ "$(sudo stat -c %a /opt/rhc/secrets/wallet.key)" = 600 ]; hasil "izin .env dan kunci 600" $?
sudo systemctl restart rhc-trader; aktif; hasil "bot jalan setelah atur" $?

printf '\n=== bersih\n'
rhc stop >/dev/null
out=$(printf 'n\n' | sudo bash -c "cd $R && ./rhc.sh bersih" 2>&1); echo "$out" | tail -5 | cut -c1-150; echo "$out" | grep -q 'DIPERTAHANKAN'; hasil bersih $?
rhc mulai >/dev/null; aktif; sleep 30
n=$(systemctl show -p NRestarts --value rhc-trader); [ "${n:-0}" -lt 3 ]; hasil "bot jalan setelah bersih (restart $n)" $?
printf '\n=== rahasia tidak ada di log\n'
kunci=$(sudo cat /opt/rhc/secrets/wallet.key)
sudo grep -rlF "$kunci" /opt/rhc/bot /var/log /etc/systemd 2>/dev/null | grep -v '^/opt/rhc/secrets' ; [ -z "$(sudo grep -rlF "$kunci" /opt/rhc/bot /var/log /etc/systemd 2>/dev/null)" ]; hasil "private key tidak tertulis di log atau data bot" $?

printf '\n=== ukuran terpasang\n'
sudo du -sh /opt/rhc /opt/rhc/bin/* /root/live-trading-meme-coin /root/.cargo /root/.rustup 2>/dev/null
free -m | head -2; df -h / | tail -1
echo "gagal: $gagal"
exit "$gagal"
