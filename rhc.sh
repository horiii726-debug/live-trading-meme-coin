#!/usr/bin/env bash
# rhc.sh — satu berkas untuk memasang, mengatur, menjalankan dan memantau rhc-trader.
#
#   sudo ./rhc.sh                 menu
#   sudo ./rhc.sh pasang          pasang semuanya (paket, Rust, build, service)
#   sudo ./rhc.sh atur            isi RPC + wallet MetaMask + batas + mode (tanya-jawab)
#   sudo ./rhc.sh isi             sama, lewat formulir config/isi-disini.env (tinggal tempel)
#   sudo ./rhc.sh cek             self-test on-chain (tidak mengirim apa pun)
#   sudo ./rhc.sh dompet          alamat wallet bot dan saldonya
#   sudo ./rhc.sh mulai | stop | status | log
#   sudo ./rhc.sh bersih          kosongkan catatan sebelum pindah ke live
#
# Tidak pernah membuat, membaca ulang, atau menampilkan private key.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PREFIX=${PREFIX:-/opt/rhc}
BOT=${BOT:-$PREFIX/bot}
SECRETS=${SECRETS:-$PREFIX/secrets}
KEYFILE=${KEYFILE:-$SECRETS/wallet.key}
BIN=${BIN:-$PREFIX/bin/rhc-trader}
PUBLIC_RPC=https://rpc.mainnet.chain.robinhood.com
CHAIN_HEX=0x1237   # 4663

say(){ printf '%s\n' "$*"; }
# escape a value for the right-hand side of a sed s### command ("&" means "the match" there, "#" ends it)
sedrepl(){ printf '%s' "$1" | sed -e 's/[\\&#]/\\&/g'; }
die(){ printf '\nGAGAL: %s\n' "$*" >&2; exit 1; }
hr(){ printf '%s\n' "--------------------------------------------------------------"; }
need_root(){ [ "$(id -u)" -eq 0 ] || die "jalankan dengan sudo: sudo ./rhc.sh $*"; }
ask(){ local p=$1 d=${2:-} a; read -r -p "$p${d:+ [$d]}: " a; printf '%s' "${a:-$d}"; }
yes_no(){ local a; read -r -p "$1 (y/n) [n]: " a; [ "${a,,}" = y ]; }
have_bin(){ [ -x "$BIN" ] || BIN="$ROOT/target/release/rhc-trader"; [ -x "$BIN" ]; }

# ---------------------------------------------------------------- service manager
svc_kind(){
    if [ -d /run/systemd/system ] && command -v systemctl >/dev/null; then echo systemd
    elif command -v supervisorctl >/dev/null; then echo supervisor
    else echo none; fi
}
svc_running(){
    case "$(svc_kind)" in
        systemd)    systemctl is-active --quiet rhc-trader 2>/dev/null;;
        supervisor) supervisorctl status rhc-trader 2>/dev/null | grep -q RUNNING;;
        *) return 1;;
    esac
}

# ---------------------------------------------------------------- RPC
# Some networks answer DNS for the RPC host with a block page (Indonesia's Internet Positif does). The bot then needs
# DOH_URL set (it resolves through Cloudflare at 1.1.1.1 itself); DOH_ARG makes these curl probes do the same.
DOH=https://1.1.1.1/dns-query
DOH_ARG=""
rpc(){ curl -sS -m 20 ${DOH_ARG:+--doh-url "$DOH_ARG"} -X POST "$1" -H 'content-type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[]}" 2>/dev/null || true; }
cek_dns(){
    [ "$(periksa_rpc "$PUBLIC_RPC")" = ok ] && return 0
    DOH_ARG=$DOH
    if [ "$(periksa_rpc "$PUBLIC_RPC")" = ok ]; then
        say "    DNS VPS ini membelokkan alamat RPC (diblokir DNS) -> bot memakai DNS-over-HTTPS (1.1.1.1)"
    else
        DOH_ARG=""
    fi
}
# apt on a freshly booted VPS is often locked by unattended-upgrades: wait for it instead of failing
APT="apt-get -o DPkg::Lock::Timeout=600"
apt_update(){   # `apt-get update` takes /var/lib/apt/lists/lock, which DPkg::Lock::Timeout does not cover: retry
    local i
    for i in $(seq 1 90); do
        $APT update -qq && return 0
        [ "$i" = 1 ] && say "    apt sedang dipakai update otomatis Ubuntu -> menunggu (bisa beberapa menit) ..."
        sleep 10
    done
    die "apt-get update gagal terus selama 15 menit (lihat pesan E: di atas). Jalankan lagi nanti."
}

# Dukungan debug_traceCall ditentukan dari KODE error JSON-RPC, bukan teks pesannya:
# balasan 429 tidak boleh terbaca sebagai "ada".
trace_support(){
    case "$(rpc "$1" debug_traceCall)" in
        *'-32601'*)        echo tidak;;
        *'"error"'*429*)   echo tidak-diketahui;;
        *'"jsonrpc"'*)     echo ya;;
        *)                 echo tidak-diketahui;;
    esac
}
periksa_rpc(){   # periksa_rpc <url> -> ok | salah-chain:<id> | bukan-evm | mati
    local r; r=$(rpc "$1" eth_chainId)
    case "$r" in
        *"\"result\":\"$CHAIN_HEX\""*) echo ok;;
        *'"result":"0x'*) echo "salah-chain:$(printf '%s' "$r" | sed -n 's/.*"result":"\([^"]*\)".*/\1/p')";;
        "") echo mati;;
        *)  echo bukan-evm;;
    esac
}

