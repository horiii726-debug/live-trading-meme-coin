//! Robinhood Chain (chainId 4663) contracts used for execution.
//!
//! Every address was verified on 2026-10-18 in two independent ways:
//! 1. the official Uniswap v4 deployment list (developers.uniswap.org, "Robinhood Chain: 4663"), and
//! 2. on chain: contract code exists, `UNIVERSAL_ROUTER.poolManager()` returns `POOL_MANAGER`, real user swaps
//!    go through `UNIVERSAL_ROUTER`, and a full buy -> approve -> sell cycle built by this crate succeeds in
//!    `eth_simulateV1` against live state (see `exec selftest`).
//!
//! `verify_deployment` re-checks the critical facts at startup and refuses to trade if any of them changed.
use alloy_primitives::{address, Address};
use serde_json::{json, Value};

use crate::net::Rpc;

pub const CHAIN_ID: u64 = 4663;
pub const POOL_MANAGER: Address = address!("8366a39cc670b4001a1121b8f6a443a643e40951");
/// "Universal Router 2.1.2" (the version the Uniswap interface uses here for native-ETH swaps).
pub const UNIVERSAL_ROUTER: Address = address!("204faca1764b154221e35c0d20abb3c525710498");
/// Canonical Permit2 (same address on every chain).
pub const PERMIT2: Address = address!("000000000022d473030f116ddee9f6b43ac78ba3");
pub const V4_QUOTER: Address = address!("8dc178efb8111bb0973dd9d722ebeff267c98f94");
pub const STATE_VIEW: Address = address!("f3334192d15450cdd385c8b70e03f9a6bd9e673b");
/// Native ETH is `address(0)` in Uniswap v4 and always sorts first, so it is always `currency0`.
pub const NATIVE: Address = Address::ZERO;

/// ETH/USDG pool (fee 0.01%, tick spacing 1): deep, stable pool used by the self-test.
pub const ETH_USDG_POOL_ID: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e";
/// Block of its Initialize event (from the research dataset; the key found there is still hash-checked).
pub const ETH_USDG_INIT_BLOCK: u64 = 1_146_712;

pub fn hex_u64(v: &Value) -> Option<u64> {
    v.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
}

fn code_size(rpc: &Rpc, a: Address) -> Result<usize, String> {
    let code = rpc.call("eth_getCode", json!([format!("{a:#x}"), "latest"]))?;
    Ok(code.as_str().map(|s| s.trim_start_matches("0x").len() / 2).unwrap_or(0))
}

/// Fail-closed startup check of everything the executor relies on.
pub fn verify_deployment(rpc: &Rpc) -> Result<(), String> {
    let id = hex_u64(&rpc.call("eth_chainId", json!([]))?).ok_or("eth_chainId: bad response")?;
    if id != CHAIN_ID {
        return Err(format!("wrong chain: RPC reports chainId {id}, expected {CHAIN_ID}"));
    }
    for (name, a) in [("PoolManager", POOL_MANAGER), ("UniversalRouter", UNIVERSAL_ROUTER), ("Permit2", PERMIT2), ("V4Quoter", V4_QUOTER), ("StateView", STATE_VIEW)] {
        if code_size(rpc, a)? == 0 {
            return Err(format!("{name} {a:#x} has no code on chain {CHAIN_ID}"));
        }
    }
    // poolManager() selector 0xdc4c90d3
    let pm = rpc.call("eth_call", json!([{"to": format!("{UNIVERSAL_ROUTER:#x}"), "data": "0xdc4c90d3"}, "latest"]))?;
    let pm = pm.as_str().unwrap_or("");
    if pm.len() < 42 || !pm[pm.len() - 40..].eq_ignore_ascii_case(&format!("{POOL_MANAGER:x}")) {
        return Err(format!("UniversalRouter.poolManager() returned {pm}, expected {POOL_MANAGER:#x}"));
    }
    Ok(())
}
