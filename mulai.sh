#!/usr/bin/env bash
# mulai.sh — satu perintah dari VPS kosong sampai bot jalan:
#
#   curl -sSL https://raw.githubusercontent.com/horiii726-debug/live-trading-meme-coin/main/mulai.sh | sudo bash
#
# 1. ambil/perbarui kode   2. pasang (paket, Rust, build, service)
# 3. formulir RPC + private key terbuka SEKALI (dilewati kalau sudah pernah diisi)
# 4. self-test on-chain    5. bot dijalankan    6. alamat wallet ditampilkan
#
# Aman dijalankan ulang: berfungsi juga sebagai pembaru (git pull + build ulang + restart).
# Tidak pernah membuat, membaca ulang, atau menampilkan private key.

# Seluruh isi dibungkus fungsi: bash membaca script utuh dulu sebelum menjalankan apa pun,
# jadi unduhan yang terputus di tengah tidak pernah menjalankan setengah script.
main(){
    set -euo pipefail
    local REPO=https://github.com/horiii726-debug/live-trading-meme-coin.git
    say(){ printf '\n\033[1m>>> %s\033[0m\n' "$*"; }
    die(){ printf '\nGAGAL: %s\n' "$*" >&2; exit 1; }

    [ "$(id -u)" -eq 0 ] || die "jalankan dengan sudo:  curl -sSL <url>/mulai.sh | sudo bash"
    # stdin adalah pipa dari curl; formulir dan pertanyaan butuh keyboard
    [ -r /dev/tty ] || die "tidak ada terminal. Jalankan dari sesi SSH biasa."

    local u=${SUDO_USER:-root} h dir
    h=$(getent passwd "$u" | cut -d: -f6)
    dir=${DIR:-$h/live-trading-meme-coin}

    say "1/6  kode"
    if ! command -v git >/dev/null || ! command -v curl >/dev/null; then
        apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq git curl ca-certificates nano >/dev/null
    fi
    command -v nano >/dev/null || DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nano >/dev/null 2>&1 || true
    if [ -d "$dir/.git" ]; then
        sudo -u "$u" git -C "$dir" pull --ff-only -q || die "git pull gagal di $dir"
        echo "    diperbarui: $dir"
    else
        sudo -u "$u" git clone -q "$REPO" "$dir" || die "git clone gagal"
        echo "    diunduh ke: $dir"
    fi
    cd "$dir"

    say "2/6  pasang (build Rust: 5-15 menit pada pemasangan pertama)"
    ./rhc.sh pasang </dev/tty

    say "3/6  RPC + wallet"
    if [ -s /opt/rhc/bot/.env ] && [ -s /opt/rhc/secrets/wallet.key ]; then
        echo "    sudah diatur sebelumnya — dilewati (ubah kapan saja: sudo ./rhc.sh isi)"
    else
        local n=0
        until ./rhc.sh isi </dev/tty; do
            n=$((n + 1)); [ $n -lt 3 ] || die "formulir gagal 3x. Perbaiki lalu jalankan: cd $dir && sudo ./rhc.sh isi"
            echo ""; echo "    Ada isian yang salah (lihat pesan di atas). Formulir dibuka lagi ..."
        done
    fi

    say "4/6  self-test on-chain (tidak mengirim apa pun)"
    ./rhc.sh cek || die "self-test gagal — bot TIDAK dijalankan. Kirim pesan error di atas, atau cek RPC: sudo ./rhc.sh isi"

    say "5/6  menjalankan bot"
    if systemctl is-active --quiet rhc-trader 2>/dev/null; then systemctl restart rhc-trader; echo "    di-restart dengan versi baru"
    elif supervisorctl status rhc-trader 2>/dev/null | grep -q RUNNING; then supervisorctl restart rhc-trader
    else ./rhc.sh mulai; fi

    say "6/6  wallet"
    ./rhc.sh dompet || true
    ./rhc.sh status || true

    cat <<EOF

==============================================================
 SELESAI. Bot jalan sendiri, tetap jalan walau SSH ditutup,
 dan menyala lagi otomatis kalau VPS restart.

 Kirim ETH ke alamat wallet di atas (Robinhood Chain 4663).
 Dengan saldo 0 bot hanya melewati pembelian; begitu terisi
 ia mulai trading dalam hitungan detik.

 Pantau  : cd $dir && sudo ./rhc.sh log
 Status  : cd $dir && sudo ./rhc.sh status
 Stop beli: sudo touch /opt/rhc/bot/rust_live/exec/STOP_BUYING
 Jual semua: sudo touch /opt/rhc/bot/rust_live/exec/FLATTEN_ALL
==============================================================
EOF
}
main "$@"; exit