# ================================================================= PASANG
cmd_pasang(){
    need_root pasang
    hr; say " 1/4  paket sistem"; hr
    if command -v apt-get >/dev/null; then
        apt_update
        DEBIAN_FRONTEND=noninteractive $APT install -y --no-install-recommends \
            build-essential pkg-config libssl-dev python3-venv ca-certificates curl git cron >/dev/null
    fi
    say "    siap"

    hr; say " 2/4  Rust"; hr
    local u h cargo
    u=${SUDO_USER:-root}; h=$(getent passwd "$u" | cut -d: -f6)
    cargo=$(command -v cargo || echo "$h/.cargo/bin/cargo")
    if [ ! -x "$cargo" ]; then
        say "    memasang rustup untuk $u ..."
        if [ "$u" = root ]; then
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
        else
            sudo -u "$u" -H sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal" >/dev/null
        fi
        cargo="$h/.cargo/bin/cargo"
    fi
    # cargo is a rustup proxy: it must run as the user it was installed for (as root it finds no toolchain)
    if [ "$u" = root ]; then "$cargo" --version; else sudo -u "$u" -H "$cargo" --version; fi | sed 's/^/    /'

    hr; say " 3/4  build (beberapa menit)"; hr
    id rhc >/dev/null 2>&1 || useradd --system --home "$PREFIX" --shell /usr/sbin/nologin rhc
    install -d -o rhc -g rhc -m 755 "$PREFIX" "$PREFIX/bin" "$BOT" "$PREFIX/training"
    install -d -o rhc -g rhc -m 700 "$SECRETS"
    if [ "$u" = root ]; then "$cargo" build --release --manifest-path "$ROOT/Cargo.toml"
    else sudo -u "$u" -H "$cargo" build --release --manifest-path "$ROOT/Cargo.toml"; fi
    # mulai.sh installs the new binary itself, only after it passed the on-chain self-test
    [ "${RHC_DARI_MULAI:-}" = 1 ] || install -m 755 "$ROOT/target/release/rhc-trader" "$PREFIX/bin/rhc-trader"
    for f in "$ROOT"/models/*; do [ -e "$BOT/$(basename "$f")" ] || install -o rhc -g rhc -m 644 "$f" "$BOT/"; done
    cp -r "$ROOT"/training/* "$PREFIX/training/" && chown -R rhc:rhc "$PREFIX/training"
    # Python is only for the daily retrain (the bot itself never runs it): a failed pip must not stop the install
    if { [ -x "$PREFIX/venv/bin/python" ] || python3 -m venv "$PREFIX/venv"; } &&
       "$PREFIX/venv/bin/pip" install --quiet --upgrade pip &&
       "$PREFIX/venv/bin/pip" install --quiet numpy pandas scikit-learn catboost; then
        :
    else
        say "    PERINGATAN: paket Python gagal dipasang -> latih ulang harian tidak jalan (bot tetap jalan)."
        say "               Ulangi nanti: sudo ./rhc.sh pasang"
    fi
    say "    terpasang di $PREFIX"

    hr; say " 4/4  service"; hr
    case "$(svc_kind)" in
      systemd)
        install -m 644 "$ROOT"/deploy/*.service "$ROOT"/deploy/*.timer /etc/systemd/system/
        systemctl daemon-reload; say "    systemd";;
      supervisor)
        install -d -m 755 /etc/supervisor/conf.d /var/log/portal
        install -m 644 "$ROOT/deploy/rhc-trader.supervisor.conf" /etc/supervisor/conf.d/rhc-trader.conf
        supervisorctl reread >/dev/null 2>&1 || true
        supervisorctl update >/dev/null 2>&1 || true
        if command -v crontab >/dev/null; then
            { crontab -l 2>/dev/null | grep -v latih_ulang.py || true
              echo "0 3 * * * cd $PREFIX/training && BOT_DIR=$BOT DATA_DIR=$PREFIX/training/data $PREFIX/venv/bin/python $PREFIX/training/latih_ulang.py >> /var/log/portal/retrain.log 2>&1"
            } | crontab -
        fi
        say "    supervisor + cron";;
      *) say "    tidak ada systemd/supervisor — jalankan sendiri dengan: $BIN live --bot $BOT --env $BOT/.env";;
    esac
    [ "${RHC_DARI_MULAI:-}" = 1 ] && return 0   # mulai.sh continues by itself
    say ""
    say "Berikutnya:  sudo ./rhc.sh atur"
}

# ================================================================= ATUR
cmd_atur(){
    need_root atur
    install -d -m 755 "$BOT"; install -d -m 700 "$SECRETS"
    hr; say " 1/5  RPC"; hr
    say "RPC publik gratis tapi kena batas laju dan TIDAK punya debug_traceCall,"
    say "jadi mode shadow tidak bisa jalan di sana."
    say "Berbayar yang mendukung chain 4663: QuickNode, Alchemy, Dwellir, ArrowRPC, Goldsky."
    say "Kosongkan untuk memakai RPC publik saja."
    say ""
    cek_dns
    local paid rpc_urls=$PUBLIC_RPC
    paid=$(ask "URL RPC berbayar (lengkap dengan api key)")
    if [ -n "$paid" ]; then
        say "    menguji ..."
        case "$(periksa_rpc "$paid")" in
          ok)
            say "    chainId 4663 OK"
            case "$(trace_support "$paid")" in
              ya)    say "    debug_traceCall ADA  -> mode shadow bisa dipakai";;
              tidak) say "    debug_traceCall TIDAK ada -> mode shadow tidak akan jalan";;
              *)     say "    debug_traceCall tidak bisa dipastikan (RPC sibuk)";;
            esac
            rpc_urls="$paid,$PUBLIC_RPC";;
          salah-chain:*)
            die "RPC itu melaporkan chainId yang salah, bukan 4663. Periksa URL-nya.";;
          bukan-evm)
            die "RPC itu menjawab tapi tidak mengenal eth_chainId — itu RPC Solana (Helius dsb), bukan EVM.";;
          mati)
            yes_no "    tidak ada jawaban. Tetap simpan?" && rpc_urls="$paid,$PUBLIC_RPC" || say "    dilewati";;
        esac
    else
        say "    memakai RPC publik saja"
    fi

    hr; say " 2/5  Wallet (MetaMask)"; hr
    if [ -s "$KEYFILE" ]; then
        say "Kunci sudah ada di $KEYFILE — dibiarkan."
    else
        say "Buat akun BARU di MetaMask khusus bot ini. Jangan akun utama Anda."
        say "  Add account -> Add a new account -> beri nama"
        say "  lalu titik tiga -> Account details -> Show private key -> salin"
        say ""
        say "Tempel di bawah. Ketikan TIDAK terlihat."
        local pk
        while :; do
            read -r -s -p "Private key: " pk; echo
            pk=${pk#0x}; pk=${pk//[[:space:]]/}
            [[ $pk =~ ^[0-9a-fA-F]{64}$ ]] && break
            say "    harus 64 karakter heksadesimal (yang ini ${#pk}). Coba lagi."
        done
        ( umask 077; printf '%s\n' "$pk" > "$KEYFILE" ); unset pk
        chmod 600 "$KEYFILE"; chown rhc:rhc "$KEYFILE" 2>/dev/null || true
        say "    tersimpan (mode 600)"
    fi

    hr; say " 3/5  Modal"; hr
    say "Isi sama dengan USD yang benar-benar Anda kirim ke wallet bot."
    local setoran
    while :; do
        setoran=$(ask "Setoran USD (angka saja, mis. 100 atau 1000)" 100)
        setoran=${setoran//[[:space:]]/}; setoran=${setoran#\$}
        # 1.000 / 1,000 is "one thousand" here: read as 1.0 the strategy could never size a position
        if [[ $setoran =~ ^[0-9]{1,3}([.,][0-9]{3})+$ ]]; then setoran=${setoran//[.,]/}; fi
        if [[ $setoran =~ ^[0-9]+([.][0-9]{1,2})?$ ]] && [ "${setoran%%.*}" -ge 20 ]; then
            yes_no "    Setoran = \$$setoran. Benar?" && break
        else
            say "    angka dolar saja, minimal 20"
        fi
    done

    hr; say " 4/5  Batas pengaman"; hr
    local maks stop_rugi maks_pos
    maks=$(ask "Maksimal USD per pembelian (0 = tanpa batas)" 5)
    stop_rugi=$(ask "Stop beli kalau rugi sehari tembus USD (0 = mati)" 25)
    maks_pos=$(ask "Maksimal posisi terbuka" 4)

    hr; say " 5/5  Mode"; hr
    say "  off     paper, tanpa transaksi. Mulai dari sini."
    say "  shadow  dibangun + ditandatangani + disimulasikan. Tidak dibroadcast."
    say "  live    TRANSAKSI SUNGGUHAN DENGAN UANG ANDA."
    say ""
    local mode confirm=""; mode=$(ask "Mode (off/shadow/live)" off); mode=${mode,,}
    case "$mode" in off|shadow) ;; live) ;; *) say "    tidak dikenal, dipakai off"; mode=off;; esac
    if [ "$mode" = live ]; then
        [ -s "$KEYFILE" ] || die "mode live butuh kunci wallet, tapi $KEYFILE kosong."
        say ""
        say "    Bot akan membelanjakan ETH sungguhan sendiri, tanpa bertanya tiap transaksi."
        if yes_no "    Lanjut dengan uang sungguhan?"; then confirm=I_ACCEPT_REAL_MONEY_RISK
        else say "    dibatalkan -> mode off"; mode=off; fi
    fi

    local tmp; tmp=$(mktemp); trap 'rm -f "$tmp"' RETURN
    sed -e "s#^RPC_URLS=.*#RPC_URLS=$(sedrepl "$rpc_urls")#" \
        -e "s#^DOH_URL=.*#DOH_URL=$(sedrepl "$DOH_ARG")#" \
        -e "s#^START_USD=.*#START_USD=$setoran#" \
        -e "s#^EXEC_MODE=.*#EXEC_MODE=$mode#" \
        -e "s#^EXEC_KEY_FILE=.*#EXEC_KEY_FILE=$KEYFILE#" \
        -e "s#^EXEC_MAX_ORDER_USD=.*#EXEC_MAX_ORDER_USD=$maks#" \
        -e "s#^EXEC_DAILY_LOSS_LIMIT_USD=.*#EXEC_DAILY_LOSS_LIMIT_USD=$stop_rugi#" \
        -e "s#^EXEC_MAX_OPEN_POSITIONS=.*#EXEC_MAX_OPEN_POSITIONS=$maks_pos#" \
        -e "s#^KELLY_SEED=.*#KELLY_SEED=$BOT/kelly_seed_backtest_last100.json#" \
        "$ROOT/config/trader.env.example" > "$tmp"
    [ -n "$confirm" ] && sed -i "s@^#*EXEC_LIVE_CONFIRM=.*@EXEC_LIVE_CONFIRM=$confirm@" "$tmp"
    [ -x "$PREFIX/venv/bin/python" ] && sed -i "s#^PYTHON=.*#PYTHON=$PREFIX/venv/bin/python#" "$tmp"
    install -m 600 "$tmp" "$BOT/.env"; chown rhc:rhc "$BOT/.env" 2>/dev/null || true
    [ -e "$BOT/kelly_seed_backtest_last100.json" ] || \
        install -m 644 "$ROOT/models/kelly_seed_backtest_last100.json" "$BOT/" 2>/dev/null || true

    say ""; hr; say " Tersimpan: $BOT/.env"; hr
    grep -E '^(RPC_URLS|START_USD|EXEC_MODE|EXEC_MAX_ORDER_USD|EXEC_DAILY_LOSS_LIMIT_USD|EXEC_MAX_OPEN_POSITIONS)=' "$BOT/.env" \
      | sed 's#\(RPC_URLS=https://[^/]*/\)[^,]*#\1***#' | sed 's/^/  /'
    say ""
    cmd_dompet || true
    say "Berikutnya:  sudo ./rhc.sh cek"
}

