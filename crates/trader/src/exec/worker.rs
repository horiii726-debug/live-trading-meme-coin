//! The execution thread: model orders in, transactions out, fills back into the ledger.
//!
//! Order flow (all decided by the unchanged strategy engine):
//! - `Buy`        the account opened a position (engine `Entry` event)        -> buy now
//! - `TakeProfit` the engine sold its take-profit fraction (`Tp1` event)       -> sell the same fraction now
//! - `Exit`       the engine signalled an exit (`ExitSignal` event)            -> sell everything now
//! - `Closed`     the engine closed the trade (any path, incl. the 9 h horizon) -> make sure we are flat
//!
//! Safety: buys are optional (skipped whenever a rail says no); sells are mandatory (retried with wider
//! tolerance until the position is gone). Every transaction is written to the ledger before it is broadcast.
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::{hex, Address, Bytes, B256, U256};
use alloy_signer_local::PrivateKeySigner;
use serde_json::{json, Value};

use super::abi::revert_name;
use super::amm::{self, PoolState};
use super::chain::{self, ETH_USDG_POOL_ID, UNIVERSAL_ROUTER, V4_QUOTER};
use super::pools;
use super::config::{ExecConfig, Mode, Sizing};
use super::fills;
use super::ledger::{now_ms, u256_f64, Ledger, PosStatus, Position, SellIntent, TxPurpose, TxRec, TxState};
use super::sender::{SendOutcome, Sender, SignedTx};
use super::uniswap::{self, Key};
use super::wallet;

pub type Alert = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Debug)]
pub enum OrderKind {
    /// `size_usd`: the account's quarter-Kelly size; `model_equity_usd`: the account equity it was computed from;
    /// `lp_cap_usd`: the strategy's per-position cap (LP_CAP x pool liquidity).
    Buy { size_usd: f64, model_equity_usd: f64, lp_cap_usd: f64 },
    TakeProfit { frac: f64 },
    Exit { why: String },
    Closed { reason: String, ret: f64 },
}

#[derive(Clone, Debug)]
pub struct Order {
    pub pool: String,
    pub kind: OrderKind,
    pub key: Option<Key>,
    pub state: Option<PoolState>,
    pub eth_usd: f64,
    pub created: Instant,
}

pub enum Cmd {
    Order(Order),
    Reconcile { model_open: Vec<String> },
}

#[derive(Debug, PartialEq, Eq)]
enum SendResult {
    Accepted,
    NonceResynced,
    Failed,
}

#[derive(Clone)]
pub struct ExecHandle {
    tx: mpsc::Sender<Cmd>,
}

impl ExecHandle {
    pub fn submit(&self, o: Order) -> bool {
        self.tx.send(Cmd::Order(o)).is_ok()
    }
    pub fn reconcile(&self, model_open: Vec<String>) -> bool {
        self.tx.send(Cmd::Reconcile { model_open }).is_ok()
    }
}

/// Exit reasons after which any output is acceptable (the engine treats them as emergencies).
pub fn is_emergency(reason: &str) -> bool {
    let r = reason.to_lowercase();
    r.contains("rug") || r.contains("lp ditarik") || r.contains("dump") || r.contains("bencana") || r.contains("flatten")
}

const STOP_FILE: &str = "STOP_BUYING";
const FLATTEN_FILE: &str = "FLATTEN_ALL";

fn hx(v: U256) -> String {
    format!("{v:#x}")
}

fn unix_now() -> u64 {
    now_ms() / 1000
}

pub struct Executor {
    cfg: ExecConfig,
    sender: Sender,
    ledger: Ledger,
    rx: Receiver<Cmd>,
    alert: Alert,
    chain_balance: U256,
    eth_usd: f64,
    states: HashMap<String, PoolState>,
    raw: HashMap<B256, Vec<u8>>,
    consecutive_buy_fail: u32,
    last_buy_fail: Option<Instant>,
    last_error: String,
    alerted: std::collections::HashSet<String>,
    counters: std::collections::BTreeMap<String, u64>,
    started: Instant,
    t_receipts: Instant,
    t_fee: Instant,
    t_bal: Instant,
    t_status: Instant,
    t_kill: Instant,
    t_px: Instant,
    flatten_seen: bool,
}

pub fn start(cfg: ExecConfig, alert: Alert) -> Result<Option<ExecHandle>, String> {
    if cfg.mode == Mode::Off {
        return Ok(None);
    }
    let signer = match &cfg.key_file {
        Some(p) => wallet::load_signer(p)?,
        None => PrivateKeySigner::random(),
    };
    let mut sender = Sender::new(signer, &cfg.rpc_urls, cfg.doh)?;
    chain::verify_deployment(&sender.reads)?;
    sender.refresh_base_fee()?;
    sender.sync_nonce()?;
    let chain_balance = if cfg.mode == Mode::Live { sender.eth_balance()? } else { U256::ZERO };
    let ledger = Ledger::open(&cfg.state_dir)?;
    let (tx, rx) = mpsc::channel();
    let handle = ExecHandle { tx };
    let mut ex = Executor {
        cfg: cfg.clone(),
        sender,
        ledger,
        rx,
        alert: alert.clone(),
        chain_balance,
        eth_usd: f64::NAN,
        states: HashMap::new(),
        raw: HashMap::new(),
        consecutive_buy_fail: 0,
        last_buy_fail: None,
        last_error: String::new(),
        alerted: Default::default(),
        counters: Default::default(),
        started: Instant::now(),
        t_receipts: Instant::now(),
        t_fee: Instant::now(),
        t_bal: Instant::now(),
        t_status: Instant::now() - Duration::from_secs(10),
        t_kill: Instant::now() - Duration::from_secs(10),
        t_px: Instant::now() - Duration::from_secs(60),
        flatten_seen: false,
    };
    ex.refresh_eth_usd();
    ex.write_status();
    alert(format!(
        "executor started: mode={} wallet={:#x} balance={:.6} ETH open_positions={}",
        cfg.mode.as_str(),
        ex.sender.address,
        u256_f64(ex.chain_balance) / 1e18,
        ex.ledger.positions.len()
    ));
    std::thread::Builder::new().name("rhc-exec".into()).spawn(move || ex.run()).map_err(|e| e.to_string())?;
    Ok(Some(handle))
}

