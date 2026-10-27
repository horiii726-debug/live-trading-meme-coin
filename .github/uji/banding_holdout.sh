#!/usr/bin/env bash
# TRUE HOLD-OUT: data after 16 Oct 2026 (block 90197096 -> 9.5 h before the tip), never used to choose any setting.
# Downloaded exactly as the research did it, then the bot's own simulator: adaptive (baru) vs frozen (lama), settings A
# (backtest, no Kelly seed) and B (the installed bot: its Kelly seed is the backtest's last 100 trades up to 16 Oct, past
# data for this window). As if the bot had been installed on 16 Oct: fresh account, fresh learner.
set -euo pipefail
ROOT=$PWD
BIN=$ROOT/target/release
D=$ROOT/kerja/n00
O=$ROOT/hasil
mkdir -p "$D" "$O"
export RPC_URLS=https://rpc.mainnet.chain.robinhood.com JALUR=2
AUDIT='{"latb":30,"entryLat":30,"keepNoAfter":true,"strictSnap":true,"lpAdjust":true,"fillOnTime":true,"gasUsd":0.06,"approveUsd":0.03,"worstOrder":true,"hz":324000,"failBuy":0.0185,"failSell":0.0054}'
say(){ printf '\n=== %s\n' "$*"; }

say "1. range"
read -r FROM TO END TIP < <(python3 .github/uji/rentang_baru.py 90197096)
MID=$(( (FROM + TO) / 2 ))
echo "$FROM $MID $TO $END $TIP" | tee "$O/periode.txt"

say "2. download, replay, minute-1 data"
for i in 1 2 3; do "$BIN/rhc-trader" fast "$FROM" "$TO" "$D" && break; echo "retry $i"; sleep 60; done
echo '{"ethUsd": 2300}' > models/eth_usd.json
(cd models && env AUDIT="$AUDIT" DANGER_TH=0.4 UP_TH=0.5 TP_UP=0.5 DANGER_FILE="$PWD/danger_model.json" UP_FILE="$PWD/pnaik_model.json" \
    REPLAY_THREADS=4 "$BIN/replay" "$D" "$FROM" "$END" "$D/trades.jsonl" auto | tail -2)
for i in 1 2 3; do "$BIN/rhc-trader" fast1 "$D" && break; echo "retry $i"; sleep 60; done
df -h . | tail -1

say "3. adaptive (baru) vs frozen (lama), A and B, first half / second half / full"
SYS_A="QTHR=0.80 MAX_POS=10 DANGER_TH=0.5 TRAIL_PUNCAK=0.4 TUTUP_SAAT_RUG=1 DUMP_FRAC=0.05 KELLY_MAKS=0.15 MAX_FEE=5000"
sed -e "s#^START_USD=.*#START_USD=100#" -e "s#^KELLY_SEED=.*#KELLY_SEED=$ROOT/models/kelly_seed_backtest_last100.json#" config/trader.env.example > sim.env
jalan(){ mode=$1; sys=$2; per=$3
    case $per in p1) a=$FROM; b=$MID;; p2) a=$((MID + 1)); b=$TO;; full) a=$FROM; b=$TO;; esac
    case $mode in baru) bl=1;; lama) bl=0;; esac
    k=${mode}_${sys}_$per
    if [ "$sys" = a ]; then
        env -i PATH="$PATH" HOME="$HOME" START_USD=100 BELAJAR=$bl CATBOOST_BIN=/opt/rhc/bin/catboost $SYS_A \
            "$BIN/rhc-trader" sim "$D" "$a" "$b" "$O/$k.jsonl" --bot models > "$O/$k.txt" 2>&1 || true
    else
        BELAJAR=$bl CATBOOST_BIN=/opt/rhc/bin/catboost "$BIN/rhc-trader" sim "$D" "$a" "$b" "$O/$k.jsonl" --bot models --env sim.env > "$O/$k.txt" 2>&1 || true
    fi
    printf '%-5s %s %-4s %s | %s\n' "$mode" "$sys" "$per" "$(grep '^SIM' "$O/$k.txt" | sed 's/^SIM [^|]*| //' | cut -c1-125)" "$(grep -E 'filter adaptif:.*dipelajari|cek kausal' "$O/$k.txt" | sed 's/.*filter adaptif: //' | tr '\n' ' ')"
}
export -f jalan; export BIN D O SYS_A FROM MID TO
for mode in baru lama; do for sys in a b; do for per in full p1 p2; do echo "$mode $sys $per"; done; done; done \
    | xargs -P 2 -L 1 bash -c 'jalan "$0" "$1" "$2"' | sort | tee "$O/tabel.txt"
say "out-of-sample accuracy over the hold-out (full B)"
grep -E "akurasi data belum dilihat|DL dimatikan|latih ulang ML #" "$O/baru_b_full.txt" | tail -25 || true
