//! Exact calldata for every transaction the executor sends (pure functions, no I/O).
//!
//! Buy  (ETH -> token): Universal Router `execute(V4_SWAP)` with actions
//!       SWAP_EXACT_IN_SINGLE(zeroForOne, amountIn, minOut) + SETTLE_ALL(ETH, amountIn) + TAKE_ALL(token, minOut),
//!       paid with `msg.value = amountIn`. This is the same plan the Uniswap interface sends (golden test below).
//! Sell (token -> ETH): SETTLE(token, amount, payerIsUser) + SWAP_EXACT_IN_SINGLE(amountIn = OPEN_DELTA)
//!       + TAKE_ALL(ETH, minOut). Settling first and swapping the open credit also works for tokens that take a
//!       fee on transfer (the swap uses what the PoolManager actually received).
//! Sells pull tokens through Permit2, so each token needs `token.approve(Permit2)` and
//! `Permit2.approve(token, router)` once.
use alloy_primitives::aliases::{I24, U160, U24, U48};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_sol_types::{SolCall, SolValue};
use serde::{Deserialize, Serialize};

use super::abi::{
    ExactInputSingleParams, IERC20, IPermit2, IUniversalRouter, IV4Quoter, PoolKey, QuoteExactSingleParams, ACT_SETTLE, ACT_SETTLE_ALL,
    ACT_SWAP_EXACT_IN_SINGLE, ACT_TAKE_ALL, CMD_V4_SWAP, OPEN_DELTA,
};
use super::chain::{NATIVE, PERMIT2, UNIVERSAL_ROUTER};

/// Uniswap v4 pool key (the pool id is keccak256 of its ABI encoding).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}

/// Fee value Uniswap v4 uses to flag a dynamic-fee pool.
const DYNAMIC_FEE_FLAG: u32 = 0x80_0000;

impl Key {
    pub fn to_sol(&self) -> Result<PoolKey, String> {
        Ok(PoolKey {
            currency0: self.currency0,
            currency1: self.currency1,
            fee: U24::try_from(self.fee).map_err(|_| format!("fee {} does not fit uint24", self.fee))?,
            tickSpacing: I24::try_from(self.tick_spacing).map_err(|_| format!("tick spacing {} does not fit int24", self.tick_spacing))?,
            hooks: self.hooks,
        })
    }

    pub fn pool_id(&self) -> Result<B256, String> {
        Ok(keccak256(self.to_sol()?.abi_encode()))
    }

    /// The traded token (ETH is always currency0).
    pub fn token(&self) -> Address {
        self.currency1
    }

    /// Refuse anything the executor was not built and tested for.
    pub fn validate(&self, expected_pool_id: &str) -> Result<(), String> {
        if self.currency0 != NATIVE {
            return Err(format!("pool is not a native-ETH pool (currency0 {:#x})", self.currency0));
        }
        if self.currency1 == NATIVE {
            return Err("currency1 is native ETH".into());
        }
        if self.fee >= DYNAMIC_FEE_FLAG {
            return Err(format!("dynamic or invalid fee {}", self.fee));
        }
        let id = format!("{:#x}", self.pool_id()?);
        if !id.eq_ignore_ascii_case(expected_pool_id) {
            return Err(format!("pool key hashes to {id}, expected {expected_pool_id}"));
        }
        Ok(())
    }
}

fn execute(actions: &[u8], params: Vec<Bytes>, deadline: U256) -> Bytes {
    let v4_input = (Bytes::copy_from_slice(actions), params).abi_encode_params();
    IUniversalRouter::executeCall { commands: Bytes::from(vec![CMD_V4_SWAP]), inputs: vec![v4_input.into()], deadline }.abi_encode().into()
}

/// Router calldata to buy `currency1` with exactly `amount_in` wei of ETH (send it as `msg.value`).
pub fn buy_calldata(key: &Key, amount_in: u128, min_out: u128, deadline: U256) -> Result<Bytes, String> {
    let swap = ExactInputSingleParams {
        poolKey: key.to_sol()?,
        zeroForOne: true,
        amountIn: amount_in,
        amountOutMinimum: min_out,
        minHopPriceX36: U256::ZERO,
        hookData: Bytes::new(),
    };
    let params = vec![
        swap.abi_encode().into(),
        (key.currency0, U256::from(amount_in)).abi_encode_params().into(),
        (key.currency1, U256::from(min_out)).abi_encode_params().into(),
    ];
    Ok(execute(&[ACT_SWAP_EXACT_IN_SINGLE, ACT_SETTLE_ALL, ACT_TAKE_ALL], params, deadline))
}

