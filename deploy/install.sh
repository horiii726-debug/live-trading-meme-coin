#!/usr/bin/env bash
# Install rhc-trader on an Ubuntu/Debian VPS under /opt/rhc (run as root from the repo root: sudo deploy/install.sh).
# Idempotent. Never creates, reads or prints a private key: you write /opt/rhc/secrets/wallet.key yourself.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
[ "$(id -u)" -eq 0 ] || { echo "run as root: sudo deploy/install.sh"; exit 1; }

# system packages: C toolchain + OpenSSL headers (TLS) for the Rust build, Python venv for the retrain job
if command -v apt-get >/dev/null; then
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev python3-venv ca-certificates >/dev/null
fi

id rhc >/dev/null 2>&1 || useradd --system --home /opt/rhc --shell /usr/sbin/nologin rhc
install -d -o rhc -g rhc -m 755 /opt/rhc /opt/rhc/bin /opt/rhc/bot /opt/rhc/training
install -d -o rhc -g rhc -m 700 /opt/rhc/secrets

# build as the user who ran sudo (rustup installs cargo in that user's home, which is not on root's PATH)
BUILD_USER=${SUDO_USER:-root}
BUILD_HOME=$(getent passwd "$BUILD_USER" | cut -d: -f6)
CARGO=$(command -v cargo || echo "$BUILD_HOME/.cargo/bin/cargo")
[ -x "$CARGO" ] || { echo "install Rust first (as $BUILD_USER): https://rustup.rs"; exit 1; }
if [ "$BUILD_USER" = root ]; then
    "$CARGO" build --release --manifest-path "$ROOT/Cargo.toml"
else
    sudo -u "$BUILD_USER" -H "$CARGO" build --release --manifest-path "$ROOT/Cargo.toml"
fi
install -m 755 "$ROOT/target/release/rhc-trader" /opt/rhc/bin/rhc-trader

# models: copied once; later the retrain job replaces filter_cat.json in place (quality gated)
for f in "$ROOT"/models/*; do [ -e "/opt/rhc/bot/$(basename "$f")" ] || install -o rhc -g rhc -m 644 "$f" /opt/rhc/bot/; done
cp -r "$ROOT"/training/* /opt/rhc/training/ && chown -R rhc:rhc /opt/rhc/training
[ -e /opt/rhc/bot/.env ] || install -o rhc -g rhc -m 600 "$ROOT/config/trader.env.example" /opt/rhc/bot/.env

[ -x /opt/rhc/venv/bin/python ] || python3 -m venv /opt/rhc/venv
/opt/rhc/venv/bin/pip install --quiet --upgrade pip
/opt/rhc/venv/bin/pip install --quiet numpy pandas scikit-learn catboost

install -m 644 "$ROOT"/deploy/*.service "$ROOT"/deploy/*.timer /etc/systemd/system/
systemctl daemon-reload
echo "installed. Next: edit /opt/rhc/bot/.env, run 'rhc-trader exec selftest', then:"
echo "  systemctl enable --now rhc-trader rhc-retrain.timer"
