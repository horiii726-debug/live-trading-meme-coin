//! Read fills from transaction receipts.
//!
//! Uniswap v4 `Swap` amounts are from the swapper's side: negative = paid, positive = received. On a buy
//! amount0 = -ETH paid and amount1 = +tokens; on a sell amount0 = +ETH received and amount1 = -tokens. Tokens
//! actually credited to the wallet are taken from ERC20 `Transfer` logs, so fee-on-transfer tokens are counted
//! correctly. Native ETH moves emit no log, so ETH amounts come from the PoolManager `Swap` log of our pool.
use alloy_primitives::{Address, I256, U256};
use serde_json::Value;

use super::abi::{TOPIC_SWAP, TOPIC_TRANSFER};
use super::chain::POOL_MANAGER;

fn hex_u256(v: &Value) -> Option<U256> {
    v.as_str().and_then(|s| U256::from_str_radix(s.trim_start_matches("0x"), 16).ok())
}

fn topic_addr(t: &Value) -> Option<Address> {
    let s = t.as_str()?;
    if s.len() < 42 {
        return None;
    }
    s[s.len() - 40..].parse::<Address>().ok()
}

fn word(data: &str, i: usize) -> Option<U256> {
    let d = data.trim_start_matches("0x");
    let w = d.get(i * 64..(i + 1) * 64)?;
    U256::from_str_radix(w, 16).ok()
}

pub fn status_ok(r: &Value) -> bool {
    r["status"].as_str() == Some("0x1")
}

/// gasUsed x effectiveGasPrice (includes the L1 data component on Arbitrum Orbit chains).
pub fn gas_cost(r: &Value) -> U256 {
    match (hex_u256(&r["gasUsed"]), hex_u256(&r["effectiveGasPrice"])) {
        (Some(g), Some(p)) => g.saturating_mul(p),
        _ => U256::ZERO,
    }
}

pub fn block_number(r: &Value) -> Option<u64> {
    r["blockNumber"].as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
}

fn logs(r: &Value) -> impl Iterator<Item = &Value> {
    r["logs"].as_array().into_iter().flatten()
}

/// (amount0, amount1) of the first PoolManager `Swap` log for `pool_id`.
pub fn swap_amounts(r: &Value, pool_id: &str) -> Option<(I256, I256)> {
    logs(r).find_map(|l| {
        let addr = l["address"].as_str()?.parse::<Address>().ok()?;
        let t = l["topics"].as_array()?;
        if addr != POOL_MANAGER || t.first()?.as_str()? != TOPIC_SWAP || !t.get(1)?.as_str()?.eq_ignore_ascii_case(pool_id) {
            return None;
        }
        let d = l["data"].as_str()?;
        Some((I256::from_raw(word(d, 0)?), I256::from_raw(word(d, 1)?)))
    })
}

/// Sum of ERC20 `Transfer` amounts of `token` with the given direction relative to `wallet`.
fn transfers(r: &Value, token: Address, wallet: Address, incoming: bool) -> U256 {
    logs(r)
        .filter_map(|l| {
            let addr = l["address"].as_str()?.parse::<Address>().ok()?;
            let t = l["topics"].as_array()?;
            if addr != token || t.first()?.as_str()? != TOPIC_TRANSFER || t.len() < 3 {
                return None;
            }
            let side = topic_addr(if incoming { &t[2] } else { &t[1] })?;
            if side != wallet {
                return None;
            }
            word(l["data"].as_str()?, 0)
        })
        .fold(U256::ZERO, |a, b| a.saturating_add(b))
}

pub fn tokens_received(r: &Value, token: Address, wallet: Address) -> U256 {
    transfers(r, token, wallet, true)
}

pub fn tokens_sent(r: &Value, token: Address, wallet: Address) -> U256 {
    transfers(r, token, wallet, false)
}

/// Magnitude of a signed swap amount as U256 (0 if the sign is not the expected one).
pub fn paid(a: I256) -> U256 {
    if a.is_negative() {
        a.unsigned_abs()
    } else {
        U256::ZERO
    }
}

pub fn received(a: I256) -> U256 {
    if a.is_positive() {
        a.unsigned_abs()
    } else {
        U256::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn case(name: &str) -> (Value, Address) {
        let f: Value = serde_json::from_str(include_str!("fixtures/real_receipts.json")).unwrap();
        let c = &f["cases"][name];
        (c["receipt"].clone(), Address::from_str(c["wallet"].as_str().unwrap()).unwrap())
    }

    fn pool_of(r: &Value) -> String {
        logs(r).find(|l| l["topics"][0].as_str() == Some(TOPIC_SWAP)).unwrap()["topics"][1].as_str().unwrap().to_string()
    }

    const TOKEN: &str = "0xa84f3ee3fe9d1a728d896c9c93dba88beb291c1e";

    #[test]
    fn parses_real_buy_receipt() {
        let (r, wallet) = case("buy");
        let token = Address::from_str(TOKEN).unwrap();
        assert!(status_ok(&r));
        let (a0, a1) = swap_amounts(&r, &pool_of(&r)).unwrap();
        assert_eq!(paid(a0), U256::from(261229139489993835u128));
        assert_eq!(received(a1), U256::from_str("81629232454963094595215").unwrap());
        assert_eq!(tokens_received(&r, token, wallet), U256::from_str("81629232454963094595215").unwrap());
        assert_eq!(gas_cost(&r), U256::from(102152u64 * 20196000u64));
        assert!(swap_amounts(&r, "0x00000000000000000000000000000000000000000000000000000000000000ff").is_none());
    }

    #[test]
    fn parses_real_sell_receipt() {
        let (r, wallet) = case("sell");
        let token = Address::from_str(TOKEN).unwrap();
        let (a0, a1) = swap_amounts(&r, &pool_of(&r)).unwrap();
        assert_eq!(received(a0), U256::from(1251649488245227u128));
        assert_eq!(paid(a1), U256::from_str("389324828137296312042").unwrap());
        assert_eq!(tokens_sent(&r, token, wallet), U256::from_str("389324828137296312042").unwrap());
        assert_eq!(tokens_received(&r, token, wallet), U256::ZERO);
    }
}
