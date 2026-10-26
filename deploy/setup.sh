#!/usr/bin/env bash
# Interactive configuration for rhc-trader: asks for the RPC endpoints, the bot wallet key and the
# safety limits, then writes <bot>/.env and the key file with the right permissions.
#
#   sudo deploy/setup.sh                 # after deploy/install.sh, configures /opt/rhc
#   BOT=/path/to/bot deploy/setup.sh     # any other location
#
# The private key is read with echo off, never printed, never written to the shell history and never
# sent anywhere. It only ever lands in the key file (mode 600).
set -euo pipefail

BOT=${BOT:-/opt/rhc/bot}
SECRETS=${SECRETS:-$(dirname "$BOT")/secrets}
KEYFILE=${KEYFILE:-$SECRETS/wallet.key}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${BIN:-$(command -v rhc-trader || echo /opt/rhc/bin/rhc-trader)}
[ -x "$BIN" ] || BIN="$ROOT/target/release/rhc-trader"

CHAIN_ID_HEX=0x1237   # 4663
say()  { printf '%s\n' "$*"; }
# escape a value for the right-hand side of a sed s### command ("&" means "the match" there, "#" ends it)
sedrepl(){ printf '%s' "$1" | sed -e 's/[\\&#]/\\&/g'; }
ask()  { local p=$1 d=${2:-} a; read -r -p "$p${d:+ [$d]}: " a; printf '%s' "${a:-$d}"; }
yes_no() { local a; read -r -p "$1 (y/n) [n]: " a; [ "${a,,}" = y ]; }

# Some networks answer DNS for the RPC host with a block page (Indonesia's Internet Positif does). The bot then needs
# DOH_URL set (it resolves through Cloudflare at 1.1.1.1 itself); DOH_ARG makes these curl probes do the same.
DOH=https://1.1.1.1/dns-query
DOH_ARG=""
rpc_call() { # rpc_call <url> <method> [params-json]
    curl -sS -m 20 ${DOH_ARG:+--doh-url "$DOH_ARG"} -X POST "$1" -H 'content-type: application/json' \
         -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":${3:-[]}}" 2>/dev/null || true
}

check_rpc() { # prints: ok:<yes|no|unknown debug_traceCall> | wrongchain:<id> | notevm | unreachable
    # (it runs in $(...), a subshell: the trace answer must travel in the output, a variable set here is lost)
    local url=$1 r
    r=$(rpc_call "$url" eth_chainId)
    case "$r" in
        *"\"result\":\"$CHAIN_ID_HEX\""*) ;;
        *'"result":"0x'*) printf 'wrongchain:%s' "$(printf '%s' "$r" | sed -n 's/.*"result":"\([^"]*\)".*/\1/p')"; return;;
        "") printf 'unreachable'; return;;
        # answered, but eth_chainId is unknown to it: a Solana endpoint (Helius, QuickNode Solana, ...)
        *) printf 'notevm'; return;;
    esac
    # decided by JSON-RPC error code, never by message text: a 429 probe must not read as "yes"
    r=$(rpc_call "$url" debug_traceCall)
    case "$r" in
        *'-32601'*)        printf 'ok:no';;
        *'"error"'*'429'*) printf 'ok:unknown';;
        *'"jsonrpc"'*)     printf 'ok:yes';;
        *)                 printf 'ok:unknown';;
    esac
}

say "=============================================================="
say " rhc-trader — konfigurasi"
say " Robinhood Chain (chainId 4663) · Uniswap v4"
say "=============================================================="
say ""
say "Direktori bot : $BOT"
say "File kunci    : $KEYFILE"
say ""

install -d -m 755 "$BOT"
install -d -m 700 "$SECRETS"

# ---------------------------------------------------------------- 1. RPC
say "--- 1/5  RPC (sumber data chain dan pengiriman transaksi) ---"
say ""
say "RPC publik Robinhood gratis tapi sering kena 429 Too Many Requests, dan TIDAK"
say "mendukung debug_traceCall, jadi mode shadow tidak bisa jalan di sana."
say "Penyedia berbayar yang mendukung chain 4663: QuickNode, Alchemy, Dwellir, ArrowRPC, Goldsky."
say "Kosongkan untuk memakai RPC publik saja."
say ""
PUBLIC_RPC=https://rpc.mainnet.chain.robinhood.com
case "$(check_rpc "$PUBLIC_RPC")" in
    ok:*) ;;
    *)  DOH_ARG=$DOH
        case "$(check_rpc "$PUBLIC_RPC")" in
            ok:*) say "DNS VPS ini membelokkan alamat RPC (diblokir DNS) -> bot memakai DNS-over-HTTPS (1.1.1.1)"; say "";;
            *)    DOH_ARG="";;
        esac;;
