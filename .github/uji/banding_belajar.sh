#!/usr/bin/env bash
# Adaptive filter (BELAJAR=1: ML CatBoost retrained by /opt/rhc/bin/catboost + DL in Rust) vs frozen (BELAJAR=0),
# same simulator, same data, settings A (backtest) and B (the installed bot), periods p1 / p2 / full.
set -euo pipefail
ROOT=$PWD
BIN=$ROOT/target/release
RISET=$ROOT/riset
W=$ROOT/kerja
O=$ROOT/hasil
mkdir -p "$W" "$O"
say(){ printf '\n=== %s\n' "$*"; }

say "1. research windows -> one dataset"
for n in $(seq -w 0 14); do
    src=$RISET/data/w$n; dst=$W/w$n; mkdir -p "$dst"
    for f in pools.csv swaps_sorted.csv liq.csv trades.jsonl swaps_m1.json transfers.json dev_creator.json; do
        if ls "$src/$f.zst.part"* >/dev/null 2>&1; then cat "$src/$f.zst.part"* > "$src/$f.zst"; fi
        zstd -dqf --rm "$src/$f.zst" -o "$dst/$f"; rm -f "$src/$f.zst.part"*
    done
done
python3 .github/uji/gabung_jendela.py "$W" all $(for n in $(seq -w 0 14); do printf 'w%s ' "$n"; done)
rm -f "$W"/w*/swaps_sorted.csv "$W"/w*/liq.csv; df -h . | tail -1

SYS_A="QTHR=0.80 MAX_POS=10 DANGER_TH=0.5 TRAIL_PUNCAK=0.4 TUTUP_SAAT_RUG=1 DUMP_FRAC=0.05 KELLY_MAKS=0.15 MAX_FEE=5000"
sed -e "s#^START_USD=.*#START_USD=100#" -e "s#^KELLY_SEED=.*#KELLY_SEED=$ROOT/models/kelly_seed_backtest_last100.json#" config/trader.env.example > sim.env
jalan(){ mode=$1; sys=$2; per=$3
    case $per in p1) a=65779486; b=79411485;; p2) a=79411486; b=90197095;; full) a=65779486; b=90197095;; esac
    case $mode in baru) bl=1;; lama) bl=0;; esac
    k=${mode}_${sys}_$per
    if [ "$sys" = a ]; then
        env -i PATH="$PATH" HOME="$HOME" START_USD=100 BELAJAR=$bl CATBOOST_BIN=/opt/rhc/bin/catboost $SYS_A \
            "$BIN/rhc-trader" sim "$W/all" "$a" "$b" "$O/$k.jsonl" --bot models > "$O/$k.txt" 2>&1 || true
    else
        BELAJAR=$bl CATBOOST_BIN=/opt/rhc/bin/catboost "$BIN/rhc-trader" sim "$W/all" "$a" "$b" "$O/$k.jsonl" --bot models --env sim.env > "$O/$k.txt" 2>&1 || true
    fi
    printf '%-5s %s %-4s %s | %s\n' "$mode" "$sys" "$per" "$(grep '^SIM' "$O/$k.txt" | sed 's/^SIM [^|]*| //' | cut -c1-125)" "$(grep 'filter adaptif:.*dipelajari' "$O/$k.txt" | sed 's/.*filter adaptif: //')"
}
export -f jalan; export BIN W O SYS_A

say "2. adaptive (baru) vs frozen (lama)"
for mode in baru lama; do for sys in a b; do for per in full p1 p2; do echo "$mode $sys $per"; done; done; done \
    | xargs -P 2 -L 1 bash -c 'jalan "$0" "$1" "$2"' | sort | tee "$O/tabel.txt"
say "retrains in the full B run"
grep -E "latih ulang ML" "$O/baru_b_full.txt" | head -80 || true