/// Router calldata to sell `token_amount` of `currency1` for at least `min_out` wei of ETH.
pub fn sell_calldata(key: &Key, token_amount: U256, min_out: u128, deadline: U256) -> Result<Bytes, String> {
    let swap = ExactInputSingleParams {
        poolKey: key.to_sol()?,
        zeroForOne: false,
        amountIn: OPEN_DELTA,
        amountOutMinimum: min_out,
        minHopPriceX36: U256::ZERO,
        hookData: Bytes::new(),
    };
    let params = vec![
        (key.currency1, token_amount, true).abi_encode_params().into(),
        swap.abi_encode().into(),
        (key.currency0, U256::from(min_out)).abi_encode_params().into(),
    ];
    Ok(execute(&[ACT_SETTLE, ACT_SWAP_EXACT_IN_SINGLE, ACT_TAKE_ALL], params, deadline))
}

/// `token.approve(Permit2, max)`.
pub fn erc20_approve_permit2_calldata() -> Bytes {
    IERC20::approveCall { spender: PERMIT2, amount: U256::MAX }.abi_encode().into()
}

/// `Permit2.approve(token, router, max, expiration)`.
pub fn permit2_approve_router_calldata(token: Address, expiration_unix: u64) -> Result<Bytes, String> {
    let exp = U48::try_from(expiration_unix).map_err(|_| "Permit2 expiration does not fit uint48".to_string())?;
    Ok(IPermit2::approveCall { token, spender: UNIVERSAL_ROUTER, amount: U160::MAX, expiration: exp }.abi_encode().into())
}

/// V4Quoter `quoteExactInputSingle` calldata.
pub fn quote_calldata(key: &Key, zero_for_one: bool, exact_amount: u128) -> Result<Bytes, String> {
    let p = QuoteExactSingleParams { poolKey: key.to_sol()?, zeroForOne: zero_for_one, exactAmount: exact_amount, hookData: Bytes::new() };
    Ok(IV4Quoter::quoteExactInputSingleCall { params: p }.abi_encode().into())
}