impl Executor {
    fn live(&self) -> bool {
        self.cfg.mode == Mode::Live
    }

    fn run(mut self) {
        loop {
            let wait = if self.any_pending() { Duration::from_millis(40) } else { Duration::from_millis(200) };
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => {
                    self.guarded(|s| s.handle(cmd));
                    while let Ok(c) = self.rx.try_recv() {
                        self.guarded(|s| s.handle(c));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = self.ledger.save();
                    return;
                }
            }
            self.guarded(|s| s.tick());
        }
    }

    fn guarded(&mut self, f: impl FnOnce(&mut Self)) {
        if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            self.last_error = format!("panic: {msg}");
            (self.alert)(format!("executor panic (recovered): {msg}"));
        }
    }

    fn bump(&mut self, k: &str) {
        *self.counters.entry(k.to_string()).or_insert(0) += 1;
    }

    fn skip(&mut self, pool: &str, why: &str) {
        self.bump(&format!("buy_skipped: {why}"));
        self.ledger.journal("buy_skipped", json!({"pool": pool, "why": why}));
    }

    fn alert_once(&mut self, key: &str, msg: String) {
        if self.alerted.insert(key.to_string()) {
            (self.alert)(msg);
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Order(o) => {
                if o.eth_usd.is_finite() && o.eth_usd > 0.0 {
                    self.eth_usd = o.eth_usd;
                }
                if let Some(s) = o.state {
                    self.states.insert(o.pool.clone(), s);
                }
                self.ledger.journal("order", json!({"pool": o.pool, "kind": format!("{:?}", o.kind), "age_ms": o.created.elapsed().as_millis() as u64}));
                match o.kind.clone() {
                    OrderKind::Buy { size_usd, model_equity_usd, lp_cap_usd } => self.on_buy(&o, size_usd, model_equity_usd, lp_cap_usd),
                    OrderKind::TakeProfit { frac } => self.set_intent(&o.pool, false, frac, "take profit", false),
                    OrderKind::Exit { why } => {
                        let em = is_emergency(&why);
                        self.set_intent(&o.pool, true, 1.0, &why, em)
                    }
                    OrderKind::Closed { reason, ret } => self.on_model_closed(&o.pool, &reason, ret),
                }
            }
            Cmd::Reconcile { model_open } => self.reconcile(&model_open),
        }
    }

    // ---------------------------------------------------------------- buys

    fn buy_block_reason(&self) -> Option<String> {
        let d = &self.cfg.state_dir;
        if d.join(STOP_FILE).exists() {
            return Some("STOP_BUYING file present".into());
        }
        if d.join(FLATTEN_FILE).exists() {
            return Some("FLATTEN_ALL file present".into());
        }
        if self.cfg.daily_loss_limit_usd > 0.0 && self.ledger.today_realized_usd() < -self.cfg.daily_loss_limit_usd {
            return Some(format!("daily loss limit ${:.2} reached", self.cfg.daily_loss_limit_usd));
        }
        if self.consecutive_buy_fail >= 5 {
            return Some("5 consecutive failed buys (pauses buying for 10 minutes)".into());
        }
        if self.sender.base_fee_gwei() > self.cfg.max_base_fee_gwei {
            return Some(format!("base fee {:.4} gwei above limit", self.sender.base_fee_gwei()));
        }
        let open = self.ledger.positions.values().filter(|p| p.is_live()).count();
        if open >= self.cfg.max_open_positions {
            return Some("max open positions".into());
        }
        None
    }

    /// Wallet ETH not committed to pending transactions.
    fn available_wei(&self) -> U256 {
        let mut out = U256::ZERO;
        for p in self.ledger.positions.values() {
            for t in &p.txs {
                if t.state == TxState::Pending {
                    let gas = U256::from(self.cfg.gas_limit_buy).saturating_mul(U256::from(self.sender.max_fee()));
                    out = out.saturating_add(gas).saturating_add(if t.purpose == TxPurpose::Buy { t.eth } else { U256::ZERO });
                }
            }
        }
        self.chain_balance.saturating_sub(out)
    }

    /// Wallet ETH plus open positions at cost, in USD (same "equity at cost" the strategy account sizes from).
    fn real_equity_usd(&self, eth_usd: f64) -> f64 {
        let held: f64 = self.ledger.positions.values().filter(|p| !p.shadow && matches!(p.status, PosStatus::Open | PosStatus::Stuck)).map(|p| u256_f64(p.eth_in)).sum();
        (u256_f64(self.chain_balance) + held) / 1e18 * eth_usd
    }

    /// Real order size. RealEquity sizing applies the account's quarter-Kelly fraction to the real equity, so the
    /// position compounds on real money; then the strategy's LP cap, the optional USD cap and the bug guard apply.
    fn real_order_usd(&self, model_usd: f64, model_equity_usd: f64, lp_cap_usd: f64, eth_usd: f64) -> f64 {
        let real_eq = self.real_equity_usd(eth_usd);
        let base = match self.cfg.sizing {
            Sizing::RealEquity if self.live() && model_equity_usd > 0.0 && real_eq > 0.0 => model_usd * real_eq / model_equity_usd,
            _ => model_usd,
        };
        let mut usd = base * self.cfg.size_scale;
        if lp_cap_usd.is_finite() && lp_cap_usd > 0.0 {
            usd = usd.min(lp_cap_usd);
        }
        if self.cfg.max_order_usd > 0.0 {
            usd = usd.min(self.cfg.max_order_usd);
        }
        if self.live() {
            usd = usd.min(self.cfg.max_position_frac * real_eq);
        }
        usd
    }

    fn on_buy(&mut self, o: &Order, size_usd: f64, model_equity_usd: f64, lp_cap_usd: f64) {
        let pool = o.pool.to_lowercase();
        if self.ledger.positions.contains_key(&pool) {
            return self.skip(&pool, "already have a position in this pool");
        }
        if let Some(r) = self.buy_block_reason() {
            return self.skip(&pool, &r);
        }
        let Some(key) = o.key else { return self.skip(&pool, "pool key unknown") };
        if let Err(e) = key.validate(&pool) {
            return self.skip(&pool, &format!("pool key rejected: {e}"));
        }
        let Some(st) = o.state else { return self.skip(&pool, "no pool state") };
        if o.created.elapsed() > Duration::from_millis(self.cfg.max_buy_delay_ms) {
            return self.skip(&pool, "order too old (price would be stale)");
        }
        let eth_usd = if o.eth_usd.is_finite() && o.eth_usd > 0.0 { o.eth_usd } else { self.eth_usd };
        if !(eth_usd.is_finite() && eth_usd > 0.0) {
            return self.skip(&pool, "ETH price unknown");
        }
        let usd = self.real_order_usd(size_usd, model_equity_usd, lp_cap_usd, eth_usd);
        if !(usd >= self.cfg.min_order_usd) {
            return self.skip(&pool, "below minimum order size");
        }
        let amount_in = (usd / eth_usd * 1e18).floor() as u128;
        if self.live() {
            let gas_budget = U256::from(self.cfg.gas_limit_buy + 2 * self.cfg.gas_limit_approve).saturating_mul(U256::from(self.sender.max_fee()));
            let reserve = U256::from((self.cfg.gas_reserve_eth * 1e18) as u128);
            if self.available_wei() < U256::from(amount_in).saturating_add(gas_budget).saturating_add(reserve) {
                return self.skip(&pool, "insufficient ETH balance");
            }
        }
        let Some(expected) = amm::buy_out(&st, amount_in as f64) else { return self.skip(&pool, "bad pool state for quote") };
        let min_out = amm::min_out(expected, self.cfg.buy_slippage_bps);
        if min_out == 0 {
            return self.skip(&pool, "quote too small");
        }
        let deadline = U256::from(unix_now() + self.cfg.deadline_secs);
        let data = match uniswap::buy_calldata(&key, amount_in, min_out, deadline) {
            Ok(d) => d,
            Err(e) => return self.skip(&pool, &format!("calldata: {e}")),
        };
        let mut pos = Position {
            pool: pool.clone(),
            key,
            shadow: !self.live(),
            manual: false,
            status: PosStatus::Buying,
            opened_ms: now_ms(),
            closed_ms: None,
            model_size_usd: size_usd,
            size_usd: usd,
            eth_usd_entry: eth_usd,
            eth_in: U256::from(amount_in),
            tokens_bought: U256::ZERO,
            tokens_held: U256::ZERO,
            eth_out: U256::ZERO,
            gas_wei: U256::ZERO,
            approvals_ready: false,
            approvals_since_ms: 0,
            tp_done: false,
            txs: Vec::new(),
            intent: None,
            model_close_reason: None,
            model_ret: None,
            last_error: None,
            realized_usd: None,
        };
        if !self.live() {
            // shadow: simulate the exact buy against live state, never broadcast
            match self.simulate_buy(&key, &pool, amount_in, &data) {
                Ok((tokens, eth_in)) => {
                    pos.status = PosStatus::Open;
                    pos.tokens_bought = tokens;
                    pos.tokens_held = tokens;
                    pos.eth_in = eth_in;
                    pos.approvals_ready = true;
                    self.bump("shadow_buys");
                }
                Err(e) => {
                    pos.status = PosStatus::BuyFailed;
                    pos.last_error = Some(e);
                    self.bump("shadow_buys_failed");
                }
            }
            self.ledger.journal("shadow_buy", json!({"pool": pool, "usd": usd, "amount_in": amount_in.to_string(), "expected_tokens": expected, "min_out": min_out.to_string(), "status": pos.status, "tokens": pos.tokens_bought.to_string(), "error": pos.last_error}));
            let done = pos.status == PosStatus::BuyFailed;
            self.ledger.positions.insert(pool.clone(), pos);
            if done {
                self.ledger.archive(&pool, eth_usd);
            }
            let _ = self.ledger.save();
            return;
        }
        // live: the position is in the book before its buy is signed; the buy is in the book before it is broadcast
        self.ledger.positions.insert(pool.clone(), pos);
        let t0 = Instant::now();
        let sent = self.sign_record_send(&pool, UNIVERSAL_ROUTER, U256::from(amount_in), &data, self.cfg.gas_limit_buy, TxPurpose::Buy, U256::from(min_out), U256::from(amount_in), 0, false, 0);
        let ms = t0.elapsed().as_millis() as u64;
        let error = sent.is_none().then(|| self.last_error.clone());
        self.ledger.journal(
            "buy_sent",
            json!({"pool": pool, "hash": sent.as_ref().map(|t| format!("{:#x}", t.hash)), "nonce": sent.as_ref().map(|t| t.nonce), "usd": usd, "amount_in": amount_in.to_string(), "min_out": min_out.to_string(), "accepted": sent.is_some(), "broadcast_ms": ms, "order_age_ms": o.created.elapsed().as_millis() as u64, "error": error}),
        );
        if sent.is_none() {
            if let Some(p) = self.ledger.positions.get_mut(&pool) {
                p.status = PosStatus::BuyFailed;
                p.last_error = error;
            }
            self.ledger.totals.buys_failed += 1;
            self.consecutive_buy_fail += 1;
            self.last_buy_fail = Some(Instant::now());
            self.finish_if_done(&pool);
            let _ = self.ledger.save();
            return;
        }
        self.ledger.totals.buys_sent += 1;
        // approvals right behind the buy (next nonces), so a sell needs a single transaction
        if self.approval_valid(&key.token()) {
            if let Some(p) = self.ledger.positions.get_mut(&pool) {
                p.approvals_ready = true;
            }
        } else {
            self.send_approvals(&pool);
        }
        let _ = self.ledger.save();
    }

    fn tx_rec(&self, tx: &SignedTx, purpose: TxPurpose, tokens: U256, eth: U256, tier_bps: u32, all: bool) -> TxRec {
        TxRec { hash: tx.hash, nonce: tx.nonce, purpose, state: TxState::Pending, sent_ms: now_ms(), mined_ms: None, tokens, eth, tier_bps, all, gas_wei: U256::ZERO, expiry: 0 }
    }

    /// Broadcast, resolving ambiguous network failures. NonceResynced = our nonce was stale and has been re-read
    /// from chain; the caller may re-sign once.
    fn send(&mut self, pool: &str, tx: &SignedTx) -> SendResult {
        self.raw.insert(tx.hash, tx.raw.clone());
        match self.sender.broadcast(tx) {
            SendOutcome::Accepted => SendResult::Accepted,
            SendOutcome::NonceTooLow => {
                let _ = self.sender.sync_nonce();
                self.mark_tx(pool, &tx.hash, TxState::Dropped);
                self.last_error = "nonce too low (resynced)".into();
                SendResult::NonceResynced
            }
            SendOutcome::Rejected { error, maybe_sent } => {
                if maybe_sent && self.sender.is_known(&tx.hash) {
                    self.sender.nonce = self.sender.nonce.max(tx.nonce + 1);
                    return SendResult::Accepted;
                }
                if maybe_sent {
                    if let Ok(n) = self.sender.sync_nonce() {
                        if n > tx.nonce && self.sender.is_known(&tx.hash) {
                            return SendResult::Accepted;
                        }
                    }
                }
                self.mark_tx(pool, &tx.hash, TxState::Dropped);
                self.last_error = error.chars().take(160).collect();
                SendResult::Failed
            }
        }
    }

    /// Sign with the next nonce, write the transaction to the ledger (before broadcasting), broadcast it.
    /// A stale nonce is resolved by re-signing once with the chain's nonce. Returns the accepted transaction.
    #[allow(clippy::too_many_arguments)]
    fn sign_record_send(&mut self, pool: &str, to: Address, value: U256, data: &Bytes, gas_limit: u64, purpose: TxPurpose, tokens: U256, eth: U256, tier_bps: u32, all: bool, expiry: u64) -> Option<SignedTx> {
        for attempt in 0..2 {
            let tx = match self.sender.sign_next(to, value, data.clone(), gas_limit) {
                Ok(t) => t,
                Err(e) => {
                    self.last_error = e;
                    return None;
                }
            };
            let mut rec = self.tx_rec(&tx, purpose, tokens, eth, tier_bps, all);
            rec.expiry = expiry;
            match self.ledger.positions.get_mut(pool) {
                Some(p) => p.txs.push(rec),
                None => return None,
            }
            let _ = self.ledger.save();
            match self.send(pool, &tx) {
                SendResult::Accepted => return Some(tx),
                SendResult::NonceResynced if attempt == 0 => {
                    // a slow endpoint can answer "nonce too low" for our own transaction that another endpoint already
                    // got included: never sign a second one in that case
                    if self.sender.is_known(&tx.hash) {
                        self.mark_tx(pool, &tx.hash, TxState::Pending);
                        self.sender.nonce = self.sender.nonce.max(tx.nonce + 1);
                        return Some(tx);
                    }
                    self.ledger.journal("nonce_resynced", json!({"pool": pool, "stale_nonce": tx.nonce, "nonce": self.sender.nonce}));
                }
                _ => return None,
            }
        }
        None
    }

    /// Token already approved for the router with at least a day of validity left.
    fn approval_valid(&self, token: &Address) -> bool {
        self.ledger.approvals.get(token).map_or(false, |exp| *exp > unix_now() + 86_400)
    }

    /// On-chain check that the wallet can still sell `amount` of `token` through Permit2.
    /// `None` when a lookup failed (unknown, so no action is taken on it).
    fn allowances_ok(&self, token: Address, amount: U256) -> Option<bool> {
        use super::abi::{IPermit2, IERC20};
        use alloy_sol_types::SolCall;
        let owner = self.sender.address;
        let erc = IERC20::allowanceCall { owner, spender: chain::PERMIT2 }.abi_encode().into();
        let p2 = IPermit2::allowanceCall { user: owner, token, spender: UNIVERSAL_ROUTER }.abi_encode().into();
        let erc = IERC20::allowanceCall::abi_decode_returns(&self.sender.eth_call(token, &erc, U256::ZERO).ok()?).ok()?;
        let p2 = IPermit2::allowanceCall::abi_decode_returns(&self.sender.eth_call(chain::PERMIT2, &p2, U256::ZERO).ok()?).ok()?;
        Some(erc >= amount && U256::from(p2.amount) >= amount && p2.expiration.to::<u64>() > unix_now() + 60)
    }

    fn mark_tx(&mut self, pool: &str, hash: &B256, state: TxState) {
        if let Some(t) = self.ledger.positions.get_mut(pool).and_then(|p| p.tx_mut(hash)) {
            t.state = state;
        }
    }

    /// Send whichever of the two sell approvals (token -> Permit2, Permit2 -> router) is not already pending or done.
    fn send_approvals(&mut self, pool: &str) {
        let Some(token) = self.ledger.positions.get(pool).map(|p| p.key.token()) else { return };
        let expiry = unix_now() + self.cfg.permit2_expiry_days * 86_400;
        let plan: [(TxPurpose, Address, Result<Bytes, String>, u64); 2] = [
            (TxPurpose::ApproveToken, token, Ok(uniswap::erc20_approve_permit2_calldata()), 0),
            (TxPurpose::ApprovePermit2, chain::PERMIT2, uniswap::permit2_approve_router_calldata(token, expiry), expiry),
        ];
        for (purpose, to, data, exp) in plan {
            if self.ledger.positions.get(pool).map_or(true, |p| p.approval_sent(purpose)) {
                continue;
            }
            let data = match data {
                Ok(d) => d,
                Err(e) => {
                    self.last_error = e;
                    return;
                }
            };
            let sent = self.sign_record_send(pool, to, U256::ZERO, &data, self.cfg.gas_limit_approve, purpose, U256::ZERO, U256::ZERO, 0, false, exp);
            self.ledger.journal("approve_sent", json!({"pool": pool, "purpose": purpose, "hash": sent.as_ref().map(|t| format!("{:#x}", t.hash)), "accepted": sent.is_some()}));
            if sent.is_none() {
                return;
            }
        }
    }

    fn simulate_buy(&self, key: &Key, pool: &str, amount_in: u128, data: &Bytes) -> Result<(U256, U256), String> {
        let from = format!("{:#x}", self.sender.address);
        let value = U256::from(amount_in);
        let tx = json!({"from": from, "to": format!("{UNIVERSAL_ROUTER:#x}"), "value": hx(value), "data": format!("0x{}", hex::encode(data)), "gas": format!("{:#x}", self.cfg.gas_limit_buy)});
        let over = json!({ from.clone(): {"balance": hx(value.saturating_add(U256::from(10u128.pow(18))))} });
        let v = self.sender.reads.call_once("debug_traceCall", json!([tx, "latest", {"tracer": "callTracer", "tracerConfig": {"withLog": true}, "stateOverrides": over}]), Duration::from_secs(8))?;
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            let out = v.get("output").and_then(|o| o.as_str()).unwrap_or("");
            return Err(format!("simulated buy reverted: {e} ({})", revert_name(out)));
        }
        let mut logs = Vec::new();
        collect_logs(&v, &mut logs);
        let pseudo = json!({"status": "0x1", "logs": logs});
        let tokens = fills::tokens_received(&pseudo, key.token(), self.sender.address);
        let eth_in = fills::swap_amounts(&pseudo, pool).map(|(a0, _)| fills::paid(a0)).unwrap_or(value);
        if tokens.is_zero() {
            return Err("simulated buy delivered no tokens".into());
        }
        Ok((tokens, eth_in))
    }

    // ---------------------------------------------------------------- sells

    fn set_intent(&mut self, pool: &str, all: bool, frac: f64, reason: &str, emergency: bool) {
        let pool = pool.to_lowercase();
        let Some(p) = self.ledger.positions.get_mut(&pool) else { return };
        if !p.is_live() {
            return;
        }
        if !all && (p.tp_done || p.intent.as_ref().map_or(false, |i| i.all)) {
            return;
        }
        let now = now_ms();
        match &mut p.intent {
            Some(i) => {
                if all && !i.all {
                    i.all = true;
                    i.reason = reason.to_string();
                }
                i.emergency |= emergency;
                i.next_try_ms = now;
            }
            None => p.intent = Some(SellIntent { all, frac, reason: reason.to_string(), emergency, attempts: 0, since_ms: now, next_try_ms: now }),
        }
        self.ledger.journal("sell_intent", json!({"pool": pool, "all": all, "frac": frac, "reason": reason, "emergency": emergency}));
        let _ = self.ledger.save();
        self.try_sell(&pool);
    }

    fn on_model_closed(&mut self, pool: &str, reason: &str, ret: f64) {
        let pool = pool.to_lowercase();
        let Some(p) = self.ledger.positions.get_mut(&pool) else { return };
        p.model_close_reason = Some(reason.to_string());
        p.model_ret = Some(ret);
        let needs = p.is_live() && (p.status == PosStatus::Buying || !p.tokens_held.is_zero()) && !p.intent.as_ref().map_or(false, |i| i.all);
        if needs {
            let em = is_emergency(reason);
            self.set_intent(&pool, true, 1.0, &format!("model closed: {reason}"), em);
        } else {
            self.finish_if_done(&pool);
            let _ = self.ledger.save();
        }
    }

    fn quote_sell(&self, key: &Key, tokens: U256) -> Option<f64> {
        let amt: u128 = tokens.try_into().ok()?;
        let data = uniswap::quote_calldata(key, false, amt).ok()?;
        let ret = self.sender.eth_call(V4_QUOTER, &data, U256::ZERO).ok()?;
        uniswap::decode_quote(&ret).ok().map(u256_f64)
    }

    fn try_sell(&mut self, pool: &str) {
        let now = now_ms();
        let live = self.live();
        let (key, amount, all, attempts, emergency, reapproved) = {
            let Some(p) = self.ledger.positions.get(pool) else { return };
            let Some(i) = &p.intent else { return };
            if !matches!(p.status, PosStatus::Open | PosStatus::Stuck) || p.pending(TxPurpose::Sell).is_some() || now < i.next_try_ms {
                return;
            }
            if live && !p.approvals_ready {
                return;
            }
            if p.tokens_held.is_zero() {
                return;
            }
            let amount = if i.all {
                p.tokens_held
            } else {
                let part = p.tokens_bought.saturating_mul(U256::from((i.frac * 1_000_000.0).round() as u64)) / U256::from(1_000_000u64);
                part.min(p.tokens_held)
            };
            (p.key, amount, i.all, i.attempts, i.emergency, p.approvals_since_ms != 0)
        };
        if amount.is_zero() {
            return;
        }
        // a sell reverted: if the allowance is gone (revoked, expired, or never set by the token), approve again, once
        if live && attempts >= 1 && !reapproved && self.allowances_ok(key.token(), amount) == Some(false) {
            if let Some(p) = self.ledger.positions.get_mut(pool) {
                p.approvals_ready = false;
                p.approvals_since_ms = now_ms();
            }
            self.ledger.approvals.remove(&key.token());
            self.ledger.journal("reapprove", json!({"pool": pool, "token": format!("{:#x}", key.token())}));
            self.send_approvals(pool);
            let _ = self.ledger.save();
            return;
        }
        let local = if attempts == 0 { self.states.get(pool).and_then(|s| amm::sell_out(s, u256_f64(amount))) } else { None };
        let expected = local.or_else(|| self.quote_sell(&key, amount));
        let tier = if emergency || expected.is_none() { 10_000 } else { self.cfg.sell_tier_bps(attempts) };
        let min_out = expected.map(|e| amm::min_out(e, tier)).unwrap_or(0);
        let deadline = U256::from(unix_now() + self.cfg.deadline_secs);
        let data = match uniswap::sell_calldata(&key, amount, min_out, deadline) {
            Ok(d) => d,
            Err(e) => {
                self.last_error = e;
                return;
            }
        };
        if !live {
            // shadow: value the sale at the current executable quote
            let got = self.quote_sell(&key, amount).unwrap_or(0.0);
            if let Some(p) = self.ledger.positions.get_mut(pool) {
                p.eth_out = p.eth_out.saturating_add(U256::from(got.max(0.0) as u128));
                p.tokens_held = p.tokens_held.saturating_sub(amount);
                if all || p.tokens_held.is_zero() {
                    p.status = PosStatus::Closed;
                    p.closed_ms = Some(now_ms());
                    p.intent = None;
                } else {
                    p.tp_done = true;
                    p.intent = None;
                }
            }
            self.ledger.journal("shadow_sell", json!({"pool": pool, "tokens": amount.to_string(), "eth_out": got, "all": all}));
            self.finish_if_done(pool);
            let _ = self.ledger.save();
            return;
        }
        let sent = self.sign_record_send(pool, UNIVERSAL_ROUTER, U256::ZERO, &data, self.cfg.gas_limit_sell, TxPurpose::Sell, amount, U256::from(min_out), tier, all, 0);
        self.ledger.journal(
            "sell_sent",
            json!({"pool": pool, "hash": sent.as_ref().map(|t| format!("{:#x}", t.hash)), "tokens": amount.to_string(), "min_out": min_out.to_string(), "tier_bps": tier, "all": all, "accepted": sent.is_some()}),
        );
        if sent.is_some() {
            self.ledger.totals.sells_sent += 1;
        } else if let Some(i) = self.ledger.positions.get_mut(pool).and_then(|p| p.intent.as_mut()) {
            i.next_try_ms = now_ms() + 1_000;
        }
        let _ = self.ledger.save();
    }

    // ---------------------------------------------------------------- receipts

    fn any_pending(&self) -> bool {
        self.ledger.positions.values().any(|p| p.has_pending())
    }

    fn poll_receipts(&mut self) {
        let pending: Vec<(String, B256, u64, u64)> =
            self.ledger.positions.values().flat_map(|p| p.txs.iter().filter(|t| t.state == TxState::Pending).map(move |t| (p.pool.clone(), t.hash, t.sent_ms, t.nonce))).collect();
        if pending.is_empty() {
            return;
        }
        let rs = self.sender.receipts(&pending.iter().map(|x| x.1).collect::<Vec<_>>());
        for ((pool, hash, sent_ms, nonce), r) in pending.into_iter().zip(rs) {
            match r {
                Some(rc) => self.on_receipt(&pool, hash, &rc),
                None => self.check_timeout(&pool, hash, sent_ms, nonce),
            }
        }
    }

    fn check_timeout(&mut self, pool: &str, hash: B256, sent_ms: u64, nonce: u64) {
        let age = now_ms().saturating_sub(sent_ms);
        let limit = self.cfg.receipt_timeout_secs * 1000;
        if age < limit {
            return;
        }
        let mined = self.sender.mined_nonce().unwrap_or(0);
        let gone = if mined > nonce { age > 3 * limit } else { !self.sender.is_known(&hash) };
        if !gone {
            if let Some(raw) = self.raw.get(&hash).cloned() {
                if age > 2 * limit {
                    let tx = SignedTx { hash, raw, nonce };
                    let _ = self.sender.broadcast(&tx);
                }
            }
            return;
        }
        self.ledger.journal("tx_dropped", json!({"pool": pool, "hash": format!("{hash:#x}"), "nonce": nonce}));
        let _ = self.sender.sync_nonce();
        let purpose = self.ledger.positions.get(pool).and_then(|p| p.txs.iter().find(|t| t.hash == hash)).map(|t| t.purpose);
        self.mark_tx(pool, &hash, TxState::Dropped);
        match purpose {
            Some(TxPurpose::Buy) => {
                if let Some(p) = self.ledger.positions.get_mut(pool) {
                    p.status = PosStatus::BuyFailed;
                }
                self.ledger.totals.buys_failed += 1;
                self.finish_if_done(pool);
            }
            Some(TxPurpose::ApproveToken) | Some(TxPurpose::ApprovePermit2) => self.send_approvals(pool),
            Some(TxPurpose::Sell) => {
                if let Some(i) = self.ledger.positions.get_mut(pool).and_then(|p| p.intent.as_mut()) {
                    i.next_try_ms = now_ms();
                }
            }
            None => {}
        }
        let _ = self.ledger.save();
    }

    fn on_receipt(&mut self, pool: &str, hash: B256, rc: &Value) {
        let ok = fills::status_ok(rc);
        let gas = fills::gas_cost(rc);
        let wallet = self.sender.address;
        let mut alert: Option<String> = None;
        let mut approved_until: Option<(Address, u64)> = None;
        {
            let Some(p) = self.ledger.positions.get_mut(pool) else { return };
            let token = p.key.token();
            let Some(t) = p.tx_mut(&hash) else { return };
            t.state = if ok { TxState::Ok } else { TxState::Reverted };
            t.gas_wei = gas;
            t.mined_ms = Some(now_ms());
            let (purpose, all, sold_hint) = (t.purpose, t.all, t.tokens);
            p.gas_wei = p.gas_wei.saturating_add(gas);
            match purpose {
                TxPurpose::Buy => {
                    if ok {
                        let tokens = fills::tokens_received(rc, token, wallet);
                        if let Some((a0, _)) = fills::swap_amounts(rc, pool) {
                            p.eth_in = fills::paid(a0);
                        }
                        p.tokens_bought = tokens;
                        p.tokens_held = tokens;
                        p.status = if tokens.is_zero() { PosStatus::Stuck } else { PosStatus::Open };
                        if tokens.is_zero() {
                            p.last_error = Some("buy mined but no tokens arrived".into());
                            alert = Some(format!("buy {pool} mined but delivered no tokens"));
                        }
                    } else {
                        p.status = PosStatus::BuyFailed;
                        p.intent = None;
                    }
                }
                TxPurpose::ApproveToken | TxPurpose::ApprovePermit2 => {
                    if ok {
                        if let Some(expiry) = p.approvals_done() {
                            p.approvals_ready = true;
                            approved_until = (expiry > 0).then_some((token, expiry));
                        }
                    } else {
                        p.last_error = Some("approval reverted: this token cannot be sold through Permit2".into());
                        if p.status == PosStatus::Open {
                            p.status = PosStatus::Stuck;
                        }
                        alert = Some(format!("approval reverted for {pool}: position may be unsellable"));
                    }
                }
                TxPurpose::Sell => {
                    if ok {
                        let (a0, a1) = fills::swap_amounts(rc, pool).unwrap_or_default();
                        let eth = fills::received(a0);
                        // what left the wallet (includes any transfer tax); fall back to the pool side, then the request
                        let sold = [fills::tokens_sent(rc, token, wallet), fills::paid(a1), sold_hint].into_iter().find(|v| !v.is_zero()).unwrap_or_default();
                        p.eth_out = p.eth_out.saturating_add(eth);
                        p.tokens_held = p.tokens_held.saturating_sub(sold.min(p.tokens_held));
                        let dust = p.tokens_bought / U256::from(1_000_000u64);
                        if all || p.tokens_held <= dust {
                            p.status = PosStatus::Closed;
                            p.closed_ms = Some(now_ms());
                            p.intent = None;
                        } else {
                            p.tp_done = true;
                            if !p.intent.as_ref().map_or(false, |i| i.all) {
                                p.intent = None;
                            }
                        }
                    } else if let Some(i) = p.intent.as_mut() {
                        i.attempts += 1;
                        i.next_try_ms = now_ms();
                        if i.attempts >= self.cfg.max_sell_attempts {
                            p.status = PosStatus::Stuck;
                            i.next_try_ms = now_ms() + 30_000;
                            alert = Some(format!("sell of {pool} reverted {} times; still retrying every 30 s", i.attempts));
                        }
                    }
                }
            }
        }
        if let Some((token, expiry)) = approved_until {
            self.ledger.approvals.insert(token, expiry);
        }
        if let Some(a) = alert {
            self.alert_once(&format!("{pool}:{}", a.len()), a);
        }
        if !ok {
            let purpose = self.ledger.positions.get(pool).and_then(|p| p.txs.iter().find(|t| t.hash == hash)).map(|t| t.purpose);
            match purpose {
                Some(TxPurpose::Buy) => {
                    self.ledger.totals.buys_failed += 1;
                    self.consecutive_buy_fail += 1;
                    self.last_buy_fail = Some(Instant::now());
                }
                Some(TxPurpose::Sell) => self.ledger.totals.sells_reverted += 1,
                _ => {}
            }
        } else if self.ledger.positions.get(pool).map_or(false, |p| p.status == PosStatus::Open) {
            self.consecutive_buy_fail = 0;
        }
        self.ledger.journal("receipt", json!({"pool": pool, "hash": format!("{hash:#x}"), "ok": ok, "gas_wei": gas.to_string(), "block": fills::block_number(rc)}));
        // refresh the balance right away so sizing never counts a just-spent (or just-received) ETH twice
        if self.live() {
            if let Ok(b) = self.sender.eth_balance() {
                self.chain_balance = b;
                self.t_bal = Instant::now();
            }
        }
        self.finish_if_done(pool);
        let _ = self.ledger.save();
        self.try_sell(pool);
    }

    /// Archive a position once it is finished, no transaction of it is in flight, and the strategy's own result is
    /// known (we usually sell before the model's 3 s fill, so wait up to 2 minutes for its `Closed` order; positions
    /// the model does not hold, e.g. after a reconcile, are archived after that wait).
    fn finish_if_done(&mut self, pool: &str) {
        let now = now_ms();
        let done = self.ledger.positions.get(pool).map_or(false, |p| {
            let finished = matches!(p.status, PosStatus::Closed | PosStatus::BuyFailed) && !p.has_pending();
            let model_known = p.model_ret.is_some() || p.status == PosStatus::BuyFailed;
            let waited = p.closed_ms.map_or(true, |t| now.saturating_sub(t) > 120_000);
            finished && (model_known || waited)
        });
        if done {
            let eth_usd = if self.eth_usd.is_finite() { self.eth_usd } else { self.ledger.positions[pool].eth_usd_entry };
            self.ledger.archive(pool, eth_usd);
            if self.cfg.daily_loss_limit_usd > 0.0 && self.ledger.today_realized_usd() < -self.cfg.daily_loss_limit_usd {
                let msg = format!("daily loss limit hit (${:.2} today): new buys stopped until UTC midnight", self.ledger.today_realized_usd());
                self.alert_once(&format!("daily:{}", self.ledger.totals.day), msg);
            }
        }
    }

    // ---------------------------------------------------------------- control

    fn reconcile(&mut self, model_open: &[String]) {
        let open: std::collections::HashSet<String> = model_open.iter().map(|s| s.to_lowercase()).collect();
        let orphans: Vec<String> = self.ledger.positions.values().filter(|p| p.is_live() && !p.manual && !open.contains(&p.pool)).map(|p| p.pool.clone()).collect();
        for pool in orphans {
            self.ledger.journal("reconcile_flatten", json!({"pool": pool}));
            self.set_intent(&pool, true, 1.0, "reconcile: strategy no longer holds this position", false);
        }
    }

    fn flatten_all(&mut self, reason: &str) {
        let pools: Vec<String> = self.ledger.positions.values().filter(|p| p.is_live()).map(|p| p.pool.clone()).collect();
        for pool in pools {
            self.set_intent(&pool, true, 1.0, reason, true);
        }
    }

    fn refresh_eth_usd(&mut self) {
        if let Ok(s) = pools::read_state(&self.sender.reads, ETH_USDG_POOL_ID, "latest") {
            let p = s.sqrt_p * s.sqrt_p * 1e12;
            if p > 100.0 && p < 100_000.0 {
                self.eth_usd = p;
            }
        }
    }

    fn check_kill_files(&mut self) {
        let flatten = self.cfg.state_dir.join(FLATTEN_FILE).exists();
        if flatten && !self.flatten_seen {
            (self.alert)("FLATTEN_ALL file found: selling every open position, no new buys".into());
            self.flatten_all("FLATTEN_ALL file");
        }
        self.flatten_seen = flatten;
    }

    fn run_intents(&mut self) {
        let pools: Vec<String> = self.ledger.positions.values().filter(|p| p.intent.is_some()).map(|p| p.pool.clone()).collect();
        for pool in pools {
            self.try_sell(&pool);
        }
        // approvals that failed to send earlier: retried every 2 s, never after one of them reverted
        if !self.live() {
            return;
        }
        let now = now_ms();
        let need: Vec<String> = self.ledger.positions.values().filter(|p| p.approvals_retry_due(now)).map(|p| p.pool.clone()).collect();
        for pool in need {
            self.send_approvals(&pool);
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        // the failed-buy pause releases itself after 10 minutes without a new failure
        if self.consecutive_buy_fail >= 5 && self.last_buy_fail.map_or(true, |t| t.elapsed() > Duration::from_secs(600)) {
            self.consecutive_buy_fail = 0;
            self.ledger.journal("buy_pause_released", json!({}));
        }
        if now - self.t_receipts >= Duration::from_millis(100) {
            self.t_receipts = now;
            self.poll_receipts();
        }
        if now - self.t_kill >= Duration::from_secs(1) {
            self.t_kill = now;
            self.check_kill_files();
            // finished positions still waiting for the strategy's Closed order (archived after at most 2 minutes)
            let finished: Vec<String> = self.ledger.positions.values().filter(|p| matches!(p.status, PosStatus::Closed | PosStatus::BuyFailed)).map(|p| p.pool.clone()).collect();
            if !finished.is_empty() {
                for pool in &finished {
                    self.finish_if_done(pool);
                }
                let _ = self.ledger.save();
            }
        }
        self.run_intents();
        if now - self.t_fee >= Duration::from_secs(3) {
            self.t_fee = now;
            if let Err(e) = self.sender.refresh_base_fee() {
                self.last_error = format!("base fee: {e}");
            }
        }
        if self.live() && now - self.t_bal >= Duration::from_secs(5) {
            self.t_bal = now;
            match self.sender.eth_balance() {
                Ok(b) => self.chain_balance = b,
                Err(e) => self.last_error = format!("balance: {e}"),
            }
        }
        if now - self.t_px >= Duration::from_secs(30) {
            self.t_px = now;
            self.refresh_eth_usd();
        }
        if now - self.t_status >= Duration::from_secs(1) {
            self.t_status = now;
            self.write_status();
        }
    }

    fn write_status(&mut self) {
        let eth_usd = self.eth_usd;
        let mut open_value_eth = 0.0;
        let positions: Vec<Value> = self
            .ledger
            .positions
            .values()
            .map(|p| {
                let held = u256_f64(p.tokens_held);
                let value_eth = self.states.get(&p.pool).and_then(|s| amm::sell_out(s, held)).map(|w| w / 1e18).unwrap_or(0.0);
                if p.is_live() {
                    open_value_eth += value_eth;
                }
                json!({
                    "pool": p.pool, "token": format!("{:#x}", p.key.token()), "status": p.status, "shadow": p.shadow, "manual": p.manual,
                    "opened_ms": p.opened_ms, "size_usd": p.size_usd, "model_size_usd": p.model_size_usd,
                    "eth_in": u256_f64(p.eth_in) / 1e18, "eth_out": u256_f64(p.eth_out) / 1e18, "gas_eth": u256_f64(p.gas_wei) / 1e18,
                    "tokens_held": p.tokens_held.to_string(), "value_eth_est": value_eth,
                    "unrealized_usd": (value_eth + u256_f64(p.eth_out) / 1e18 - u256_f64(p.eth_in) / 1e18 - u256_f64(p.gas_wei) / 1e18) * eth_usd,
                    "tp_done": p.tp_done, "approvals_ready": p.approvals_ready, "intent": p.intent, "last_error": p.last_error,
                    "txs": p.txs.iter().map(|t| json!({"purpose": t.purpose, "hash": format!("{:#x}", t.hash), "state": t.state, "nonce": t.nonce})).collect::<Vec<_>>(),
                })
            })
            .collect();
        let bal_eth = u256_f64(self.chain_balance) / 1e18;
        let block = self.buy_block_reason();
        let j = json!({
            "updated_ms": now_ms(), "mode": self.cfg.mode.as_str(), "address": format!("{:#x}", self.sender.address),
            "eth_usd": eth_usd, "eth_balance": bal_eth, "equity_usd_est": (bal_eth + open_value_eth) * eth_usd,
            "base_fee_gwei": self.sender.base_fee_gwei(), "nonce": self.sender.nonce, "uptime_s": self.started.elapsed().as_secs(),
            "buying": block.is_none(), "buy_block_reason": block, "positions": positions,
            "totals": self.ledger.totals, "today_realized_usd": self.ledger.today_realized_usd(),
            "recent_closed": self.ledger.recent_closed.iter().rev().take(60).cloned().collect::<Vec<_>>(),
            "counters": self.counters, "last_error": self.last_error,
            "limits": {"max_order_usd": self.cfg.max_order_usd, "size_scale": self.cfg.size_scale, "daily_loss_limit_usd": self.cfg.daily_loss_limit_usd,
                       "max_open_positions": self.cfg.max_open_positions, "buy_slippage_bps": self.cfg.buy_slippage_bps, "sell_slippage_bps": self.cfg.sell_slippage_bps},
        });
        let tmp = self.cfg.state_dir.join("exec_status.json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&j).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.cfg.state_dir.join("exec_status.json"));
        }
    }
}

