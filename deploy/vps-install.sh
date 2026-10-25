#!/usr/bin/env bash
# One command from a bare Ubuntu/Debian server to a running bot.
#
#   sudo deploy/vps-install.sh
#
# Installs the build toolchain and Rust if missing, builds the binary, installs it under /opt/rhc,
# registers the service (systemd on a VPS, supervisor in a container), and then applies
# config/isi-disini.env if you already filled it in.
#
# Safe to re-run: it never overwrites an existing key file, and never creates or prints a key.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
[ "$(id -u)" -eq 0 ] || { echo "jalankan sebagai root: sudo deploy/vps-install.sh"; exit 1; }

say(){ printf '\n>>> %s\n' "$*"; }

say "1/4  paket sistem"
if command -v apt-get >/dev/null; then
    # apt-get update ignores DPkg::Lock::Timeout (it takes the lists lock): retry while first-boot updates run
    for i in $(seq 1 90); do apt-get -o DPkg::Lock::Timeout=600 update -qq && break; [ "$i" -lt 90 ] || exit 1; sleep 10; done
    DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=600 install -y --no-install-recommends \
        build-essential pkg-config libssl-dev python3-venv ca-certificates curl git cron >/dev/null
fi

say "2/4  Rust"
BUILD_USER=${SUDO_USER:-root}
BUILD_HOME=$(getent passwd "$BUILD_USER" | cut -d: -f6)
CARGO=$(command -v cargo || echo "$BUILD_HOME/.cargo/bin/cargo")
if [ ! -x "$CARGO" ]; then
    echo "    memasang rustup untuk $BUILD_USER ..."
    if [ "$BUILD_USER" = root ]; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
    else
        sudo -u "$BUILD_USER" -H sh -c \
          "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal" >/dev/null
    fi
    CARGO="$BUILD_HOME/.cargo/bin/cargo"
fi
# cargo is a rustup proxy: run it as the user it was installed for (as root it finds no toolchain)
if [ "$BUILD_USER" = root ]; then "$CARGO" --version; else sudo -u "$BUILD_USER" -H "$CARGO" --version; fi \
    || { echo "Rust gagal dipasang"; exit 1; }

say "3/4  build dan pasang"
"$ROOT/deploy/install.sh"

say "4/4  konfigurasi"
FORM=$ROOT/config/isi-disini.env
[ -f "$FORM" ] || install -m 600 "$FORM.example" "$FORM"
if [ -f "$FORM" ] && grep -qE '^(RPC_API_KEY|WALLET_PRIVATE_KEY)=.+' "$FORM"; then
    "$ROOT/deploy/pasang.sh"
else
    cat <<EOF

    Formulir belum diisi.

    Buka:   $FORM
    Isi:    KOLOM 1 (URL RPC berbayar)  dan  KOLOM 2 (private key MetaMask)
    Lalu:   sudo deploy/pasang.sh

    Atau pakai wizard tanya-jawab:   sudo deploy/setup.sh

EOF
fi
