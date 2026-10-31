# Runbook

## 1. Install on a VPS

Ubuntu/Debian, 2 vCPU, 4 GB RAM, no GPU. Install Rust (https://rustup.rs) and git, clone this repository, then:

```
sudo deploy/install.sh
sudo -u rhc nano /opt/rhc/bot/.env        # set RPC_URLS (your paid RPC key first); leave EXEC_MODE=off for now
```

## 2. Wallet (you do this yourself)

Use a **new wallet used only by the bot**. Fund it only with what you are prepared to lose. Write the private key into
the key file without it touching your shell history:

```
sudo -u rhc sh -c 'umask 077; cat > /opt/rhc/secrets/wallet.key'
# paste the 64-hex private key, press Enter, then Ctrl-D
sudo -u rhc /opt/rhc/bin/rhc-trader wallet address --env /opt/rhc/bot/.env   # prints the address and ETH balance
```

The bot refuses a key file that other users can read. Keep an offline backup of the key.

## 3. Verify before any money moves

```
cargo test --release                                            # in the repository
sudo -u rhc /opt/rhc/bin/rhc-trader exec selftest --env /opt/rhc/bot/.env
```
The self-test must end with `SELFTEST OK`. It simulates buy → approve → sell with this binary's calldata; it sends
nothing and needs no funds.

## 4. Paper, then shadow

```
sudo systemctl enable --now rhc-trader
```
`EXEC_MODE=off` first, until the warm-up (30 scored coins) is done and trades look like the backtest. Then set
`EXEC_MODE=shadow` and restart: every strategy buy and sell is simulated against live state and recorded in the
ledger. Compare `real_ret` with `model_ret` per trade in `/opt/rhc/bot/rust_live/exec/closed.jsonl`; the backtest
stress test shows the edge disappears once execution is about 20% worse than modelled.

## 5. Live

1. Fund the bot wallet with a small amount (e.g. $100 of ETH; keep at least 0.003 ETH for gas).
2. In `.env`: `EXEC_MODE=live` and `EXEC_LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK`.
3. `sudo systemctl restart rhc-trader`. `live.log` must show `EXEC: executor started: mode=live wallet=0x…`.
4. From then on the bot buys and sells by itself. Watch it with:
   ```
   sudo ./rhc.sh status     # WALLET NYATA: real balance and results
   sudo ./rhc.sh log        # EXEC: lines are real transactions, TUTUP lines are the simulated strategy account
   ```
   Telegram alerts (TELEGRAM_TOKEN / TELEGRAM_CHAT_ID) report starts, stuck positions, the daily loss stop and
   emergency stops.

## 6. Changing the RPC

Edit `RPC_URLS` (chain data) and `EXEC_RPC_URLS` (transactions) in `/opt/rhc/bot/.env`, private endpoint first, then
`sudo systemctl restart rhc-trader`. No code change is needed. Check afterwards in
`/opt/rhc/bot/rust_live/status.json`:

| Field | Healthy |
|---|---|
| `lag_blocks` | 0–5 |
| `log_gate.duplicate_dropped`, `log_gate.outside_range_dropped` | grow only on RPCs whose nodes are out of sync (the public one); the gate drops them, nothing to do |
| `log_gate.late_delivered` | 0 or rare; frequent values mean the RPC's nodes lag by seconds: use another RPC |
| `log_gate.range_cap_blocks` | the largest getLogs range the RPC accepts (halved automatically when it refuses one) |
| `last_error` | empty most of the time; `429 Too Many Requests` often means the RPC plan is too small |

## 7. Emergency stops

| Need | Do |
|---|---|
| stop new buys, keep managing open positions | `sudo -u rhc touch /opt/rhc/bot/rust_live/exec/STOP_BUYING` (delete it to resume) |
| sell everything now | `sudo -u rhc touch /opt/rhc/bot/rust_live/exec/FLATTEN_ALL` (delete it to trade again) |
| stop the bot completely | `sudo ./rhc.sh stop` (stays off after a reboot; open positions stay open: flatten first if needed) |

Files in `/opt/rhc/bot/rust_live/exec/`: `positions.json` (open positions, rewritten before every broadcast),
`closed.jsonl` (history with real and model returns), `journal.jsonl` (every order, transaction, receipt and skip),
`exec_status.json` (current state).

## 8. Recovery

- **Restart / crash**: start the service again. The strategy replays its stored history without trading it, the
  executor reloads `positions.json`, checks receipts of in-flight transactions, and sells real positions the
  strategy no longer holds.
- **`positions.json is corrupt`**: the executor refuses to start on purpose. Rebuild it from `journal.jsonl` before
  trading again; never delete it while positions are open.
- **Position `stuck`**: the token reverted on approve or on repeated sells. The executor simulates again every 30 s
  (free) and sends only when it would succeed; check the token on the explorer.
- **Strategy stop (`STOP DARURAT`)**: the strategy account fell past KILL_DD and no longer buys, for good. Real positions
  are still sold. To start a new account: `sudo ./rhc.sh stop`, `sudo ./rhc.sh bersih` (answer y), `sudo ./rhc.sh mulai`.
- **Disk**: `rhc.sh status` shows free space. A full disk stops the ledger, which fires the kill switch and stops sells.

## 9. Retraining (automatic, inside the bot)

The entry filter is adaptive (`crates/trader/src/belajar.rs`): score = (CatBoost + deep-learning network) / 2. The
network learns from every coin's outcome as soon as its shadow trade closes. The CatBoost half is retrained by
`/opt/rhc/bin/catboost` (the official C++ program, installed by `deploy/catboost.sh`) when the last 20 known picks go bad
(win < 65% or rug > 20%) or after 300 new outcomes, in a background thread at low CPU priority; the new model is used
only after the Rust predictor matched CatBoost's own predictions on 500 rows, and is saved to `filter_cat.json` (the
old one goes to `arsip_model/`). The learner's state is `rust_live/belajar.json`. Log lines: `filter adaptif AKTIF`
at start, `LATIH ULANG ML mulai`, `MODEL ML BARU dipasang`. `BELAJAR=0` in `.env` freezes the filter (emergency only).