# ================================================================= ISI (formulir)
cmd_isi(){
    need_root isi
    local form=$ROOT/config/isi-disini.env ed
    [ -f "$form" ] || install -m 600 "$ROOT/config/isi-disini.env.example" "$form"
    ed=${EDITOR:-$(command -v nano || command -v vi)}
    [ -n "$ed" ] || die "tidak ada editor teks. Pasang nano: apt-get install -y nano"
    say "Membuka formulir. Tempel URL RPC dan private key, lalu simpan:"
    say "  nano: Ctrl+O, Enter (simpan) lalu Ctrl+X (keluar)"
    read -r -p "Tekan Enter untuk membuka ..." _
    "$ed" "$form"
    FORM=$form BOT=$BOT SECRETS=$SECRETS KEYFILE=$KEYFILE BIN=$BIN "$ROOT/deploy/pasang.sh"
    if svc_running; then
        say "Bot sedang jalan -> restart supaya pengaturan baru dipakai"
        case "$(svc_kind)" in
            systemd)    systemctl restart rhc-trader;;
            supervisor) supervisorctl restart rhc-trader;;
        esac
    fi
}

# ================================================================= LAIN-LAIN
cmd_dompet(){
    have_bin || die "binary belum ada. Jalankan: sudo ./rhc.sh pasang"
    [ -s "$KEYFILE" ] || { say "Belum ada wallet. Jalankan: sudo ./rhc.sh atur"; return 0; }
    say "Wallet bot:"
    "$BIN" wallet address --env "$BOT/.env" 2>&1 | sed 's/^/  /'
    say ""
    say "  Kirim ETH ke alamat itu di jaringan Robinhood Chain (chainId 4663)."
    say "  Sisakan minimal 0,003 ETH untuk gas."
    say ""
}