/// Flatten callTracer logs (nested calls) into receipt-shaped logs.
fn collect_logs(n: &Value, out: &mut Vec<Value>) {
    if let Some(ls) = n.get("logs").and_then(|l| l.as_array()) {
        for l in ls {
            out.push(json!({"address": l["address"], "topics": l["topics"], "data": l["data"]}));
        }
    }
    if let Some(cs) = n.get("calls").and_then(|c| c.as_array()) {
        for c in cs {
            collect_logs(c, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_reasons_match_engine_strings() {
        for r in ["rug terdeteksi", "LP ditarik >30%", "dump >=5% LP", "dump pelari", "stop bencana", "FLATTEN_ALL file"] {
            assert!(is_emergency(r), "{r}");
        }
        for r in ["model bahaya", "tanda pucuk", "trailing 40%", "stop -40%", "akhir data", "take profit"] {
            assert!(!is_emergency(r), "{r}");
        }
    }

    #[test]
    fn collects_nested_trace_logs() {
        let v = json!({"logs": [{"address": "0x1", "topics": ["0xa"], "data": "0x"}], "calls": [{"calls": [{"logs": [{"address": "0x2", "topics": ["0xb"], "data": "0x01"}]}]}]});
        let mut out = Vec::new();
        collect_logs(&v, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1]["address"], "0x2");
    }
}
