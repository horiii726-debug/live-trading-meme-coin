//! `rhc-trader exec selftest`: prove the execution path against live chain state WITHOUT sending anything.
//!
//! 1. deployment checks (chain id, contract code, router -> PoolManager);
//! 2. the pool key hashes to the pool id;
//! 3. the local AMM quote equals the official V4Quoter at the same block;
//! 4. `eth_simulateV1` runs the full cycle built by this crate (buy -> token.approve(Permit2) ->
//!    Permit2.approve(router) -> sell) from a throwaway address funded only by a state override, and checks that
//!    every step succeeds, the tokens received equal the quote exactly, and the sell returns ETH.
use std::time::Duration;

use alloy_primitives::{hex, Address, Bytes, U256};
use serde_json::{json, Value};

use super::abi::{revert_name, TOPIC_TRANSFER};
use super::chain::{self, PERMIT2, UNIVERSAL_ROUTER, V4_QUOTER};
use super::{amm, pools, uniswap};
use crate::net::Rpc;

/// `eth_simulateV1` reports native ETH movements as transfers of this pseudo token.
const NATIVE_PSEUDO: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
/// Throwaway sender; it has no key and exists only inside the simulation (funded by a state override).
const SIM_SENDER: &str = "0x5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f";

fn transfers_to(logs: &Value, token: &str, to: &Address) -> U256 {
    let to_s = format!("{to:x}");
    logs.as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["address"].as_str().map_or(false, |a| a.eq_ignore_ascii_case(token)))
        .filter(|l| l["topics"][0].as_str() == Some(TOPIC_TRANSFER))
        .filter(|l| l["topics"][2].as_str().map_or(false, |t| t.to_lowercase().ends_with(&to_s)))
        .filter_map(|l| l["data"].as_str().and_then(|d| U256::from_str_radix(d.trim_start_matches("0x"), 16).ok()))
        .fold(U256::ZERO, |a, b| a.saturating_add(b))
}

/// The self-test often runs while the old bot keeps the same (public) RPC at its rate limit (an update): an HTTP
/// 429 or a network error is retried with a growing pause; any real answer (a revert, a wrong result) is final.
fn coba<T>(mut f: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut jeda = Duration::from_millis(500);
    for _ in 0..8 {
        match f() {
            Err(e) if e.starts_with("net:") || e.contains("429") || e.to_lowercase().contains("too many") => {
                std::thread::sleep(jeda);
                jeda = (jeda * 2).min(Duration::from_secs(8));
            }
            r => return r,
        }
    }
    f()
}

