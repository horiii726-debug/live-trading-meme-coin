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

# service manager: systemd on a VPS, supervisor inside a container (Docker, Vast.ai, LXC)
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null; then
    install -m 644 "$ROOT"/deploy/*.service "$ROOT"/deploy/*.timer /etc/systemd/system/
    systemctl daemon-reload
    START="systemctl enable --now rhc-trader rhc-retrain.timer"
    LOGS="journalctl -fu rhc-trader"
elif command -v supervisorctl >/dev/null; then
    install -d -m 755 /etc/supervisor/conf.d /var/log/portal
    install -m 644 "$ROOT/deploy/rhc-trader.supervisor.conf" /etc/supervisor/conf.d/rhc-trader.conf
    supervisorctl reread >/dev/null 2>&1 || true
    supervisorctl update >/dev/null 2>&1 || true
    # no timers under supervisor: retrain runs from cron instead
    if command -v crontab >/dev/null; then
        TAB=$(crontab -l 2>/dev/null | grep -v latih_ulang.py || true)
        printf '%s\n0 3 * * * cd /opt/rhc/training && BOT_DIR=/opt/rhc/bot DATA_DIR=/opt/rhc/training/data /opt/rhc/venv/bin/python /opt/rhc/training/latih_ulang.py >> /var/log/portal/retrain.log 2>&1\n' "$TAB" | crontab -
    fi
    START="supervisorctl start rhc-trader"
    LOGS="tail -f /var/log/portal/rhc-trader.log"
else
    START="(no systemd or supervisor found — run rhc-trader live yourself)"
    LOGS="see <bot>/rust_live/live.log"
fi

echo "installed."
echo "Next:"
echo "  1. sudo deploy/setup.sh                              # RPC, wallet, limits, mode"
echo "  2. /opt/rhc/bin/rhc-trader exec selftest --env /opt/rhc/bot/.env"
echo "  3. $START"
echo "  4. $LOGS"
