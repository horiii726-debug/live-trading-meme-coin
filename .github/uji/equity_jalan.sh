#!/usr/bin/env bash
# Test only: the same backtest on main and on the fix branch, trade by trade. Same data, same settings, one continuous
# account from $100 (18 Sep - 16 Oct). main runs twice, to show what the run itself repeats.
set -euo pipefail
ROOT=$PWD
O=$ROOT/hasil
W=$ROOT/kerja
RISET=$ROOT/riset
mkdir -p "$O"
say(){ printf '\n=== %s\n' "$*"; }
SYS_A="QTHR=0.80 MAX_POS=10 DANGER_TH=0.5 TRAIL_PUNCAK=0.4 TUTUP_SAAT_RUG=1 DUMP_FRAC=0.05 KELLY_MAKS=0.15 MAX_FEE=5000"

say "research windows -> one dataset"
for n in $(seq -w 0 14); do
    src=$RISET/data/w$n; dst=$W/w$n; mkdir -p "$dst"
    for f in pools.csv swaps_sorted.csv liq.csv trades.jsonl swaps_m1.json transfers.json dev_creator.json; do
        if ls "$src/$f.zst.part"* >/dev/null 2>&1; then cat "$src/$f.zst.part"* > "$src/$f.zst"; fi
        zstd -dqf --rm "$src/$f.zst" -o "$dst/$f"; rm -f "$src/$f.zst.part"*
    done
done
python3 .github/uji/gabung_jendela.py "$W" all $(for n in $(seq -w 0 14); do printf 'w%s ' "$n"; done)
rm -f "$W"/w*/swaps_sorted.csv "$W"/w*/liq.csv

bangun(){
    say "build $2"
    git worktree add -q "/tmp/b_$1" "$2"
    (cd "/tmp/b_$1" && cargo build --release --locked -p rhc-trader 2>&1 | tail -1)
    git -C "/tmp/b_$1" log --oneline -1
}
bangun main origin/main
bangun baru origin/perbaikan-siap

jalan(){
    say "backtest: $1"
    env -i PATH="$PATH" HOME="$HOME" START_USD=100 BELAJAR=1 CATBOOST_BIN=/opt/rhc/bin/catboost LAT_BLOCKS=3 $SYS_A \
        "/tmp/b_$2/target/release/rhc-trader" sim "$W/all" 65779486 90197095 "$O/trades_$1.jsonl" --bot "/tmp/b_$2/models" 2>&1 \
        | grep -E "^SIM|cek kausal" | sed "s/^/$1 | /" | tee -a "$O/equity.txt"
}
jalan main main
jalan main_ulang main
jalan baru baru

say "perbandingan trade demi trade"
{
    sha256sum "$O"/trades_*.jsonl | sed "s#$O/##"
    wc -l "$O"/trades_*.jsonl | sed "s#$O/##"
    if cmp -s "$O/trades_main.jsonl" "$O/trades_baru.jsonl"; then echo "IDENTIK: main dan perbaikan menghasilkan semua trade yang sama, byte demi byte"; else echo "BEDA: main dan perbaikan tidak sama"; fi
    if cmp -s "$O/trades_main.jsonl" "$O/trades_main_ulang.jsonl"; then echo "main diulang: identik"; else echo "main diulang: beda"; fi
} | tee -a "$O/equity.txt"