cmd_cek(){
    have_bin || die "binary belum ada. Jalankan: sudo ./rhc.sh pasang"
    [ -f "$BOT/.env" ] || die "belum diatur. Jalankan: sudo ./rhc.sh atur"
    "$BIN" exec selftest --env "$BOT/.env"
}

cmd_mulai(){
    need_root mulai
    case "$(svc_kind)" in
        # status exits 3 while the unit is (re)starting; under pipefail that must not abort the caller
        systemd)    systemctl enable --now rhc-trader rhc-retrain.timer; systemctl status --no-pager rhc-trader 2>/dev/null | head -5 || true;;
        supervisor) supervisorctl start rhc-trader || true; supervisorctl status rhc-trader;;
        *) die "tidak ada systemd/supervisor.";;
    esac
}
cmd_stop(){
    need_root stop
    case "$(svc_kind)" in
        systemd)    systemctl stop rhc-trader; say "berhenti";;
        supervisor) supervisorctl stop rhc-trader; say "berhenti";;
        *) die "tidak ada systemd/supervisor.";;
    esac
}
cmd_log(){
    # live.log holds the strategy lines AND the executor's real buys/sells ("EXEC: ..."), which only go there;
    # the journal has the strategy lines and crash messages only
    if [ -f "$BOT/rust_live/live.log" ]; then
        say "(Ctrl+C untuk keluar)"
        tail -n 40 -F "$BOT/rust_live/live.log"; return
    fi
    case "$(svc_kind)" in
        systemd)    journalctl -fu rhc-trader;;
        supervisor) tail -f /var/log/portal/rhc-trader.log;;
        *) tail -f "$BOT/rust_live/live.log";;
    esac
}
cmd_status(){
    hr; say " Status"; hr
    printf '  service   : %s\n' "$(svc_running && echo JALAN || echo berhenti)"
    printf '  binary    : %s\n' "$(have_bin && echo "$BIN" || echo 'belum dibuild')"
    printf '  konfig    : %s\n' "$([ -f "$BOT/.env" ] && echo "$BOT/.env" || echo 'belum ada')"
    printf '  wallet    : %s\n' "$([ -s "$KEYFILE" ] && echo 'terisi' || echo 'BELUM ADA')"
    [ -f "$BOT/.env" ] && grep -E '^(EXEC_MODE|START_USD|EXEC_MAX_ORDER_USD)=' "$BOT/.env" | sed 's/^/  /'
    local st=$BOT/rust_live/status.json
    if [ -f "$st" ] && command -v python3 >/dev/null; then
        python3 - "$st" <<'PY' 2>/dev/null || true
import json,sys
d=json.load(open(sys.argv[1])); a=d['akun']
print("  mode      : %s"%d['mode'])
print("  blok      : %d (lag %s)"%(d['last_block'],d['lag_blocks']))
print("  equity    : $%.2f | beli %d | tutup %d | menang %d"%(
      a['equity_harga_pasar'],a['trade_dibeli'],a['trade_selesai'],a['menang']))
print("  saringan  : %s"%d['saringan']['tolak'])
if d.get('last_error'): print("  error     : %s"%d['last_error'][:70])
PY
    fi
    say ""
}

