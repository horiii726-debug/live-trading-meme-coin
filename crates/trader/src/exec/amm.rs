//! Local Uniswap v4 quote from the pool state the engine already holds (no RPC round trip on the hot path).
//!
//! These are the engine's own fill formulas (port_engine `enter_at` / `sell_val`), valid while a swap stays inside
//! the active tick range. At a pinned block they match the on-chain V4Quoter to float precision (tests below), and
//! our order sizes are capped at 1% of pool liquidity, so they stay inside the range in practice. Results feed
//! `amountOutMinimum` together with a slippage tolerance; they are never used as a fill price.
use alloy_primitives::U256;

/// Last pool state seen by the engine. ETH is always currency0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoolState {
    /// sqrtPriceX96 / 2^96 (raw units, token1 per token0)
    pub sqrt_p: f64,
    /// active liquidity
    pub liquidity: f64,
    /// LP fee in pips (1e-6)
    pub fee_pips: f64,
    /// block of the swap this state comes from
    pub blk: f64,
}

impl PoolState {
    fn ok(&self) -> bool {
        self.sqrt_p.is_finite() && self.sqrt_p > 0.0 && self.liquidity.is_finite() && self.liquidity > 0.0 && (0.0..1e6).contains(&self.fee_pips)
    }
}

/// Tokens (raw units) received for `eth_in` wei.
pub fn buy_out(s: &PoolState, eth_in: f64) -> Option<f64> {
    if !s.ok() || !(eth_in > 0.0) {
        return None;
    }
    let d_in = eth_in * (1.0 - s.fee_pips / 1e6);
    let inv = 1.0 / s.sqrt_p + d_in / s.liquidity;
    let out = s.liquidity * (s.sqrt_p - 1.0 / inv);
    (out.is_finite() && out > 0.0).then_some(out)
}

/// Wei received for `tokens_in` raw token units.
pub fn sell_out(s: &PoolState, tokens_in: f64) -> Option<f64> {
    if !s.ok() || !(tokens_in > 0.0) {
        return None;
    }
    let b_in = tokens_in * (1.0 - s.fee_pips / 1e6);
    let spb = s.sqrt_p + b_in / s.liquidity;
    let out = s.liquidity * (1.0 / s.sqrt_p - 1.0 / spb);
    (out.is_finite() && out > 0.0).then_some(out)
}

/// floor(expected * (1 - slippage)) as an integer amount, saturating at u128::MAX.
pub fn min_out(expected: f64, slippage_bps: u32) -> u128 {
    if !(expected > 0.0) || slippage_bps >= 10_000 {
        return 0;
    }
    let v = (expected * (10_000 - slippage_bps) as f64 / 10_000.0).floor();
    if v >= u128::MAX as f64 {
        u128::MAX
    } else {
        v as u128
    }
}

pub fn u256_to_f64(v: U256) -> f64 {
    v.to_string().parse::<f64>().unwrap_or(f64::NAN)
}

/// sqrtPriceX96 (decimal string or U256) -> engine float representation.
pub fn sqrt_p_from_x96(x96: U256) -> f64 {
    u256_to_f64(x96) / 79228162514264337593543950336.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::str::FromStr;

    fn state(p: &Value) -> PoolState {
        PoolState {
            sqrt_p: sqrt_p_from_x96(U256::from_str(p["sqrt_price_x96"].as_str().unwrap()).unwrap()),
            liquidity: p["liquidity"].as_str().unwrap().parse().unwrap(),
            fee_pips: p["lp_fee"].as_f64().unwrap(),
            blk: 0.0,
        }
    }

    fn rel(a: f64, b: f64) -> f64 {
        (a / b - 1.0).abs()
    }

    /// Quoter outputs recorded at one block; the local formulas must reproduce them.
    /// The memecoin pool (tick spacing 200, like the pools the bot trades) matches to 1e-9. ETH/USDG has tick spacing 1,
    /// so even 0.001 ETH crosses initialized ticks and the single-range formula is off by ~2.5e-5; that is far inside
    /// the >= 10% slippage tolerance the quote is used with.
    #[test]
    fn matches_on_chain_quoter() {
        let f: Value = serde_json::from_str(include_str!("fixtures/amm_quotes.json")).unwrap();
        for (name, tol) in [("meme", 1e-9), ("eth_usdg", 1e-4)] {
            let p = &f[name];
            let s = state(p);
            let num = |k: &str| p[k].as_str().unwrap().parse::<f64>().unwrap();
            let b = buy_out(&s, num("buy_in")).unwrap();
            assert!(rel(b, num("buy_out")) < tol, "{name} buy: local {b} vs quoter {}", num("buy_out"));
            let x = sell_out(&s, num("sell_in")).unwrap();
            assert!(rel(x, num("sell_out")) < tol, "{name} sell: local {x} vs quoter {}", num("sell_out"));
        }
    }

    #[test]
    fn min_out_applies_tolerance_and_edges() {
        assert_eq!(min_out(1000.0, 1000), 900);
        assert_eq!(min_out(1000.0, 0), 1000);
        assert_eq!(min_out(1000.0, 10_000), 0);
        assert_eq!(min_out(f64::NAN, 100), 0);
        assert_eq!(min_out(-5.0, 100), 0);
    }

    #[test]
    fn rejects_bad_state() {
        let s = PoolState { sqrt_p: 0.0, liquidity: 1.0, fee_pips: 100.0, blk: 0.0 };
        assert!(buy_out(&s, 1.0).is_none());
        let s = PoolState { sqrt_p: 1.0, liquidity: f64::NAN, fee_pips: 100.0, blk: 0.0 };
        assert!(sell_out(&s, 1.0).is_none());
    }
}