esac
PAID_RPC=$(ask "URL RPC berbayar Anda (boleh kosong)")

RPC_URLS=$PUBLIC_RPC
if [ -n "$PAID_RPC" ]; then
    say ""
    say "menguji RPC ..."
    res=$(check_rpc "$PAID_RPC")
    case "$res" in
        ok:*)
            say "  chainId 4663 OK"
            case "${res#ok:}" in
                yes) say "  debug_traceCall tersedia — mode shadow bisa dipakai";;
                no)  say "  debug_traceCall TIDAK tersedia — mode shadow tidak akan jalan di RPC ini";;
                *)   say "  debug_traceCall tidak bisa dipastikan (RPC sibuk atau kena batas laju)";;
            esac
            RPC_URLS="$PAID_RPC,$PUBLIC_RPC"
            ;;
        wrongchain:*)
            say "  GAGAL: RPC ini melaporkan chainId ${res#wrongchain:}, bukan $CHAIN_ID_HEX (4663)."
            say "  Endpoint Solana (mis. Helius) tidak bisa dipakai: bot ini butuh RPC EVM."
            yes_no "  Tetap pakai?" && RPC_URLS="$PAID_RPC,$PUBLIC_RPC" || say "  dilewati, memakai RPC publik saja"
            ;;
        notevm)
            say "  GAGAL: endpoint ini menjawab, tapi tidak mengenal eth_chainId."
            say "  Itu tanda RPC Solana (Helius, QuickNode Solana, dsb). Bot ini butuh RPC EVM"
            say "  untuk Robinhood Chain 4663 — RPC Solana tidak akan pernah bisa dipakai."
            say "  dilewati, memakai RPC publik saja"
            ;;
        unreachable)
            say "  GAGAL: tidak ada jawaban dari RPC itu (URL salah, atau jaringan diblokir)."
            yes_no "  Tetap simpan?" && RPC_URLS="$PAID_RPC,$PUBLIC_RPC" || say "  dilewati, memakai RPC publik saja"
            ;;
    esac
else
    say "  memakai RPC publik saja (ada batas laju; mode shadow tidak tersedia)"
fi
say ""

# ---------------------------------------------------------------- 2. wallet
say "--- 2/5  Wallet bot ---"
say ""
if [ -s "$KEYFILE" ]; then
    say "Kunci sudah ada di $KEYFILE — dibiarkan apa adanya."