# Is any real position still held? positions.json stores statuses in snake_case ("buying", "open", "stuck");
# the file is parsed, not grepped, and an unreadable file counts as "yes" (wiping it would strand the tokens).
ada_posisi_nyata(){
    [ -s "$1" ] || return 1
    command -v python3 >/dev/null || return 0
    python3 -c 'import json,sys
try: d = json.load(open(sys.argv[1]))
except Exception: sys.exit(0)
sys.exit(0 if any(p.get("status") in ("buying", "open", "stuck") for p in d.get("positions", {}).values()) else 1)' "$1"
}

cmd_bersih(){
    need_root bersih
    local LIVE=$BOT/rust_live
    [ -d "$LIVE" ] || { say "belum ada data — sudah bersih."; return 0; }
    svc_running && die "bot masih jalan. Hentikan dulu: sudo ./rhc.sh stop"
    local POS=$LIVE/exec/positions.json
    if ada_posisi_nyata "$POS"; then
        die "masih ada posisi terbuka di $POS (atau berkasnya tidak terbaca). Jual dulu (touch $LIVE/exec/FLATTEN_ALL, jalankan bot sampai kosong), baru bersihkan."
    fi
    local bak=$BOT/rust_live_backup_$(date -u +%Y%m%d_%H%M%S).tar.gz
    say "1. cadangan -> $bak"; tar -czf "$bak" -C "$BOT" rust_live; say "   $(du -h "$bak" | cut -f1)"
    say "2. mengosongkan catatan eksekusi"; rm -rf "$LIVE/exec" "$LIVE/exec_shadow"; install -d -m 755 "$LIVE/exec"
    if yes_no "3. setel ulang akun strategi ke START_USD juga?"; then
        rm -f "$LIVE/account_rust.json" "$LIVE/checkpoint.json" "$LIVE/state.json"; rm -rf "$LIVE/store"
        say "   akun disetel ulang — bot mulai dari blok terkini (pemanasan radar diulang)"
    else
        say "   akun dibiarkan"
    fi
    chown -R rhc:rhc "$LIVE" 2>/dev/null || true
    say ""
    say "DIPERTAHANKAN: model, labels.jsonl, riwayat retrain"
    say "Mulai sekarang setiap baris di $LIVE/exec/ adalah transaksi nyata."
}

