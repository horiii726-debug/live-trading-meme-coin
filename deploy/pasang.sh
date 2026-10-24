#!/usr/bin/env bash
# Apply config/isi-disini.env: validate it, write <bot>/.env and the key file, then blank the two
# secret fields in the form so the plaintext key does not stay on disk twice.
#
#   sudo deploy/pasang.sh                    # reads config/isi-disini.env
#   FORM=/path/to/form.env deploy/pasang.sh  # another form
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
FORM=${FORM:-$ROOT/config/isi-disini.env}
BOT=${BOT:-/opt/rhc/bot}
SECRETS=${SECRETS:-$(dirname "$BOT")/secrets}
KEYFILE=${KEYFILE:-$SECRETS/wallet.key}
BIN=${BIN:-/opt/rhc/bin/rhc-trader}
[ -x "$BIN" ] || BIN="$ROOT/target/release/rhc-trader"
PUBLIC_RPC=https://rpc.mainnet.chain.robinhood.com
CHAIN_ID_HEX=0x1237

say(){ printf '%s\n' "$*"; }
die(){ printf 'GAGAL: %s\n' "$*" >&2; exit 1; }

if [ ! -f "$FORM" ]; then
    if [ -f "$FORM.example" ]; then
        install -m 600 "$FORM.example" "$FORM"
        die "formulir dibuat dari contoh: $FORM
       Isi KOLOM 1 (URL RPC) dan KOLOM 2 (private key MetaMask), lalu jalankan lagi."
    fi
    die "formulir tidak ada: $FORM"
fi

# read only the keys we expect; never source the file (it must not be able to run commands).
# strips an inline "  # komentar" and surrounding whitespace, so "5   # 0 = mati" reads as "5".
field(){ sed -n "s/^[[:space:]]*$1=//p" "$FORM" | head -1 | sed -e 's/[[:space:]]\+#.*$//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//'; }
# escape a value for the right-hand side of a sed s### command ("&" means "the match" there, "#" ends it)
sedrepl(){ printf '%s' "$1" | sed -e 's/[\\&#]/\\&/g'; }

# Does this endpoint support debug_traceCall? Decided by JSON-RPC error code, not by message text:
#   -32601 method not found  -> no
#   any other error / none   -> yes (the method exists, it just dislikes our empty params)
#   no answer, 429, html     -> unknown (a rate-limited probe must not be read as "yes")
trace_support(){
    local url=$1 t
    t=$(curl -sS -m 20 -X POST "$url" -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","id":1,"method":"debug_traceCall","params":[]}' 2>/dev/null || true)
    case "$t" in
        *'-32601'*)        printf 'tidak';;
        *'"error"'*'429'*) printf 'tidak-diketahui';;
        *'"jsonrpc"'*)     printf 'ya';;
        *)                 printf 'tidak-diketahui';;
    esac
}
RPC_API_KEY=$(field RPC_API_KEY)
WALLET_PRIVATE_KEY=$(field WALLET_PRIVATE_KEY)
SETORAN_USD=$(field SETORAN_USD); SETORAN_USD=${SETORAN_USD:-100}
MODE=$(field MODE); MODE=$(printf '%s' "${MODE:-off}" | tr 'A-Z' 'a-z')
MAKS_BELI=$(field MAKS_USD_PER_BELI); MAKS_BELI=${MAKS_BELI:-5}
STOP_RUGI=$(field STOP_RUGI_HARIAN_USD); STOP_RUGI=${STOP_RUGI:-25}
MAKS_POS=$(field MAKS_POSISI_TERBUKA); MAKS_POS=${MAKS_POS:-4}

say "=============================================================="
say " Memasang konfigurasi dari $FORM"
say "=============================================================="
say ""

# ---------------------------------------------------------------- kolom 1: RPC
RPC_URLS=$PUBLIC_RPC
if [ -n "$RPC_API_KEY" ]; then
    say "KOLOM 1  menguji RPC ..."
    r=$(curl -sS -m 20 -X POST "$RPC_API_KEY" -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' 2>/dev/null || true)
    case "$r" in
        *"\"result\":\"$CHAIN_ID_HEX\""*)
            say "         chainId 4663 OK"
            case "$(trace_support "$RPC_API_KEY")" in
                ya)    say "         debug_traceCall ada -> EXEC_MODE=shadow bisa dipakai";;
                tidak) say "         debug_traceCall TIDAK ada -> EXEC_MODE=shadow tidak akan jalan";;
                *)     say "         debug_traceCall tidak bisa dipastikan (RPC sibuk atau kena batas laju)";;
            esac
            RPC_URLS="$RPC_API_KEY,$PUBLIC_RPC"
            ;;
        *'"result":"0x'*)
            die "RPC itu melaporkan chainId $(printf '%s' "$r" | sed -n 's/.*"result":"\([^"]*\)".*/\1/p'), bukan 4663. Perbaiki KOLOM 1."
            ;;
        "") die "RPC tidak menjawab. Periksa URL di KOLOM 1.";;
        *)  die "RPC itu menjawab tapi tidak mengenal eth_chainId — itu RPC Solana (Helius dsb), bukan EVM. Perbaiki KOLOM 1.";;
    esac
else
    say "KOLOM 1  kosong -> memakai RPC publik (ada batas laju; shadow tidak tersedia)"
fi

