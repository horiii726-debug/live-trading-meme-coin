# rhc-trader

Fully automatic live trading for the RHC memecoin strategy on **Robinhood Chain (chainId 4663, Uniswap v4)**.

The strategy decides everything (pool screening, minute-1 ML filter, quarter-Kelly sizing with compounding, exit
models); it is the backtested one, unchanged and proven byte-for-byte identical to the published backtest. The
execution layer, written in Rust, turns each decision into real Uniswap v4 transactions: it buys when the strategy
buys and sells when the strategy sells. There is no manual trading. Fund the wallet, switch `EXEC_MODE=live`, and the
bot trades by itself.

> Trading memecoins can lose the whole stake. The backtest is not a promise; real fills, competing bots and token
> behaviour differ from the simulation. Run `shadow` first, then `live` with a small amount.

## What is guaranteed, and how it is checked

| Claim | Check |
|---|---|
| Decisions are identical to the backtest | `scripts/parity.sh <data/all>` reproduces System A: 2,496 trades, $100 → $161,557.85, and the SHA-256 of every trade record |
| Calldata is exactly what Uniswap expects | golden test: the encoder rebuilds a real Universal Router buy from this chain byte for byte |
| Buy → approve → sell works on chain | `rhc-trader exec selftest` runs the full cycle with this binary's calldata in `eth_simulateV1` (nothing is sent) |
| Local quotes equal the official Quoter | unit test on recorded on-chain state, and the self-test at the current block |
| Fills are read correctly | unit tests on real buy and sell receipts |
| Signing is correct | unit test: signed transactions decode back and recover the signer |
| Every chain event reaches the strategy exactly once, in order | `loggate.rs` unit tests; live counters in `status.json` → `log_gate` (see below) |
| The whole loop works on live data | shadow run: strategy buy → simulated buy → take-profit 60% → exit → close, end to end |

```
cargo test --release                          # 38 unit tests
scripts/parity.sh /path/to/research/data/all  # backtest parity gate (dataset lives in the research repo)
target/release/rhc-trader exec selftest --env /opt/rhc/bot/.env
```

## How a trade flows

```
chain logs ──► engine (crates/engine, crates/features: backtested, unchanged)
                 │  Entry / Tp1 / ExitSignal events, trade closes
                 ▼
             account rules (crates/trader/src/bot.rs: quarter Kelly, compounding, brakes, kill switch)
                 │  ModelOrder: buy / take-profit / exit / closed   (live and shadow mode only)
                 ▼  in-process channel
             executor thread (crates/trader/src/exec)
                 ├─ buy:  Universal Router V4_SWAP (ETH -> token), then Permit2 approvals right behind it
                 ├─ take-profit: sell the same 60% the strategy sells
                 ├─ exit / close: sell everything; retries widen the tolerance until the position is gone
                 └─ receipts -> fills -> crash-safe ledger
```

Latency: no RPC call before a broadcast (local nonce, cached base fee, fixed gas limits, local AMM quote from the
engine's pool state, measured identical to the official Quoter). The signed transaction is sent to every RPC endpoint
in parallel. In the shadow run a strategy buy reached a signed, simulated transaction in 12 ms.

## Chain data: exactly once, in order

The public RPC's nodes are not in sync and answer outside the requested range (measured: with `fromBlock` above a
node's head it returns that head's logs; with `toBlock` above it, fewer blocks and no error). The original poll loop
stored about 2–3% of all events twice that way: an LP deposit counted twice made a drained pool look funded and the
strategy entered it. The poll now asks for the tip first, then for an explicit block range that re-reads the last 25
blocks; `loggate.rs` drops repeats and out-of-range logs and delivers a log a lagging node returned late at the next
block, so the engine never sees time go backwards. A private RPC that rejects ranges it does not have yet (Goldsky:
error -32014) gives an exact stream with no late logs at all. Older stores are replayed without their duplicate rows.

