//! Read a pool's key and current state from chain (manual buys and the self-test; the strategy path uses the
//! engine's own state and the key recorded at pool birth).
use std::time::Duration;

use alloy_primitives::{hex, Address, Bytes, B256, U256};
use alloy_sol_types::SolCall;
use serde_json::json;

use super::abi::IStateView;
use super::amm::{self, PoolState};
use super::chain::{POOL_MANAGER, STATE_VIEW};
use super::uniswap::Key;
use crate::net::Rpc;

/// keccak256("Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)")
pub const TOPIC_INITIALIZE: &str = "0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438";

pub fn eth_call(rpc: &Rpc, to: Address, data: &Bytes, block: &str) -> Result<Bytes, String> {
    let v = rpc.call_once("eth_call", json!([{"to": format!("{to:#x}"), "data": format!("0x{}", hex::encode(data))}, block]), Duration::from_secs(8))?;
    let s = v.as_str().ok_or("eth_call: bad response")?;
    hex::decode(s).map(Bytes::from).map_err(|e| format!("eth_call: {e}"))
}

/// Pool key from its `Initialize` event, verified against the pool id (a wrong hint can only make the lookup fail,
/// never return a different pool). With `init_block` the search is one 1000-block query; without it the whole
/// history is scanned, which needs an RPC endpoint that allows wide log ranges (keep the public RPC in RPC_URLS).
pub fn fetch_key(rpc: &Rpc, pool: &str, init_block: Option<u64>) -> Result<Key, String> {
    let (from, to) = match init_block {
        Some(b) => {
            // never ask for blocks the node does not have yet (young pools)
            let tip = rpc.call_once("eth_blockNumber", json!([]), Duration::from_secs(5))?;
            let tip = tip.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).ok_or("eth_blockNumber: bad response")?;
            (format!("{b:#x}"), format!("{:#x}", (b + 999).min(tip)))
        }
        None => ("0x0".to_string(), "latest".to_string()),
    };
    let logs = rpc.call_once(
        "eth_getLogs",
        json!([{"address": format!("{POOL_MANAGER:#x}"), "topics": [TOPIC_INITIALIZE, pool], "fromBlock": from, "toBlock": to}]),
        Duration::from_secs(30),
    )?;
    let l = logs.as_array().and_then(|a| a.first()).ok_or("pool not found (no Initialize event)")?;
    let t = l["topics"].as_array().ok_or("bad log")?;
    let addr = |i: usize| -> Result<Address, String> {
        let s = t.get(i).and_then(|x| x.as_str()).ok_or("bad topic")?;
        s.get(s.len().saturating_sub(40)..).ok_or("bad topic")?.parse::<Address>().map_err(|e| e.to_string())
    };
    let d = l["data"].as_str().ok_or("bad data")?.trim_start_matches("0x");
    let word = |i: usize| d.get(i * 64..(i + 1) * 64).ok_or_else(|| "short Initialize data".to_string());
    let fee = u32::from_str_radix(&word(0)?[56..], 16).map_err(|e| e.to_string())?;
    let raw_ts = i32::from_str_radix(&word(1)?[58..], 16).map_err(|e| e.to_string())?;
    let tick_spacing = if raw_ts & 0x80_0000 != 0 { raw_ts - 0x100_0000 } else { raw_ts };
    let hooks = word(2)?[24..].parse::<Address>().map_err(|e| e.to_string())?;
    let key = Key { currency0: addr(2)?, currency1: addr(3)?, fee, tick_spacing, hooks };
    key.validate(pool)?;
    Ok(key)
}

/// Current pool state from StateView at `block` ("latest" or a hex block number).
pub fn read_state(rpc: &Rpc, pool: &str, block: &str) -> Result<PoolState, String> {
    let id: B256 = pool.parse().map_err(|_| "bad pool id".to_string())?;
    let s0 = eth_call(rpc, STATE_VIEW, &IStateView::getSlot0Call { poolId: id }.abi_encode().into(), block)?;
    let s0 = IStateView::getSlot0Call::abi_decode_returns(&s0).map_err(|e| e.to_string())?;
    let lq = eth_call(rpc, STATE_VIEW, &IStateView::getLiquidityCall { poolId: id }.abi_encode().into(), block)?;
    let lq = IStateView::getLiquidityCall::abi_decode_returns(&lq).map_err(|e| e.to_string())?;
    Ok(PoolState { sqrt_p: amm::sqrt_p_from_x96(U256::from(s0.sqrtPriceX96)), liquidity: lq as f64, fee_pips: s0.lpFee.to::<u32>() as f64, blk: 0.0 })
}
