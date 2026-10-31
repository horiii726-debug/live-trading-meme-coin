//! Execution settings. Keys start with `EXEC_`; they are read from the same `.env` file as the strategy and the
//! process environment overrides the file. Nothing here changes a strategy decision: these only control how (and
//! whether) the strategy's orders are executed on chain.
use std::collections::HashMap;
use std::path::PathBuf;

/// Exact text that must be in EXEC_LIVE_CONFIRM before real transactions are broadcast.
pub const LIVE_CONFIRM_PHRASE: &str = "I_ACCEPT_REAL_MONEY_RISK";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Paper trading only (default). The executor is not started.
    Off,
    /// Build, sign and simulate every order against live chain state; never broadcast.
    Shadow,
    /// Broadcast real transactions.
    Live,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Shadow => "shadow",
            Mode::Live => "live",
        }
    }
}

/// How the strategy's order size is turned into a real order size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sizing {
    /// Same quarter-Kelly fraction, applied to the REAL wallet equity (compounds on real money):
    /// real = model_size x real_equity / model_equity. Default.
    RealEquity,
    /// Use the model's dollar size as is.
    Model,
}

#[derive(Clone, Debug)]
pub struct ExecConfig {
    pub mode: Mode,
    pub sizing: Sizing,
    /// Bug guard: one buy never exceeds this fraction of real equity (the strategy's Kelly cap is 0.15).
    pub max_position_frac: f64,
    /// File holding the hex private key (chmod 600). Required for live; optional for shadow.
    pub key_file: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub rpc_urls: Vec<String>,
    pub doh: bool,
    /// Extra multiplier on the real order size (1.0 = exactly the strategy's sizing).
    pub size_scale: f64,
    /// Optional hard cap per buy in USD (0 = off, so compounding is not cut).
    pub max_order_usd: f64,
    pub min_order_usd: f64,
    /// ETH always kept for gas (never spent on buys).
    pub gas_reserve_eth: f64,
    /// Stop new buys for the rest of the UTC day once realized losses exceed this (0 = off).
    pub daily_loss_limit_usd: f64,
    pub max_open_positions: usize,
    pub buy_slippage_bps: u32,
    /// Sell tolerance per attempt; the last tier is reused for further attempts. 10000 = accept any output.
    pub sell_slippage_bps: Vec<u32>,
    /// Drop a buy that waited this many ms in the executor's queue. This measures queue time only: how old the block
    /// behind the order may be is the strategy's own limit (FRESH_BLOCKS).
    pub max_buy_delay_ms: u64,
    /// Refuse new buys while the base fee is above this.
    pub max_base_fee_gwei: f64,
    pub gas_limit_buy: u64,
    pub gas_limit_sell: u64,
    pub gas_limit_approve: u64,
    pub deadline_secs: u64,
    pub permit2_expiry_days: u64,
    pub receipt_timeout_secs: u64,
    pub max_sell_attempts: u32,
}