# ---------------------------------------------------------------- kolom 2: kunci
install -d -m 700 "$SECRETS"
install -d -m 755 "$BOT"
# the service runs as user rhc: a root-owned mode-600 key or .env would be unreadable to it
chown rhc:rhc "$SECRETS" 2>/dev/null || true
if [ -n "$WALLET_PRIVATE_KEY" ]; then
    PK=${WALLET_PRIVATE_KEY#0x}
    [[ $PK =~ ^[0-9a-fA-F]{64}$ ]] || die "KOLOM 2 bukan 64 karakter heksadesimal (panjangnya ${#PK})."
    ( umask 077; printf '%s\n' "$PK" > "$KEYFILE" )
    chmod 600 "$KEYFILE"; chown rhc:rhc "$KEYFILE" 2>/dev/null || true
    unset PK
    say "KOLOM 2  kunci disimpan di $KEYFILE (mode 600)"
elif [ -s "$KEYFILE" ]; then
    say "KOLOM 2  kosong, tapi kunci lama sudah ada di $KEYFILE — dipakai"
else
    say "KOLOM 2  kosong dan belum ada kunci -> mode live/shadow tidak akan bisa jalan"
fi

# ---------------------------------------------------------------- kolom 4: mode
case "$MODE" in
    off|shadow) ;;
    live)
        [ -s "$KEYFILE" ] || die "MODE=live tapi KOLOM 2 kosong dan tidak ada kunci di $KEYFILE."
        say ""
        say "         MODE=live: bot akan membelanjakan ETH sungguhan dari wallet itu,"
        say "         sendiri, tanpa bertanya tiap transaksi."
        say ""
        ;;
    *) die "MODE tidak dikenal: $MODE (pakai off, shadow, atau live)";;
esac

# ---------------------------------------------------------------- tulis .env
TMP=$(mktemp); trap 'rm -f "$TMP"' EXIT
sed -e "s#^RPC_URLS=.*#RPC_URLS=$(sedrepl "$RPC_URLS")#" \
    -e "s#^START_USD=.*#START_USD=$SETORAN_USD#" \
    -e "s#^EXEC_MODE=.*#EXEC_MODE=$MODE#" \
    -e "s#^EXEC_KEY_FILE=.*#EXEC_KEY_FILE=$KEYFILE#" \
    -e "s#^EXEC_MAX_ORDER_USD=.*#EXEC_MAX_ORDER_USD=$MAKS_BELI#" \
    -e "s#^EXEC_DAILY_LOSS_LIMIT_USD=.*#EXEC_DAILY_LOSS_LIMIT_USD=$STOP_RUGI#" \
    -e "s#^EXEC_MAX_OPEN_POSITIONS=.*#EXEC_MAX_OPEN_POSITIONS=$MAKS_POS#" \
    -e "s#^KELLY_SEED=.*#KELLY_SEED=$BOT/kelly_seed_backtest_last100.json#" \
    "$ROOT/config/trader.env.example" > "$TMP"
[ "$MODE" = live ] && sed -i "s@^#*EXEC_LIVE_CONFIRM=.*@EXEC_LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK@" "$TMP"
[ -x /opt/rhc/venv/bin/python ] && sed -i "s#^PYTHON=.*#PYTHON=/opt/rhc/venv/bin/python#" "$TMP"
install -m 600 "$TMP" "$BOT/.env"; chown rhc:rhc "$BOT/.env" 2>/dev/null || true
[ -e "$BOT/kelly_seed_backtest_last100.json" ] || \
    install -m 644 "$ROOT/models/kelly_seed_backtest_last100.json" "$BOT/" 2>/dev/null || true

# ---------------------------------------------------------------- kosongkan rahasia di formulir
if [ -n "$RPC_API_KEY" ] || [ -n "$WALLET_PRIVATE_KEY" ]; then
    sed -i -e 's/^RPC_API_KEY=.*/RPC_API_KEY=/' -e 's/^WALLET_PRIVATE_KEY=.*/WALLET_PRIVATE_KEY=/' "$FORM"
    chmod 600 "$FORM" 2>/dev/null || true
    say ""
    say "         KOLOM 1 dan 2 di formulir sudah dikosongkan (rahasia tidak disimpan dua kali)"
fi

say ""
say "=============================================================="
say " Tersimpan: $BOT/.env"
say "=============================================================="
grep -E '^(RPC_URLS|START_USD|EXEC_MODE|EXEC_KEY_FILE|EXEC_MAX_ORDER_USD|EXEC_DAILY_LOSS_LIMIT_USD|EXEC_MAX_OPEN_POSITIONS)=' "$BOT/.env" \
  | sed 's#\(RPC_URLS=https://[^/]*/\)[^,]*#\1***#'
say ""

if [ -x "$BIN" ] && [ -s "$KEYFILE" ]; then
    say "Wallet bot:"
    "$BIN" wallet address --env "$BOT/.env" 2>&1 | sed 's/^/  /' || say "  (saldo gagal dibaca — periksa RPC)"
    say ""
    say "  Kirim ETH ke alamat itu di Robinhood Chain (chainId 4663)."
    say "  Sisakan minimal 0,003 ETH untuk gas."
    say ""
fi

say "Berikutnya:"
say "  $BIN exec selftest --env $BOT/.env     # wajib berakhir SELFTEST OK"
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null; then
    say "  systemctl restart rhc-trader && journalctl -fu rhc-trader"
else
    say "  supervisorctl restart rhc-trader && tail -f /var/log/portal/rhc-trader.log"
fi
say ""
say "Rem darurat (tanpa restart):"
say "  touch $BOT/rust_live/exec/STOP_BUYING    # stop beli"
say "  touch $BOT/rust_live/exec/FLATTEN_ALL    # jual semua sekarang"
