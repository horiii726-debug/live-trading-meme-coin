#!/usr/bin/env bash
# The official CatBoost command-line program (C++), which the bot runs to retrain the ML half of its adaptive
# filter (crates/trader/src/belajar.rs). Pinned release, checked against the sha256 GitHub publishes for it.
# Idempotent: nothing is downloaded when the right file is already there.
#   deploy/catboost.sh [install path]        default /opt/rhc/bin/catboost
set -euo pipefail
DEST=${1:-/opt/rhc/bin/catboost}
VER=1.2.10
SHA=478dc57f4c19de205b19b709fd4c6af93f79753dc462b6bb49d33c920dddec75
URL=https://github.com/catboost/catboost/releases/download/v$VER/catboost-linux-x86_64-$VER

[ "$(uname -m)" = x86_64 ] || { echo "    CatBoost: hanya untuk x86_64 (mesin ini $(uname -m))"; exit 1; }
if [ -x "$DEST" ] && echo "$SHA  $DEST" | sha256sum -c --status 2>/dev/null; then
    echo "    CatBoost $VER sudah terpasang"
    exit 0
fi
install -d -m 755 "$(dirname "$DEST")"
tmp=$(mktemp "$DEST.unduh.XXXXXX")
trap 'rm -f "$tmp"' EXIT
echo "    mengunduh CatBoost $VER (±290 MB) ..."
ok=""
for i in 1 2 3; do
    if curl -fsSL --retry 3 --connect-timeout 20 -o "$tmp" "$URL"; then ok=1; break; fi
    sleep $((i * 5))
done
[ -n "$ok" ] || { echo "    CatBoost: unduhan gagal ($URL)"; exit 1; }
echo "$SHA  $tmp" | sha256sum -c --status || { echo "    CatBoost: sha256 tidak cocok, berkas dibuang"; exit 1; }
chmod 755 "$tmp"
mv -f "$tmp" "$DEST"
trap - EXIT
echo "    CatBoost $VER terpasang: $DEST"