pub fn run(rpc_urls: &[String], doh: bool, pool: Option<&str>, init_block: Option<u64>) -> Result<(), String> {
    let rpc = Rpc::new(&rpc_urls.join(","), doh)?;
    chain::verify_deployment(&rpc)?;
    println!("1. deployment: chainId {}, PoolManager/UniversalRouter/Permit2/V4Quoter/StateView verified", chain::CHAIN_ID);

    let (pool, hint) = match pool {
        Some(p) => (p.to_lowercase(), init_block),
        None => (chain::ETH_USDG_POOL_ID.to_string(), Some(chain::ETH_USDG_INIT_BLOCK)),
    };
    let key = coba(|| pools::fetch_key(&rpc, &pool, hint))?;
    println!("2. pool {pool}: key verified (token {:#x}, fee {}, tickSpacing {}, hooks {:#x})", key.token(), key.fee, key.tick_spacing, key.hooks);

    let blk = rpc.call("eth_blockNumber", json!([]))?.as_str().ok_or("eth_blockNumber: bad response")?.to_string();
    let st = coba(|| pools::read_state(&rpc, &pool, &blk))?;
    let amount_in: u128 = 500_000_000_000_000; // 0.0005 ETH, simulated only
    let local = amm::buy_out(&st, amount_in as f64).ok_or("local quote failed (pool state)")?;
    let quote_data = uniswap::quote_calldata(&key, true, amount_in)?;
    let q = coba(|| pools::eth_call(&rpc, V4_QUOTER, &quote_data, &blk))?;
    let quoted = uniswap::decode_quote(&q)?;
    let qf = amm::u256_to_f64(quoted);
    let diff = (local / qf - 1.0).abs();
    println!("3. quote at block {}: local {local:.0} vs V4Quoter {quoted} (diff {:.6}%)", u64::from_str_radix(blk.trim_start_matches("0x"), 16).unwrap_or(0), diff * 100.0);
    if !(diff < 1e-3) {
        return Err(format!("local quote differs from the V4Quoter by {:.4}% (limit 0.1%)", diff * 100.0));
    }

    let me: Address = SIM_SENDER.parse().map_err(|_| "bad sim sender")?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let deadline = U256::from(now + 300);
    let buy = uniswap::buy_calldata(&key, amount_in, amm::min_out(qf, 100), deadline)?;
    let approve = uniswap::erc20_approve_permit2_calldata();
    let p2 = uniswap::permit2_approve_router_calldata(key.token(), now + 86_400)?;
    let sell_tokens = quoted / U256::from(2u8);
    let sell = uniswap::sell_calldata(&key, sell_tokens, 1, deadline)?;
    let call = |to: Address, value: U256, data: &Bytes| {
        json!({"from": format!("{me:#x}"), "to": format!("{to:#x}"), "value": format!("{value:#x}"), "data": format!("0x{}", hex::encode(data)), "gas": "0x16e360"})
    };
    let calls = vec![
        call(UNIVERSAL_ROUTER, U256::from(amount_in), &buy),
        call(key.token(), U256::ZERO, &approve),
        call(PERMIT2, U256::ZERO, &p2),
        call(UNIVERSAL_ROUTER, U256::ZERO, &sell),
    ];
    let req = json!({"blockStateCalls": [{"stateOverrides": {format!("{me:#x}"): {"balance": "0xde0b6b3a7640000"}}, "calls": calls}], "traceTransfers": true, "validation": false});
    let res = coba(|| rpc.call_once("eth_simulateV1", json!([req, blk]), Duration::from_secs(30)))?;
    let out = res[0]["calls"].as_array().ok_or("eth_simulateV1: bad response")?;
    let names = ["buy 0.0005 ETH", "token.approve(Permit2)", "Permit2.approve(router)", "sell half of the tokens"];
    if out.len() != names.len() {
        return Err(format!("eth_simulateV1 returned {} results, expected {}", out.len(), names.len()));
    }
    println!("4. eth_simulateV1 (nothing is sent):");
    for (n, r) in names.iter().zip(out) {
        let ok = r["status"].as_str() == Some("0x1");
        let gas = r["gasUsed"].as_str().and_then(|g| u64::from_str_radix(g.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
        println!("   {n:28} {} gasUsed {gas}", if ok { "OK  " } else { "FAIL" });
        if !ok {
            let data = r["returnData"].as_str().unwrap_or("");
            return Err(format!("{n} reverted: {} {}", revert_name(data), r["error"]));
        }
    }
    let token = format!("{:#x}", key.token());
    let got = transfers_to(&out[0]["logs"], &token, &me);
    let eth_back = transfers_to(&out[3]["logs"], NATIVE_PSEUDO, &me);
    println!("   tokens received {got} (quote {quoted}) | ETH back from selling half: {} wei", eth_back);
    if got != quoted {
        return Err("tokens received differ from the quote at the same block".into());
    }
    if eth_back.is_zero() {
        return Err("sell returned no ETH".into());
    }
    println!("SELFTEST OK: the exact calldata this binary sends works against live chain state.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::coba;

    #[test]
    fn coba_retries_rate_limits_but_not_real_answers() {
        let mut n = 0;
        let r: Result<u32, String> = coba(|| {
            n += 1;
            if n < 3 { Err("net: http 429 {\"error\":{\"code\":429}}".into()) } else { Ok(7) }
        });
        assert_eq!((r, n), (Ok(7), 3), "429 / network errors are retried until an answer comes");
        let mut m = 0;
        let r: Result<u32, String> = coba(|| {
            m += 1;
            Err("rpc: execution reverted".into())
        });
        assert_eq!((r.is_err(), m), (true, 1), "a revert is a real answer: no retry");
    }
}