## Execution modes (`EXEC_MODE`)

| Mode | What happens |
|---|---|
| `off` (default) | paper trading only; identical to the research bot |
| `shadow` | every order is built, signed and simulated against live state; nothing is broadcast; results go to the ledger as `shadow` |
| `live` | real transactions from `EXEC_KEY_FILE`; requires `EXEC_LIVE_CONFIRM=I_ACCEPT_REAL_MONEY_RISK` |

### Sizing: quarter Kelly with compounding, on real money

The account sizes every position with the backtested rule: 2% of equity for the first 30 trades, 9.4% until 100,
then quarter Kelly clamped to 2–15%, scaled down in drawdowns, capped at 1% of pool liquidity and 50% total exposure.
With `EXEC_SIZING=real_equity` (default) the executor applies that same fraction to the **real** wallet equity
(`real size = model size × real equity / model equity`), so profits compound on the real balance and losses shrink
the next positions. The 1% LP cap still applies.

### Safety rails (they never change a decision; they can only skip a buy or force a sell)

- buys are skipped when: `STOP_BUYING` / `FLATTEN_ALL` file exists, daily realized loss limit hit, 5 failed buys in
  a row, base fee above limit, max open positions, not enough ETH (gas reserve kept), order older than 1.5 s,
  pool key does not hash to the pool id, non-native or dynamic-fee pool;
- sells never give up: each retry widens the tolerance (15% → 50% → any); rug/dump exits accept any output;
- a single buy can never exceed 20% of real equity (bug guard; the strategy's own cap is 15%);
- after a restart the stored history is replayed without trading, then real positions the strategy no longer holds
  are sold;
- every transaction is written to the ledger before it is broadcast;
- with 0 ETH in the wallet buys are simply skipped; once you fund it the bot starts trading within seconds.

## Repository layout

| Path | Content |
|---|---|
| `crates/engine` | Uniswap v4 replay/exit engine, unchanged |
| `crates/features` | minute-1 feature extraction, unchanged |
| `crates/trader` | the bot (`live.rs`, `bot.rs`, `sim.rs`, `radar.rs`, ...) and `src/exec/`, the execution layer |
| `models/` | the models the backtest used (`filter_cat.json`, `danger_model.json`, `pnaik_model.json`) + parity check files |
| `training/` | quality-gated daily retraining (`latih_ulang.py`) and its historical training data (`data/arena_feats.csv`) |
| `config/trader.env.example` | every setting, documented |
| `deploy/` | systemd units and `install.sh` for a VPS |
| `scripts/parity.sh` | backtest parity gate |
| `docs/RUNBOOK.md` | setup, going live, monitoring, emergencies |

The large historical swap dataset stays in the research repository; this repository only carries what the live bot and
its retraining need.

The vendored engine keeps its original Indonesian identifiers and log strings (they are part of the parity
contract and of the training data). Glossary: *beli* buy, *jual* sell, *tutup* close, *tolak* reject,
*akun* account, *kas* cash, *gerbang* buy gate, *saring* screen, *skor* score, *model bahaya* danger model,
*tanda pucuk* peak signal, *rug terdeteksi* rug detected, *rem* drawdown brake, *stop darurat* kill switch.

## Contracts (verified 2026-10-18 against the official Uniswap list and on chain)

| Contract | Address |
|---|---|
| PoolManager | `0x8366a39cc670b4001a1121b8f6a443a643e40951` |
| Universal Router 2.1.2 | `0x204faca1764b154221e35c0d20abb3c525710498` |
| Permit2 | `0x000000000022D473030F116dDEE9F6B43aC78BA3` |
| V4Quoter | `0x8dc178efb8111bb0973dd9d722ebeff267c98f94` |
| StateView | `0xf3334192d15450cdd385c8b70e03f9a6bd9e673b` |

The executor re-checks chain id, contract code and `UniversalRouter.poolManager()` at every start and refuses to run
if anything changed.