else
    say "Buat akun BARU di MetaMask khusus untuk bot ini. Jangan pakai akun utama Anda."
    say "  MetaMask: Add account -> Add a new account -> beri nama (mis. \"Bot Trading\")"
    say "  lalu: titik tiga -> Account details -> Show private key -> salin"
    say ""
    say "Tempel private key di bawah. Ketikan TIDAK akan terlihat di layar."
    say "Seed phrase MetaMask tetap jadi cadangan Anda kalau server ini hilang."
    say ""
    while :; do
        read -r -s -p "Private key (64 hex, boleh diawali 0x): " PK; echo
        PK=${PK#0x}; PK=${PK//[[:space:]]/}
        if [ ${#PK} -ne 64 ]; then
            say "  panjangnya ${#PK} karakter, seharusnya 64. Coba lagi."
        elif ! [[ $PK =~ ^[0-9a-fA-F]{64}$ ]]; then
            say "  ada karakter yang bukan heksadesimal. Coba lagi."
        else
            break
        fi
    done
    ( umask 077; printf '%s\n' "$PK" > "$KEYFILE" )
    unset PK
    chmod 600 "$KEYFILE"; chown rhc:rhc "$SECRETS" "$KEYFILE" 2>/dev/null || true
    say "  tersimpan di $KEYFILE (mode 600)"
fi
say ""

# ---------------------------------------------------------------- 3. modal
say "--- 3/5  Modal ---"
say ""
say "START_USD harus sama dengan jumlah USD yang Anda setor ke wallet bot:"
say "akun strategi memakai angka ini sebagai acuan ekuitas untuk menghitung ukuran posisi."
START_USD=$(ask "Setoran Anda dalam USD" 100)
say ""

# ---------------------------------------------------------------- 4. rem
say "--- 4/5  Batas pengaman ---"
say ""
say "Batas ini hanya bisa MEMBATALKAN beli atau MEMAKSA jual. Tidak pernah mengubah keputusan strategi."
MAX_ORDER=$(ask "Maksimal USD per pembelian (0 = tanpa batas)" 0)
DAILY_LOSS=$(ask "Stop beli kalau rugi terealisasi sehari tembus USD (0 = mati)" 0)
MAX_OPEN=$(ask "Maksimal posisi terbuka" 6)
say ""

# ---------------------------------------------------------------- 5. mode
say "--- 5/5  Mode eksekusi ---"
say ""
say "  off     paper trading. Tidak ada transaksi. Mulai dari sini."
say "  shadow  order dibangun, ditandatangani, DISIMULASIKAN di state live. Tidak dibroadcast."
say "          Butuh RPC dengan debug_traceCall."
say "  live    TRANSAKSI SUNGGUHAN DENGAN UANG ANDA."
say ""
MODE=$(ask "Mode (off/shadow/live)" off)
MODE=${MODE,,}
case "$MODE" in off|shadow|live) ;; *) say "mode tidak dikenal, dipakai off"; MODE=off;; esac

LIVE_CONFIRM=""
if [ "$MODE" = live ]; then
    say ""
    say "  Anda memilih LIVE. Bot akan membelanjakan ETH sungguhan dari wallet itu,"
    say "  sendiri, tanpa menanyakan tiap transaksi. Memecoin bisa habis total."
    say ""
    if yes_no "  Lanjut dengan uang sungguhan?"; then
        LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK
    else
        say "  dibatalkan — dipakai mode off"
        MODE=off
    fi
fi
say ""

# ---------------------------------------------------------------- tulis .env
TMP=$(mktemp); trap 'rm -f "$TMP"' EXIT
sed -e "s#^RPC_URLS=.*#RPC_URLS=$(sedrepl "$RPC_URLS")#" \
    -e "s#^DOH_URL=.*#DOH_URL=$(sedrepl "$DOH_ARG")#" \
    -e "s#^START_USD=.*#START_USD=$START_USD#" \
    -e "s#^EXEC_MODE=.*#EXEC_MODE=$MODE#" \
    -e "s#^EXEC_KEY_FILE=.*#EXEC_KEY_FILE=$KEYFILE#" \
    -e "s#^EXEC_MAX_ORDER_USD=.*#EXEC_MAX_ORDER_USD=$MAX_ORDER#" \
    -e "s#^EXEC_DAILY_LOSS_LIMIT_USD=.*#EXEC_DAILY_LOSS_LIMIT_USD=$DAILY_LOSS#" \
    -e "s#^EXEC_MAX_OPEN_POSITIONS=.*#EXEC_MAX_OPEN_POSITIONS=$MAX_OPEN#" \
    -e "s#^KELLY_SEED=.*#KELLY_SEED=$BOT/kelly_seed_backtest_last100.json#" \
    "$ROOT/config/trader.env.example" > "$TMP"
if [ -n "$LIVE_CONFIRM" ]; then
    sed -i "s@^#*EXEC_LIVE_CONFIRM=.*@EXEC_LIVE_CONFIRM=$LIVE_CONFIRM@" "$TMP"
fi
install -m 600 "$TMP" "$BOT/.env"; chown rhc:rhc "$BOT/.env" 2>/dev/null || true
[ -e "$BOT/kelly_seed_backtest_last100.json" ] || \
    install -m 644 "$ROOT/models/kelly_seed_backtest_last100.json" "$BOT/" 2>/dev/null || true

say "=============================================================="
say " Tersimpan: $BOT/.env"
say "=============================================================="
grep -E '^(RPC_URLS|START_USD|EXEC_MODE|EXEC_MAX_ORDER_USD|EXEC_DAILY_LOSS_LIMIT_USD|EXEC_MAX_OPEN_POSITIONS)=' "$BOT/.env"
say ""

if [ -x "$BIN" ] && [ -s "$KEYFILE" ]; then
    say "Alamat wallet bot dan saldonya:"
    "$BIN" wallet address --env "$BOT/.env" || say "  (gagal membaca saldo — cek RPC)"
    say ""
    say "Kirim ETH ke alamat itu di Robinhood Chain (chainId 4663)."
    say "Sisakan minimal 0,003 ETH untuk gas."
    say ""
fi

say "Berikutnya:"
say "  1. $BIN exec selftest --env $BOT/.env      # wajib berakhir SELFTEST OK"
if command -v systemctl >/dev/null && [ -d /run/systemd/system ]; then
    say "  2. systemctl enable --now rhc-trader"
    say "  3. journalctl -fu rhc-trader"
else
    say "  2. supervisorctl start rhc-trader"
    say "  3. tail -f /var/log/portal/rhc-trader.log"
fi
say ""
say "Rem darurat (tanpa restart):"
say "  stop beli       : touch $BOT/rust_live/exec/STOP_BUYING"
say "  jual semua      : touch $BOT/rust_live/exec/FLATTEN_ALL"
