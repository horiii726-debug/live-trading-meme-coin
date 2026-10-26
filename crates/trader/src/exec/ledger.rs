//! Real-position book, persisted so a restart never loses track of money on chain.
//!
//! Files in the state directory:
//! - `positions.json`  open (not yet closed) positions + approved tokens + running totals, rewritten atomically
//!   (temp file + rename) after every change and BEFORE each broadcast, so a crash cannot hide a sent transaction;
//! - `closed.jsonl`    one line per closed position (append-only history);
//! - `journal.jsonl`   one line per event (orders, transactions, receipts, skips), append-only audit trail.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::uniswap::Key;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PosStatus {
    /// buy sent, receipt pending
    Buying,
    /// holding tokens
    Open,
    Closed,
    BuyFailed,
    /// cannot be sold right now (approval failed or repeated sell reverts); still retried, owner alerted
    Stuck,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TxPurpose {
    Buy,
    ApproveToken,
    ApprovePermit2,
    Sell,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TxState {
    Pending,
    Ok,
    Reverted,
    Dropped,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TxRec {
    pub hash: B256,
    pub nonce: u64,
    pub purpose: TxPurpose,
    pub state: TxState,
    pub sent_ms: u64,
    pub mined_ms: Option<u64>,
    /// token amount (sell: tokens in; buy: minimum tokens out)
    pub tokens: U256,
    /// ETH amount (buy: wei in; sell: minimum wei out)
    pub eth: U256,
    pub tier_bps: u32,
    /// sell of the whole remaining position (false = take-profit fraction)
    pub all: bool,
    pub gas_wei: U256,
    /// Permit2 approval expiry (unix seconds) for ApprovePermit2 transactions
    #[serde(default)]
    pub expiry: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SellIntent {
    /// sell everything (otherwise `frac` of the tokens bought)
    pub all: bool,
    pub frac: f64,
    pub reason: String,
    pub emergency: bool,
    pub attempts: u32,
    pub since_ms: u64,
    pub next_try_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Position {
    pub pool: String,
    pub key: Key,
    pub shadow: bool,
    pub manual: bool,
    pub status: PosStatus,
    pub opened_ms: u64,
    pub closed_ms: Option<u64>,
    pub model_size_usd: f64,
    pub size_usd: f64,
    pub eth_usd_entry: f64,
    pub eth_in: U256,
    pub tokens_bought: U256,
    pub tokens_held: U256,
    pub eth_out: U256,
    pub gas_wei: U256,
    pub approvals_ready: bool,
    /// approvals sent before this time no longer count (set once, when a sell revert finds the allowance missing)
    #[serde(default)]
    pub approvals_since_ms: u64,
    pub tp_done: bool,
    pub txs: Vec<TxRec>,
    pub intent: Option<SellIntent>,
    pub model_close_reason: Option<String>,
    pub model_ret: Option<f64>,
    pub last_error: Option<String>,
    pub realized_usd: Option<f64>,
}

impl Position {
    pub fn pending(&self, purpose: TxPurpose) -> Option<&TxRec> {
        self.txs.iter().find(|t| t.purpose == purpose && t.state == TxState::Pending)
    }
    pub fn has_pending(&self) -> bool {
        self.txs.iter().any(|t| t.state == TxState::Pending)
    }
    pub fn tx_mut(&mut self, hash: &B256) -> Option<&mut TxRec> {
        self.txs.iter_mut().find(|t| &t.hash == hash)
    }
    pub fn is_live(&self) -> bool {
        matches!(self.status, PosStatus::Buying | PosStatus::Open | PosStatus::Stuck)
    }
    /// Approval transactions that still count (sent at or after `approvals_since_ms`).
    fn current_approvals(&self) -> impl Iterator<Item = &TxRec> {
        self.txs.iter().filter(move |t| matches!(t.purpose, TxPurpose::ApproveToken | TxPurpose::ApprovePermit2) && t.sent_ms >= self.approvals_since_ms)
    }
    /// An approval of this purpose is pending or mined OK, so it must not be sent again.
    pub fn approval_sent(&self, purpose: TxPurpose) -> bool {
        self.current_approvals().any(|t| t.purpose == purpose && matches!(t.state, TxState::Pending | TxState::Ok))
    }
    /// Both approvals mined OK: returns the Permit2 expiry (0 when not recorded).
    pub fn approvals_done(&self) -> Option<u64> {
        let done = |w: TxPurpose| self.current_approvals().filter(|t| t.purpose == w && t.state == TxState::Ok).map(|t| t.expiry).max();
        done(TxPurpose::ApproveToken)?;
        done(TxPurpose::ApprovePermit2)
    }
    /// Approvals are missing and should be (re)sent: nothing in flight, none reverted, last attempt over 2 s ago.
    pub fn approvals_retry_due(&self, now_ms: u64) -> bool {
        matches!(self.status, PosStatus::Open | PosStatus::Stuck)
            && !self.approvals_ready
            && !self.has_pending()
            && !self.current_approvals().any(|t| t.state == TxState::Reverted)
            && self.current_approvals().map(|t| t.sent_ms).max().map_or(true, |t| now_ms.saturating_sub(t) > 2_000)
    }
    /// (ETH out - gas) / ETH in, after the position is closed.
    pub fn real_ret(&self) -> Option<f64> {
        let inn = u256_f64(self.eth_in);
        (inn > 0.0).then(|| (u256_f64(self.eth_out) - u256_f64(self.gas_wei)) / inn)
    }
}

pub fn u256_f64(v: U256) -> f64 {
    v.to_string().parse::<f64>().unwrap_or(f64::NAN)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Totals {
    pub closed: u64,
    pub wins: u64,
    pub realized_usd: f64,
    pub gas_usd: f64,
    pub day: i64,
    pub day_realized_usd: f64,
    pub buys_sent: u64,
    pub buys_failed: u64,
    pub sells_sent: u64,
    pub sells_reverted: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Snapshot {
    positions: BTreeMap<String, Position>,
    /// token -> Permit2 approval expiry (unix seconds) for tokens already approved for the router
    #[serde(default)]
    approvals: BTreeMap<Address, u64>,
    totals: Totals,
}

pub struct Ledger {
    dir: PathBuf,
    journal: File,
    closed_log: File,
    pub positions: BTreeMap<String, Position>,
    pub approvals: BTreeMap<Address, u64>,
    pub totals: Totals,
    pub recent_closed: Vec<Value>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn open_append(p: &Path) -> Result<File, String> {
    OpenOptions::new().create(true).append(true).open(p).map_err(|e| format!("{}: {e}", p.display()))
}

impl Ledger {
    pub fn open(dir: &Path) -> Result<Ledger, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let snap: Snapshot = match std::fs::read_to_string(dir.join("positions.json")) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("positions.json is corrupt ({e}); refusing to start"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Snapshot::default(),
            Err(e) => return Err(format!("positions.json: {e}")),
        };
        let recent_closed = std::fs::read_to_string(dir.join("closed.jsonl"))
            .map(|t| t.lines().rev().take(200).filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<Value>>().into_iter().rev().collect())
            .unwrap_or_default();
        Ok(Ledger {
            journal: open_append(&dir.join("journal.jsonl"))?,
            closed_log: open_append(&dir.join("closed.jsonl"))?,
            dir: dir.to_path_buf(),
            positions: snap.positions,
            approvals: snap.approvals,
            totals: snap.totals,
            recent_closed,
        })
    }

    /// Atomic, durable rewrite of positions.json (fsync before the rename, so a power loss right after a broadcast
    /// cannot leave the sent transaction out of the book).
    pub fn save(&self) -> Result<(), String> {
        // a book that cannot be written could forget a sent transaction: internal error -> kill switch
        self.save_inner().map_err(|e| {
            crate::kill::picu(&format!("ledger eksekusi gagal disimpan: {e}"));
            e
        })
    }
    fn save_inner(&self) -> Result<(), String> {
        let snap = Snapshot { positions: self.positions.clone(), approvals: self.approvals.clone(), totals: self.totals.clone() };
        let body = serde_json::to_vec_pretty(&snap).map_err(|e| e.to_string())?;
        let tmp = self.dir.join("positions.json.tmp");
        let mut f = File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(&body).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, self.dir.join("positions.json")).map_err(|e| format!("positions.json: {e}"))?;
        if let Ok(d) = File::open(&self.dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }

    pub fn journal(&mut self, event: &str, data: Value) {
        let line = json!({"ms": now_ms(), "event": event, "data": data}).to_string();
        let _ = writeln!(self.journal, "{line}");
    }

    /// Move a finished position to closed.jsonl and update totals.
    pub fn archive(&mut self, pool: &str, eth_usd: f64) {
        let Some(mut p) = self.positions.remove(pool) else { return };
        p.closed_ms.get_or_insert(now_ms());
        let gas_usd = u256_f64(p.gas_wei) / 1e18 * eth_usd;
        let pnl_usd = (u256_f64(p.eth_out) - u256_f64(p.eth_in) - u256_f64(p.gas_wei)) / 1e18 * eth_usd;
        p.realized_usd = Some(pnl_usd);
        if !p.shadow {
            let day = (p.closed_ms.unwrap_or(0) / 86_400_000) as i64;
            if day != self.totals.day {
                self.totals.day = day;
                self.totals.day_realized_usd = 0.0;
            }
            self.totals.closed += 1;
            self.totals.wins += (pnl_usd > 0.0) as u64;
            self.totals.realized_usd += pnl_usd;
            self.totals.day_realized_usd += pnl_usd;
            self.totals.gas_usd += gas_usd;
        }
        let rec = json!({
            "pool": p.pool, "token": format!("{:#x}", p.key.token()), "status": p.status, "shadow": p.shadow, "manual": p.manual,
            "opened_ms": p.opened_ms, "closed_ms": p.closed_ms, "size_usd": p.size_usd, "model_size_usd": p.model_size_usd,
            "eth_in": p.eth_in.to_string(), "eth_out": p.eth_out.to_string(), "gas_wei": p.gas_wei.to_string(),
            "real_ret": p.real_ret(), "model_ret": p.model_ret, "pnl_usd": pnl_usd, "gas_usd": gas_usd,
            "reason": p.model_close_reason.clone().or_else(|| p.intent.as_ref().map(|i| i.reason.clone())),
            "txs": p.txs.iter().map(|t| json!({"purpose": t.purpose, "hash": format!("{:#x}", t.hash), "state": t.state})).collect::<Vec<_>>(),
        });
        let _ = writeln!(self.closed_log, "{rec}");
        self.recent_closed.push(rec);
        if self.recent_closed.len() > 200 {
            self.recent_closed.remove(0);
        }
    }

    /// Realized PnL of the current UTC day (0 after midnight).
    pub fn today_realized_usd(&self) -> f64 {
        if self.totals.day == (now_ms() / 86_400_000) as i64 {
            self.totals.day_realized_usd
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::chain::NATIVE;

    fn pos(pool: &str) -> Position {
        Position {
            pool: pool.into(),
            key: Key { currency0: NATIVE, currency1: Address::repeat_byte(7), fee: 10_000, tick_spacing: 200, hooks: NATIVE },
            shadow: false,
            manual: false,
            status: PosStatus::Open,
            opened_ms: 1,
            closed_ms: None,
            model_size_usd: 2.0,
            size_usd: 2.0,
            eth_usd_entry: 2400.0,
            eth_in: U256::from(1_000_000_000_000_000u64),
            tokens_bought: U256::from(5u64),
            tokens_held: U256::ZERO,
            eth_out: U256::from(1_500_000_000_000_000u64),
            gas_wei: U256::from(10_000_000_000_000u64),
            approvals_ready: true,
            approvals_since_ms: 0,
            tp_done: true,
            txs: vec![],
            intent: None,
            model_close_reason: Some("model bahaya".into()),
            model_ret: Some(1.5),
            last_error: None,
            realized_usd: None,
        }
    }

    fn rec(purpose: TxPurpose, state: TxState, sent_ms: u64, expiry: u64) -> TxRec {
        TxRec { hash: B256::ZERO, nonce: 0, purpose, state, sent_ms, mined_ms: None, tokens: U256::ZERO, eth: U256::ZERO, tier_bps: 0, all: false, gas_wei: U256::ZERO, expiry }
    }

    #[test]
    fn approval_lifecycle() {
        let mut p = pos("0xbb");
        p.approvals_ready = false;
        assert_eq!(p.approvals_done(), None);
        assert!(p.approvals_retry_due(10_000), "nothing sent yet");
        p.txs.push(rec(TxPurpose::ApproveToken, TxState::Ok, 1_000, 0));
        assert_eq!(p.approvals_done(), None, "one of two");
        assert!(p.approval_sent(TxPurpose::ApproveToken));
        assert!(!p.approval_sent(TxPurpose::ApprovePermit2));
        p.txs.push(rec(TxPurpose::ApprovePermit2, TxState::Pending, 1_001, 1_900_000_000));
        assert!(!p.approvals_retry_due(10_000), "one in flight");
        p.txs[1].state = TxState::Ok;
        assert_eq!(p.approvals_done(), Some(1_900_000_000));
        // a sell revert found the allowance gone: the older approvals stop counting
        p.approvals_since_ms = 5_000;
        assert_eq!(p.approvals_done(), None);
        assert!(!p.approval_sent(TxPurpose::ApproveToken));
        assert!(p.approvals_retry_due(5_000));
        p.txs.push(rec(TxPurpose::ApproveToken, TxState::Dropped, 5_000, 0));
        assert!(!p.approvals_retry_due(6_000), "2 s backoff after a failed attempt");
        assert!(p.approvals_retry_due(7_001));
        p.txs.push(rec(TxPurpose::ApproveToken, TxState::Reverted, 8_000, 0));
        assert!(!p.approvals_retry_due(60_000), "never retried after a revert");
    }

    #[test]
    fn loads_snapshot_written_before_approval_expiry_tracking() {
        let dir = std::env::temp_dir().join(format!("rhc_ledger_old_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = pos("0xcc");
        p.txs.push(rec(TxPurpose::ApprovePermit2, TxState::Ok, 1, 1_900_000_000));
        let snap = Snapshot { positions: [("0xcc".to_string(), p)].into(), approvals: BTreeMap::new(), totals: Totals::default() };
        // the old layout: `approved` token set, no `approvals`, no `approvals_since_ms`, no tx `expiry`
        let mut v = serde_json::to_value(&snap).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("approvals");
        o.insert("approved".into(), json!([format!("{:#x}", Address::repeat_byte(7))]));
        let pj = o["positions"]["0xcc"].as_object_mut().unwrap();
        pj.remove("approvals_since_ms");
        pj["txs"][0].as_object_mut().unwrap().remove("expiry");
        std::fs::write(dir.join("positions.json"), v.to_string()).unwrap();
        let l = Ledger::open(&dir).unwrap();
        let p = &l.positions["0xcc"];
        assert_eq!(p.approvals_since_ms, 0);
        assert_eq!(p.txs[0].expiry, 0);
        assert!(l.approvals.is_empty(), "unknown expiry: approve again rather than trust it");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn persists_and_archives() {
        let dir = std::env::temp_dir().join(format!("rhc_ledger_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut l = Ledger::open(&dir).unwrap();
            l.positions.insert("0xaa".into(), pos("0xaa"));
            l.approvals.insert(Address::repeat_byte(7), 1_900_000_000);
            l.save().unwrap();
        }
        let mut l = Ledger::open(&dir).unwrap();
        assert_eq!(l.positions.len(), 1);
        assert_eq!(l.approvals.get(&Address::repeat_byte(7)), Some(&1_900_000_000));
        assert!((l.positions["0xaa"].real_ret().unwrap() - 1.49).abs() < 1e-9);
        l.archive("0xaa", 2000.0);
        assert!(l.positions.is_empty());
        assert_eq!(l.totals.closed, 1);
        assert_eq!(l.totals.wins, 1);
        // (0.0015 - 0.001 - 0.00001) ETH * 2000 = 0.98 USD
        assert!((l.totals.realized_usd - 0.98).abs() < 1e-9);
        l.save().unwrap();
        let l2 = Ledger::open(&dir).unwrap();
        assert_eq!(l2.recent_closed.len(), 1);
        assert_eq!(l2.totals.closed, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_snapshot_refuses_to_start() {
        let dir = std::env::temp_dir().join(format!("rhc_ledger_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("positions.json"), "{not json").unwrap();
        assert!(Ledger::open(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
