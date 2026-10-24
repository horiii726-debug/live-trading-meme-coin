#!/usr/bin/env bash
# mulai.sh — satu perintah dari VPS kosong sampai bot jalan:
#
#   curl -sSL https://raw.githubusercontent.com/horiii726-debug/live-trading-meme-coin/main/mulai.sh | sudo bash
#
# 1. ambil/perbarui kode   2. pasang (paket, Rust, build, service)
# 3. ditanya SEKALI: URL RPC + private key (tempel, Enter) — dilewati kalau sudah pernah diisi
# 4. self-test on-chain    5. bot dijalankan dan diperiksa tetap hidup    6. alamat wallet ditampilkan
#
# Aman dijalankan ulang: berfungsi juga sebagai pembaru (git pull + build ulang + restart).
# Tidak pernah membuat, membaca ulang, atau menampilkan private key.

# Seluruh isi dibungkus fungsi: bash membaca script utuh dulu sebelum menjalankan apa pun,
# jadi unduhan yang terputus di tengah tidak pernah menjalankan setengah script.
main(){
    set -Eeuo pipefail
    local REPO=https://github.com/horiii726-debug/live-trading-meme-coin.git
    say(){ printf '\n\033[1m>>> %s\033[0m\n' "$*"; }
    die(){ printf '\nGAGAL: %s\n' "$*" >&2; exit 1; }
    # any other failing command: say which one, instead of stopping silently
    trap 'printf "\nGAGAL: perintah ini gagal (baris %s): %s\n       Salin pesan di atas dan kirim ke yang membantu Anda.\n" "$LINENO" "$BASH_COMMAND" >&2' ERR

    [ "$(id -u)" -eq 0 ] || die "jalankan dengan sudo:  curl -sSL <url>/mulai.sh | sudo bash"
    command -v apt-get >/dev/null || die "VPS harus Ubuntu atau Debian (apt-get tidak ada)."
    # stdin adalah pipa dari curl; pertanyaan butuh keyboard
    { : </dev/tty; } 2>/dev/null || die "tidak ada terminal. Jalankan dari sesi SSH biasa."

    local u=${SUDO_USER:-root} h dir
    h=$(getent passwd "$u" | cut -d: -f6)
    dir=${RHC_DIR:-$h/live-trading-meme-coin}
    as_user(){ if [ "$u" = root ]; then "$@"; else sudo -u "$u" -H "$@"; fi; }
    # apt on a freshly booted VPS is often locked by unattended-upgrades: wait for it instead of failing
    local APT="apt-get -o DPkg::Lock::Timeout=600"
    export RHC_DARI_MULAI=1

    say "1/6  kode"
    local avail_kb mem_kb swap_kb
    avail_kb=$(df -Pk "$h" | awk 'NR==2{print $4}')
    [ "${avail_kb:-0}" -ge 5000000 ] || die "disk kosong tinggal $((${avail_kb:-0} / 1024)) MB; butuh minimal 5 GB (Rust + build + Python)."
    if ! command -v git >/dev/null || ! command -v curl >/dev/null; then
        $APT update -qq && DEBIAN_FRONTEND=noninteractive $APT install -y -qq git curl ca-certificates >/dev/null
    fi
    command -v nano >/dev/null || DEBIAN_FRONTEND=noninteractive $APT install -y -qq nano >/dev/null 2>&1 || true
    if [ -d "$dir/.git" ]; then
        as_user git -C "$dir" pull --ff-only -q || die "git pull gagal di $dir (ada berkas yang diubah? cek: cd $dir && git status)"
        echo "    diperbarui: $dir"
    else
        [ ! -e "$dir" ] || die "folder $dir sudah ada tapi bukan hasil git clone. Pindahkan/hapus dulu folder itu."
        as_user git clone -q "$REPO" "$dir" || die "git clone gagal (internet VPS?)"
        echo "    diunduh ke: $dir"
    fi
    cd "$dir"

    # cargo's release build (fat LTO, one codegen unit) needs about 3 GB: a small VPS without swap is OOM-killed
    mem_kb=$(awk '/^MemTotal:/{print $2}' /proc/meminfo)
    swap_kb=$(awk '/^SwapTotal:/{print $2}' /proc/meminfo)
    if [ $((mem_kb + swap_kb)) -lt 3500000 ] && [ ! -e /swapfile ]; then
        if [ "$avail_kb" -ge 9500000 ] && { fallocate -l 4G /swapfile 2>/dev/null || dd if=/dev/zero of=/swapfile bs=1M count=4096 status=none 2>/dev/null; } &&
           chmod 600 /swapfile && mkswap /swapfile >/dev/null 2>&1 && swapon /swapfile 2>/dev/null; then
            grep -q '^/swapfile ' /etc/fstab 2>/dev/null || echo '/swapfile none swap sw 0 0' >> /etc/fstab
            echo "    RAM $((mem_kb / 1024)) MB -> swap 4 GB dibuat supaya build tidak kehabisan memori"
        else
            rm -f /swapfile
            echo "    PERINGATAN: RAM $((mem_kb / 1024)) MB dan swap tidak bisa dibuat; build bisa gagal. Disarankan VPS RAM 4 GB."
        fi
    fi

    say "2/6  pasang (build Rust: 5-15 menit pada pemasangan pertama)"
    ./rhc.sh pasang </dev/tty

    say "3/6  RPC + wallet"
    if [ -s /opt/rhc/bot/.env ] && [ -s /opt/rhc/secrets/wallet.key ]; then
        echo "    sudah diatur sebelumnya — dilewati (ubah kapan saja: sudo ./rhc.sh isi)"
    else
        # plain questions instead of an editor: paste + Enter. The answers go into the same form that
        # deploy/pasang.sh validates; it blanks the secrets on success, and we delete the form on failure.
        local form=$dir/config/isi-disini.env n=0 rpc pk usd
        echo "    Menempel di PowerShell/terminal: klik kanan."
        while :; do
            echo ""
            echo "  1) Tempel URL RPC berbayar, lalu Enter."
            echo "     (kosongkan lalu Enter = pakai RPC publik gratis)"
            read -r -p "     RPC: " rpc </dev/tty
            echo ""
            echo "  2) Tempel PRIVATE KEY MetaMask (64 karakter, BUKAN 12 kata), lalu Enter."
            echo "     Ketikan memang TIDAK terlihat."
            read -r -s -p "     Private key: " pk </dev/tty; echo
            echo ""
            read -r -p "  3) Berapa USD yang Anda kirim ke wallet bot? [100]: " usd </dev/tty
            rpc=${rpc//[[:space:]]/}; pk=${pk//[[:space:]]/}; usd=${usd//[[:space:]]/}; usd=${usd#\$}
            [[ $usd =~ ^[0-9]+([.][0-9]+)?$ ]] || usd=100
            ( umask 077
              { printf 'RPC_API_KEY=%s\n' "$rpc"; printf 'WALLET_PRIVATE_KEY=%s\n' "$pk"
                printf 'SETORAN_USD=%s\nMODE=live\n' "$usd"
                grep -E '^(MAKS_USD_PER_BELI|STOP_RUGI_HARIAN_USD|MAKS_POSISI_TERBUKA)=' config/isi-disini.env.example
              } > "$form" )
            unset pk
            FORM=$form ./deploy/pasang.sh && break
            rm -f "$form"
            n=$((n + 1)); [ $n -lt 3 ] || die "isian salah 3x. Coba lagi nanti: cd $dir && sudo ./rhc.sh isi"
            echo ""; echo "    Ada isian yang salah (lihat pesan GAGAL di atas). Ditanya lagi ..."
        done
    fi

    say "4/6  self-test on-chain (tidak mengirim apa pun)"
    ./rhc.sh cek || die "self-test gagal — bot TIDAK dijalankan. Kirim pesan error di atas, atau cek RPC: sudo ./rhc.sh isi"

    say "5/6  menjalankan bot"
    local systemd=0 n0=0 n1=0
    [ -d /run/systemd/system ] && command -v systemctl >/dev/null && systemd=1
    if [ $systemd = 1 ] && systemctl is-active --quiet rhc-trader; then systemctl restart rhc-trader; echo "    di-restart dengan versi baru"
    elif [ $systemd = 0 ] && supervisorctl status rhc-trader 2>/dev/null | grep -q RUNNING; then supervisorctl restart rhc-trader >/dev/null
    else ./rhc.sh mulai; fi
    # a config error makes the service crash-loop (Restart=always): check that it stays up
    echo "    memeriksa bot tetap hidup (30 dtk) ..."
    if [ $systemd = 1 ]; then
        n0=$(systemctl show -p NRestarts --value rhc-trader 2>/dev/null || echo 0)
        sleep 30
        n1=$(systemctl show -p NRestarts --value rhc-trader 2>/dev/null || echo 0)
        if ! systemctl is-active --quiet rhc-trader || [ "${n1:-0}" != "${n0:-0}" ]; then
            journalctl -u rhc-trader -n 30 --no-pager 2>/dev/null | sed 's/^/    /' || true
            die "bot mati sendiri setelah dinyalakan (log di atas). Kirim log itu ke yang membantu Anda."
        fi
    else
        sleep 30
        if ! supervisorctl status rhc-trader 2>/dev/null | grep -q RUNNING; then
            tail -n 30 /var/log/portal/rhc-trader.log 2>/dev/null | sed 's/^/    /' || true
            die "bot mati sendiri setelah dinyalakan (log di atas). Kirim log itu ke yang membantu Anda."
        fi
    fi
    echo "    bot hidup"

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
