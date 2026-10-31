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
git worktree add -q /tmp/b_main origin/main
python3 - <<'PY'
p = '/tmp/b_main/crates/trader/src/bot.rs'
s = open(p, encoding='utf-8').read()
a = '        if self.killed {\n            return self.tolak("stop darurat");'
a = a if a in s else a.replace('\n', '\r\n')
assert s.count(a) == 1
b = '        if sim.m.hooks.trim_start_matches("0x").chars().any(|c| c != \'0\') {\n            return self.tolak("hook");\n        }\n'
open(p, 'w', encoding='utf-8').write(s.replace(a, b + a))
PY
(cd /tmp/b_main && git log --oneline -1 && cargo build --release --locked -p rhc-trader 2>&1 | tail -1)
sed -e "s#^START_USD=.*#START_USD=100#" -e "s#^KELLY_SEED=.*#KELLY_SEED=/tmp/b_main/models/kelly_seed_backtest_last100.json#" /tmp/b_main/config/trader.env.example > "$W/sim.env"
grep -nE "^(START_USD|KELLY_SEED|LAT_BLOCKS|MAX_POS|MAX_POS_AWAL|BELAJAR|QTHR)" "$W/sim.env"
BELAJAR=1 CATBOOST_BIN=/opt/rhc/bin/catboost /tmp/b_main/target/release/rhc-trader sim "$W/all" 65779486 90197095 "$O/trades_terpasang.jsonl" --bot /tmp/b_main/models --env "$W/sim.env" 2>&1     | grep -E "^SIM|cek kausal" | tee -a "$O/equity.txt"
