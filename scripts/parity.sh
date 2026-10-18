#!/usr/bin/env bash
# Parity gate: the trader's decision engine must reproduce the published System A backtest exactly.
#
# Runs `rhc-trader sim` over the continuous 18 Sep - 16 Oct 2026 dataset with the System A settings and the
# walk-forward entry scores, then compares the summary line with the published result. Any difference fails.
#
# Usage: scripts/parity.sh <dataset_dir>   (the merged `data/all` folder of the research repo)
set -euo pipefail

DATA=${1:?usage: scripts/parity.sh <dataset_dir>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${BIN:-$ROOT/target/release/rhc-trader}
MODELS=${MODELS:-$ROOT/models}
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

FROM=65779486
TO=90197095
EXPECTED='beli 2496 | tutup 2496 | win 78.0% | PF 2.50 | akun $100.00 -> $161557.85 | DD maks 25.2%'
THR0=$(python3 -c "import json,sys; print('%.6f' % json.load(open(sys.argv[1]))['thr_start'])" "$MODELS/filter_cat.json")

[ -s "$DATA/rscore.csv" ] || { echo "missing $DATA/rscore.csv (walk-forward scores)"; exit 2; }

# System A settings exactly as published. MAX_FEE is pinned to the backtest value (0.5%).
line=$(env -i PATH="$PATH" HOME="$HOME" \
  QTHR=0.80 MAX_POS=10 DANGER_TH=0.5 TRAIL_PUNCAK=0.4 TUTUP_SAAT_RUG=1 DUMP_FRAC=0.05 KELLY_MAKS=0.15 MAX_FEE=5000 \
  "$BIN" sim "$DATA" "$FROM" "$TO" "$OUT/trades.jsonl" --bot "$MODELS" --skor "$DATA/rscore.csv" --thr0 "$THR0" \
  | grep -o 'beli [0-9]* | tutup [0-9]* | win [0-9.]*% | PF [0-9.inf]* | akun \$[0-9.]* -> \$[0-9.]* | DD maks [0-9.]*%')

# Byte-level fingerprint of every trade record (the summary line alone could hide offsetting differences).
EXPECTED_SHA=79e603a9886c297fd3a45088aeff1eac8e94419ec8ab9c831efdc68734d55884
sha=$(sha256sum "$OUT/trades.jsonl" | awk '{print $1}')

echo "result:   $line"
echo "expected: $EXPECTED"
echo "trades sha256: $sha (expected $EXPECTED_SHA)"
if [ "$line" = "$EXPECTED" ] && [ "$sha" = "$EXPECTED_SHA" ]; then
  echo "PARITY OK"
else
  echo "PARITY FAILED"
  exit 1
fi