fn read_env_file(path: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(t) = std::fs::read_to_string(path) {
        for l in t.lines() {
            let l = l.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = l.split_once('=') {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    m
}

impl ExecConfig {
    pub fn load(env_file: Option<&str>, bot_dir: &str, rpc_urls: &str, doh: bool, max_pos: usize) -> Result<ExecConfig, String> {
        let file = env_file.map(read_env_file).unwrap_or_default();
        let get = |k: &str| -> Option<String> { std::env::var(k).ok().filter(|s| !s.is_empty()).or_else(|| file.get(k).cloned()).filter(|s| !s.is_empty()) };
        let num = |k: &str, d: f64| -> Result<f64, String> {
            match get(k) {
                None => Ok(d),
                Some(s) => s.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("{k}: not a number: {s}")),
            }
        };
        let int = |k: &str, d: u64| -> Result<u64, String> {
            match get(k) {
                None => Ok(d),
                Some(s) => s.parse::<u64>().map_err(|_| format!("{k}: not a non-negative integer: {s}")),
            }
        };
        let mode = match get("EXEC_MODE").as_deref().map(|s| s.to_ascii_lowercase()) {
            None => Mode::Off,
            Some(s) if s == "off" => Mode::Off,
            Some(s) if s == "shadow" => Mode::Shadow,
            Some(s) if s == "live" => Mode::Live,
            Some(s) => return Err(format!("EXEC_MODE must be off, shadow or live (got {s})")),
        };
        let sell_slippage_bps = match get("EXEC_SELL_SLIPPAGE_BPS") {
            None => vec![1500, 5000, 10_000],
            Some(s) => s.split(',').map(|x| x.trim().parse::<u32>().map_err(|_| format!("EXEC_SELL_SLIPPAGE_BPS: bad value {x}"))).collect::<Result<Vec<_>, _>>()?,
        };
        let sizing = match get("EXEC_SIZING").as_deref().map(|s| s.to_ascii_lowercase()) {
            None => Sizing::RealEquity,
            Some(s) if s == "real_equity" => Sizing::RealEquity,
            Some(s) if s == "model" => Sizing::Model,
            Some(s) => return Err(format!("EXEC_SIZING must be real_equity or model (got {s})")),
        };
        let c = ExecConfig {
            mode,
            sizing,
            max_position_frac: num("EXEC_MAX_POSITION_FRAC", 0.20)?,
            key_file: get("EXEC_KEY_FILE").map(PathBuf::from),
            state_dir: get("EXEC_STATE_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(bot_dir).join("rust_live").join("exec")),
            // EXEC_RPC_URLS lets execution use its own endpoints (e.g. the fastest one first); default = RPC_URLS
            rpc_urls: get("EXEC_RPC_URLS").unwrap_or_else(|| rpc_urls.to_string()).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            doh,
            size_scale: num("EXEC_SIZE_SCALE", 1.0)?,
            max_order_usd: num("EXEC_MAX_ORDER_USD", 0.0)?,
            min_order_usd: num("EXEC_MIN_ORDER_USD", 0.25)?,
            gas_reserve_eth: num("EXEC_GAS_RESERVE_ETH", 0.002)?,
            daily_loss_limit_usd: num("EXEC_DAILY_LOSS_LIMIT_USD", 25.0)?,
            max_open_positions: int("EXEC_MAX_OPEN_POSITIONS", max_pos as u64)? as usize,
            buy_slippage_bps: int("EXEC_BUY_SLIPPAGE_BPS", 1000)? as u32,
            sell_slippage_bps,
            max_buy_delay_ms: int("EXEC_MAX_BUY_DELAY_MS", 1500)?,
            max_base_fee_gwei: num("EXEC_MAX_BASE_FEE_GWEI", 1.0)?,
            gas_limit_buy: int("EXEC_GAS_LIMIT_BUY", 600_000)?,
            gas_limit_sell: int("EXEC_GAS_LIMIT_SELL", 600_000)?,
            gas_limit_approve: int("EXEC_GAS_LIMIT_APPROVE", 150_000)?,
            deadline_secs: int("EXEC_DEADLINE_SECS", 60)?,
            permit2_expiry_days: int("EXEC_PERMIT2_EXPIRY_DAYS", 30)?,
            receipt_timeout_secs: int("EXEC_RECEIPT_TIMEOUT_SECS", 20)?,
            max_sell_attempts: int("EXEC_MAX_SELL_ATTEMPTS", 8)? as u32,
        };
        c.validate(get("EXEC_LIVE_CONFIRM").as_deref())?;
        Ok(c)
    }

    fn validate(&self, live_confirm: Option<&str>) -> Result<(), String> {
        if self.mode == Mode::Off {
            return Ok(());
        }
        if self.rpc_urls.is_empty() {
            return Err("RPC_URLS is empty".into());
        }
        if self.mode == Mode::Live {
            if self.key_file.is_none() {
                return Err("EXEC_MODE=live needs EXEC_KEY_FILE (path to your private key file)".into());
            }
            if live_confirm != Some(LIVE_CONFIRM_PHRASE) {
                return Err(format!("EXEC_MODE=live needs EXEC_LIVE_CONFIRM={LIVE_CONFIRM_PHRASE}"));
            }
        }
        let pos = |name: &str, v: f64| if v > 0.0 { Ok(()) } else { Err(format!("{name} must be > 0")) };
        pos("EXEC_SIZE_SCALE", self.size_scale)?;
        pos("EXEC_MAX_BASE_FEE_GWEI", self.max_base_fee_gwei)?;
        if !(self.max_position_frac > 0.0 && self.max_position_frac <= 1.0) {
            return Err("EXEC_MAX_POSITION_FRAC must be in (0, 1]".into());
        }
        if self.max_order_usd < 0.0 {
            return Err("EXEC_MAX_ORDER_USD must be >= 0 (0 = no cap)".into());
        }
        if self.min_order_usd < 0.0 || (self.max_order_usd > 0.0 && self.min_order_usd > self.max_order_usd) {
            return Err("EXEC_MIN_ORDER_USD must be between 0 and EXEC_MAX_ORDER_USD".into());
        }
        if self.gas_reserve_eth < 0.0 || self.daily_loss_limit_usd < 0.0 {
            return Err("EXEC_GAS_RESERVE_ETH and EXEC_DAILY_LOSS_LIMIT_USD must be >= 0".into());
        }
        if self.buy_slippage_bps == 0 || self.buy_slippage_bps >= 10_000 {
            return Err("EXEC_BUY_SLIPPAGE_BPS must be between 1 and 9999".into());
        }
        if self.sell_slippage_bps.is_empty() || self.sell_slippage_bps.iter().any(|b| *b == 0 || *b > 10_000) {
            return Err("EXEC_SELL_SLIPPAGE_BPS must be a comma list of values in 1..=10000".into());
        }
        if self.max_open_positions == 0 || self.max_sell_attempts == 0 || self.deadline_secs == 0 || self.receipt_timeout_secs == 0 {
            return Err("EXEC_MAX_OPEN_POSITIONS, EXEC_MAX_SELL_ATTEMPTS, EXEC_DEADLINE_SECS and EXEC_RECEIPT_TIMEOUT_SECS must be > 0".into());
        }
        for (name, g) in [("EXEC_GAS_LIMIT_BUY", self.gas_limit_buy), ("EXEC_GAS_LIMIT_SELL", self.gas_limit_sell), ("EXEC_GAS_LIMIT_APPROVE", self.gas_limit_approve)] {
            if !(50_000..=5_000_000).contains(&g) {
                return Err(format!("{name} must be between 50000 and 5000000"));
            }
        }
        Ok(())
    }

    /// Slippage tolerance for a sell attempt (0-based); the last tier repeats.
    pub fn sell_tier_bps(&self, attempt: u32) -> u32 {
        let i = (attempt as usize).min(self.sell_slippage_bps.len() - 1);
        self.sell_slippage_bps[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each call gets its own file: tests run in parallel.
    fn with_env(pairs: &[(&str, &str)]) -> Result<ExecConfig, String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("rhc_cfg_test_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("test.env");
        let body: String = pairs.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        std::fs::write(&f, body).unwrap();
        ExecConfig::load(Some(f.to_str().unwrap()), "/tmp/bot", "https://a.example,https://b.example", false, 10)
    }

    #[test]
    fn default_is_off() {
        let c = with_env(&[]).unwrap();
        assert_eq!(c.mode, Mode::Off);
        assert_eq!(c.rpc_urls.len(), 2);
        assert_eq!(c.sizing, Sizing::RealEquity);
        assert_eq!(c.max_order_usd, 0.0);
    }

    #[test]
    fn sizing_modes() {
        assert_eq!(with_env(&[("EXEC_SIZING", "model")]).unwrap().sizing, Sizing::Model);
        assert!(with_env(&[("EXEC_SIZING", "kelly")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "shadow"), ("EXEC_MAX_POSITION_FRAC", "1.5")]).is_err());
    }

    #[test]
    fn live_requires_key_and_confirmation() {
        assert!(with_env(&[("EXEC_MODE", "live")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "live"), ("EXEC_KEY_FILE", "/k")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "live"), ("EXEC_KEY_FILE", "/k"), ("EXEC_LIVE_CONFIRM", "yes")]).is_err());
        let c = with_env(&[("EXEC_MODE", "live"), ("EXEC_KEY_FILE", "/k"), ("EXEC_LIVE_CONFIRM", LIVE_CONFIRM_PHRASE)]).unwrap();
        assert_eq!(c.mode, Mode::Live);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(with_env(&[("EXEC_MODE", "paper")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "shadow"), ("EXEC_BUY_SLIPPAGE_BPS", "0")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "shadow"), ("EXEC_SELL_SLIPPAGE_BPS", "1500,abc")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "shadow"), ("EXEC_MAX_ORDER_USD", "-1")]).is_err());
        assert!(with_env(&[("EXEC_MODE", "shadow"), ("EXEC_GAS_LIMIT_BUY", "10")]).is_err());
    }

    #[test]
    fn sell_tiers_repeat_last() {
        let c = with_env(&[("EXEC_MODE", "shadow"), ("EXEC_SELL_SLIPPAGE_BPS", "1500,5000")]).unwrap();
        assert_eq!((c.sell_tier_bps(0), c.sell_tier_bps(1), c.sell_tier_bps(7)), (1500, 5000, 5000));
    }
}
