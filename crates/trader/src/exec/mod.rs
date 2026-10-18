//! Live execution layer: turns the strategy's model orders into real Uniswap v4 transactions.
//!
//! The strategy (engine + account) is unchanged and decides everything; this layer only executes.
//! Modes (EXEC_MODE): `off` (default, paper only), `shadow` (build + sign + simulate, never broadcast),
//! `live` (broadcast; requires EXEC_LIVE_CONFIRM).
pub mod abi;
pub mod amm;
pub mod chain;
pub mod config;
pub mod fills;
pub mod ledger;
pub mod pools;
pub mod selftest;
pub mod sender;
pub mod uniswap;
pub mod wallet;
pub mod worker;

pub use worker::{start, Alert, ExecHandle, Order, OrderKind};
