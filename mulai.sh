#!/usr/bin/env bash
# mulai.sh — satu perintah dari VPS kosong sampai bot jalan:
#
#   curl -sSL https://raw.githubusercontent.com/horiii726-debug/live-trading-meme-coin/main/mulai.sh | sudo bash
#
# 1. ambil/perbarui kode   2. pasang (paket, Rust, build, service)
# 3. ditanya SEKALI: URL RPC + private key (tempel, Enter) — dilewati kalau sudah pernah diisi
# 4. self-test on-chain dengan binary baru, baru setelah lulus binary dipasang
# 5. bot dijalankan dan diperiksa tetap hidup    6. alamat wallet ditampilkan
#
# Aman dijalankan ulang: berfungsi juga sebagai pembaru (git pull + build ulang + restart).
# Tidak pernah membuat, membaca ulang, atau menampilkan private key.

# the form holding the typed key, deleted on exit unless deploy/pasang.sh accepted (and blanked) it
FORM_TMP=""

# Seluruh isi dibungkus fungsi: bash membaca script utuh dulu sebelum menjalankan apa pun,
# jadi unduhan yang terputus di tengah tidak pernah menjalankan setengah script.
main(){
    set -Eeuo pipefail
    local REPO=https://github.com/horiii726-debug/live-trading-meme-coin.git
    say(){ printf '\n\033[1m>>> %s\033[0m\n' "$*"; }
    die(){ printf '\nGAGAL: %s\n' "$*" >&2; exit 1; }
    # any other failing command: say which one, instead of stopping silently
    trap 'printf "\nGAGAL: perintah ini gagal (baris %s): %s\n       Salin pesan di atas dan kirim ke yang membantu Anda.\n" "$LINENO" "$BASH_COMMAND" >&2' ERR
    trap '[ -z "$FORM_TMP" ] || rm -f "$FORM_TMP"' EXIT
    trap 'exit 130' INT TERM

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
    local avail_kb mem_kb swap_kb i
    avail_kb=$(df -Pk "$h" | awk 'NR==2{print $4}')
    [ "${avail_kb:-0}" -ge 5000000 ] || die "disk kosong tinggal $((${avail_kb:-0} / 1024)) MB; butuh minimal 5 GB (Rust + build + Python)."
    # a VPS bought minutes ago is still provisioning (cloud-init) and updating itself: let that finish first
    if command -v cloud-init >/dev/null && cloud-init status 2>/dev/null | grep -q running; then
        echo "    VPS masih menyiapkan diri (cloud-init) -> menunggu ..."
        timeout 900 cloud-init status --wait >/dev/null 2>&1 || true
    fi
    if ! command -v git >/dev/null || ! command -v curl >/dev/null; then
        # `apt-get update` ignores DPkg::Lock::Timeout (it takes the lists lock): retry
        for i in $(seq 1 90); do $APT update -qq && break; [ "$i" -lt 90 ] || die "apt-get update gagal terus 15 menit. Jalankan lagi nanti."; sleep 10; done
        DEBIAN_FRONTEND=noninteractive $APT install -y -qq git curl ca-certificates >/dev/null
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
        echo "    sudah diatur sebelumnya — dilewati (ubah kapan saja: cd $dir && sudo ./rhc.sh isi)"
    else
        # Plain questions instead of an editor: paste + Enter. Each answer is checked before the next question, then
        # all go into the form deploy/pasang.sh validates against the network; it blanks the secrets on success.
        local n=0 rpc pk usd ok spasi
        # drop what was typed while the build ran, and the extra newline some copies carry: a stray Enter would
        # otherwise answer the next question (an empty key prompt, then the key pasted visibly at question 3)
        flush(){ while read -r -t 0.2 -n 10000 _ </dev/tty; do :; done; }
        echo "    Menempel di PowerShell/terminal: klik kanan."
        while :; do
            while :; do
                flush; echo ""
                echo "  1) Tempel URL RPC berbayar, lalu Enter."
                echo "     (kosongkan lalu Enter = pakai RPC publik gratis)"
                read -r -p "     RPC: " rpc </dev/tty
                rpc=${rpc//[[:space:]]/}
                if [[ $rpc =~ ^(0x)?[0-9a-fA-F]{64}$ ]]; then
                    printf '\033[2J\033[H'   # the key was just shown in clear: wipe the screen
                    echo "     Itu PRIVATE KEY, bukan URL RPC (layar sudah dibersihkan). Private key ditempel di pertanyaan 2."
                    continue
                fi
                case "$rpc" in
                    ""|https://*) break;;
                    wss://*|ws://*) echo "     Itu URL WebSocket (wss://). Salin URL HTTP-nya (diawali https://) dari dasbor RPC.";;
                    *) echo "     URL harus diawali https:// — salin URL utuh dari dasbor RPC.";;
                esac
            done
            echo ""
            echo "  2) Tempel PRIVATE KEY MetaMask (64 karakter, BUKAN 12 kata), lalu Enter."
            echo "     Ketikan memang TIDAK terlihat."
            while :; do
                flush
                read -r -s -p "     Private key: " pk </dev/tty; echo
                spasi=${pk//[^ ]/}; pk=${pk//[[:space:]]/}
                [[ $pk =~ ^(0x)?[0-9a-fA-F]{64}$ ]] && break
                if [ ${#spasi} -ge 10 ]; then
                    echo "     Itu SEED PHRASE (kata-kata) — JANGAN dipakai. Ambil Private key: MetaMask > Account details > Show private key."
                elif [ -z "$pk" ]; then
                    echo "     Kosong. Tempel private key-nya (klik kanan), lalu Enter."
                else
                    echo "     Bukan private key: harus 64 karakter 0-9/a-f (yang ini ${#pk} karakter). Coba lagi."
                fi
            done
            echo ""
            while :; do
                flush
                read -r -p "  3) Berapa dolar (USD) yang Anda kirim ke wallet bot? Angka saja, mis. 100 atau 1000 [100]: " usd </dev/tty
                usd=${usd//[[:space:]]/}; usd=${usd#\$}; usd=${usd%[Uu][Ss][Dd]}; usd=${usd:-100}
                # 1.000 / 1,000 is one thousand here; read as 1.0 the strategy could never size a position
                if [[ $usd =~ ^[0-9]{1,3}([.,][0-9]{3})+$ ]]; then usd=${usd//[.,]/}; fi
                usd=${usd/,/.}
                if [[ $usd =~ ^[0-9]+([.][0-9]{1,2})?$ ]] && [ "${usd%%.*}" -ge 10 ]; then
                    read -r -p "     Setoran = \$$usd. Benar? (y/n) [y]: " ok </dev/tty
                    case "${ok:-y}" in [yY]*) break;; esac
                else
                    echo "     Isi jumlah dolar dengan angka saja, minimal 10 (contoh: 100 atau 1000)."
                fi
            done
            FORM_TMP=$dir/config/isi-disini.env
            ( umask 077
              { printf 'RPC_API_KEY=%s\n' "$rpc"; printf 'WALLET_PRIVATE_KEY=%s\n' "$pk"
                printf 'SETORAN_USD=%s\nMODE=live\n' "$usd"
                grep -E '^(MAKS_USD_PER_BELI|STOP_RUGI_HARIAN_USD|MAKS_POSISI_TERBUKA)=' config/isi-disini.env.example
              } > "$FORM_TMP" )
            unset pk
            if FORM=$FORM_TMP ./deploy/pasang.sh; then FORM_TMP=""; break; fi
            rm -f "$FORM_TMP"
            n=$((n + 1)); [ $n -lt 3 ] || die "isian ditolak 3x. Coba lagi nanti: cd $dir && sudo ./rhc.sh isi"
            echo ""; echo "    Ada isian yang ditolak (lihat pesan GAGAL di atas). Ditanya lagi ..."
        done
    fi

    say "4/6  self-test on-chain dengan versi baru (tidak mengirim apa pun)"
    local running=0 systemd=0 n0=0 n1=0
    [ -d /run/systemd/system ] && command -v systemctl >/dev/null && systemd=1
    if [ $systemd = 1 ]; then systemctl is-active --quiet rhc-trader && running=1
    else supervisorctl status rhc-trader 2>/dev/null | grep -q RUNNING && running=1; fi
    # a rate-limited RPC (429, measured: constant on the public RPC while the old bot runs) fails a single try
    local cek_ok=0
    for i in 1 2 3 4; do
        if BIN=$dir/target/release/rhc-trader ./rhc.sh cek; then cek_ok=1; break; fi
        if [ "$i" -lt 4 ]; then echo "    self-test belum lulus (RPC sibuk?) — dicoba lagi dalam 20 dtk ($i/3) ..."; sleep 20; fi
    done
    if [ $cek_ok = 0 ]; then
        [ $running = 1 ] && die "self-test versi baru gagal 4x — versi baru TIDAK dipasang, bot lama tetap jalan. Kirim pesan error di atas, atau periksa RPC: cd $dir && sudo ./rhc.sh isi"
        die "self-test gagal 4x — bot TIDAK dijalankan. Kirim pesan error di atas, atau periksa RPC: cd $dir && sudo ./rhc.sh isi"
    fi
    install -m 755 "$dir/target/release/rhc-trader" /opt/rhc/bin/rhc-trader

    say "5/6  menjalankan bot"
    if [ $running = 1 ]; then
        if [ $systemd = 1 ]; then systemctl restart rhc-trader; else supervisorctl restart rhc-trader >/dev/null; fi
        echo "    di-restart dengan versi baru"
    else
        ./rhc.sh mulai
    fi
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