/// Decode a V4Quoter return value into `amountOut`.
pub fn decode_quote(ret: &[u8]) -> Result<U256, String> {
    IV4Quoter::quoteExactInputSingleCall::abi_decode_returns(ret).map(|r| r.amountOut).map_err(|e| format!("quote decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;
    use alloy_sol_types::SolType;
    use serde_json::Value;
    use std::str::FromStr;

    fn addr(s: &str) -> Address {
        Address::from_str(s).unwrap()
    }

    fn fixture(name: &str) -> Value {
        let raw = match name {
            "buy" => include_str!("fixtures/real_router_buy_tx.json"),
            _ => include_str!("fixtures/amm_quotes.json"),
        };
        serde_json::from_str(raw).unwrap()
    }

    /// Our encoder must reproduce a real Uniswap-interface buy on this chain byte for byte.
    #[test]
    fn golden_real_router_buy() {
        let f = fixture("buy");
        let k = &f["pool_key"];
        let key = Key {
            currency0: addr(k["currency0"].as_str().unwrap()),
            currency1: addr(k["currency1"].as_str().unwrap()),
            fee: k["fee"].as_u64().unwrap() as u32,
            tick_spacing: k["tick_spacing"].as_i64().unwrap() as i32,
            hooks: addr(k["hooks"].as_str().unwrap()),
        };
        let amount_in: u128 = f["amount_in"].as_str().unwrap().parse().unwrap();
        let min_out: u128 = f["amount_out_minimum"].as_str().unwrap().parse().unwrap();
        let deadline = U256::from_str(f["deadline"].as_str().unwrap()).unwrap();
        let ours = buy_calldata(&key, amount_in, min_out, deadline).unwrap();
        let real = hex::decode(f["input"].as_str().unwrap()).unwrap();
        assert_eq!(ours.as_ref(), real.as_slice(), "buy calldata differs from the real router transaction");
        assert_eq!(f["value_wei"].as_str().unwrap(), amount_in.to_string());
    }

    #[test]
    fn pool_ids_match_chain() {
        let f = fixture("amm");
        for name in ["meme", "eth_usdg"] {
            let p = &f[name];
            let k = p["key"].as_array().unwrap();
            let key = Key {
                currency0: addr(k[0].as_str().unwrap()),
                currency1: addr(k[1].as_str().unwrap()),
                fee: k[2].as_u64().unwrap() as u32,
                tick_spacing: k[3].as_i64().unwrap() as i32,
                hooks: addr(k[4].as_str().unwrap()),
            };
            key.validate(p["pool_id"].as_str().unwrap()).unwrap();
        }
    }

    #[test]
    fn validate_rejects_wrong_or_unsupported_keys() {
        let good = Key { currency0: NATIVE, currency1: addr("0x5fc5360d0400a0fd4f2af552add042d716f1d168"), fee: 100, tick_spacing: 1, hooks: NATIVE };
        good.validate(super::super::chain::ETH_USDG_POOL_ID).unwrap();
        assert!(Key { tick_spacing: 60, ..good }.validate(super::super::chain::ETH_USDG_POOL_ID).is_err());
        assert!(Key { fee: DYNAMIC_FEE_FLAG, ..good }.validate(super::super::chain::ETH_USDG_POOL_ID).is_err());
        assert!(Key { currency0: good.currency1, currency1: good.currency0, ..good }.validate(super::super::chain::ETH_USDG_POOL_ID).is_err());
    }

    /// Decode our own sell calldata back and check every field.
    #[test]
    fn sell_calldata_round_trip() {
        let key = Key { currency0: NATIVE, currency1: addr("0x147190c741f07748cc03ec7ac23bdbba62409f43"), fee: 10_000, tick_spacing: 200, hooks: NATIVE };
        let amount = U256::from(123_456_789_000_000_000_000u128);
        let data = sell_calldata(&key, amount, 42, U256::from(1_800_000_000u64)).unwrap();
        let call = IUniversalRouter::executeCall::abi_decode(&data).unwrap();
        assert_eq!(call.commands.as_ref(), &[CMD_V4_SWAP]);
        assert_eq!(call.deadline, U256::from(1_800_000_000u64));
        type Plan = (alloy_sol_types::sol_data::Bytes, alloy_sol_types::sol_data::Array<alloy_sol_types::sol_data::Bytes>);
        let (actions, params) = <Plan as SolType>::abi_decode_params(&call.inputs[0]).unwrap();
        assert_eq!(actions.as_ref(), &[ACT_SETTLE, ACT_SWAP_EXACT_IN_SINGLE, ACT_TAKE_ALL]);
        type Settle = (alloy_sol_types::sol_data::Address, alloy_sol_types::sol_data::Uint<256>, alloy_sol_types::sol_data::Bool);
        let (cur, amt, payer_is_user) = <Settle as SolType>::abi_decode_params(&params[0]).unwrap();
        assert_eq!((cur, amt, payer_is_user), (key.currency1, amount, true));
        let swap = <ExactInputSingleParams as SolType>::abi_decode(&params[1]).unwrap();
        assert!(!swap.zeroForOne);
        assert_eq!(swap.amountIn, OPEN_DELTA);
        assert_eq!(swap.amountOutMinimum, 42);
        assert_eq!(swap.minHopPriceX36, U256::ZERO);
        assert_eq!(swap.poolKey.currency1, key.currency1);
        type Take = (alloy_sol_types::sol_data::Address, alloy_sol_types::sol_data::Uint<256>);
        let (tcur, tmin) = <Take as SolType>::abi_decode_params(&params[2]).unwrap();
        assert_eq!((tcur, tmin), (NATIVE, U256::from(42u64)));
    }

    #[test]
    fn approvals_target_permit2_and_router() {
        let a = IERC20::approveCall::abi_decode(&erc20_approve_permit2_calldata()).unwrap();
        assert_eq!((a.spender, a.amount), (PERMIT2, U256::MAX));
        let token = addr("0x147190c741f07748cc03ec7ac23bdbba62409f43");
        let p = IPermit2::approveCall::abi_decode(&permit2_approve_router_calldata(token, 1_900_000_000).unwrap()).unwrap();
        assert_eq!((p.token, p.spender, p.amount), (token, UNIVERSAL_ROUTER, U160::MAX));
        assert_eq!(p.expiration, U48::from(1_900_000_000u64));
    }
}