# ================================================================= MENU
menu(){
    while :; do
        say ""; hr; say " rhc-trader — Robinhood Chain 4663 · Uniswap v4"; hr
        cmd_status
        say "  1) pasang   — paket, Rust, build, service"
        say "  2) atur     — RPC + wallet MetaMask + batas + mode (tanya-jawab)"
        say "  9) isi      — sama, lewat formulir (tinggal tempel)"
        say "  3) cek      — self-test on-chain (tidak mengirim apa pun)"
        say "  4) dompet   — alamat wallet dan saldo"
        say "  5) mulai    6) stop    7) log"
        say "  8) bersih   — kosongkan catatan sebelum live"
        say "  0) keluar"
        say ""
        case "$(ask 'Pilih')" in
            1) cmd_pasang;; 2) cmd_atur;; 3) cmd_cek || true;; 4) cmd_dompet || true;;
            5) cmd_mulai;;  6) cmd_stop;; 7) cmd_log;; 8) cmd_bersih;; 9) cmd_isi;;
            0|q|keluar) exit 0;;
            *) say "pilihan tidak dikenal";;
        esac
    done
}

case "${1:-menu}" in
    pasang)  cmd_pasang;;
    atur)    cmd_atur;;
    isi)     cmd_isi;;
    cek)     cmd_cek;;
    dompet)  cmd_dompet;;
    mulai)   cmd_mulai;;
    stop)    cmd_stop;;
    log)     cmd_log;;
    status)  cmd_status;;
    bersih)  cmd_bersih;;
    menu)    menu;;
    *) say "pakai: sudo ./rhc.sh [pasang|atur|isi|cek|dompet|mulai|stop|log|status|bersih]"; exit 1;;
esac
