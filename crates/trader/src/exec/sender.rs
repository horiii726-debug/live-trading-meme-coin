//! Transaction pipeline: EIP-1559 signing, local nonce, cached base fee, parallel broadcast, receipts.
//!
//! Latency: nothing on the send path waits for the network except the broadcast itself. The nonce is tracked
//! locally, the base fee is refreshed in the background, gas limits are fixed (on this Arbitrum Orbit chain only
//! gas *used* is charged), and the signed transaction goes to every RPC endpoint at once; the first one that
//! accepts it wins.
use std::time::Duration;

use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{hex, Address, Bytes, TxKind, B256, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use serde_json::{json, Value};

use super::chain::{hex_u64, CHAIN_ID};
use crate::net::Rpc;

/// A signed transaction ready to broadcast.
#[derive(Clone, Debug)]
pub struct SignedTx {
    pub hash: B256,
    pub raw: Vec<u8>,
    pub nonce: u64,
}

pub fn sign_eip1559(
    signer: &PrivateKeySigner,
    nonce: u64,
    to: Address,
    value: U256,
    input: Bytes,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
) -> Result<SignedTx, String> {
    let tx = TxEip1559 {
        chain_id: CHAIN_ID,
        nonce,
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        to: TxKind::Call(to),
        value,
        access_list: Default::default(),
        input,
    };
    let sig = signer.sign_hash_sync(&tx.signature_hash()).map_err(|e| format!("sign: {e}"))?;
    let signed = tx.into_signed(sig);
    Ok(SignedTx { hash: *signed.hash(), raw: signed.encoded_2718(), nonce })
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Accepted,
    /// Every endpoint said the nonce is already used: resync and rebuild.
    NonceTooLow,
    /// Nobody accepted it. `maybe_sent` is true when at least one endpoint timed out or failed at the network level
    /// (the sequencer may still have it), so the caller must check before reusing the nonce.
    Rejected { error: String, maybe_sent: bool },
}

fn classify(r: &Result<Value, String>, hash: &B256) -> (bool, bool, bool, bool, String) {
    // (accepted, nonce_too_low, failed at the network level (worth re-sending), may have been delivered, message)
    match r {
        Ok(v) => {
            let ok = v.as_str().map(|h| h.eq_ignore_ascii_case(&format!("{hash:#x}"))).unwrap_or(false);
            (ok, false, false, false, if ok { String::new() } else { format!("unexpected response {v}") })
        }
        Err(e) => {
            let l = e.to_lowercase();
            if l.contains("already known") || l.contains("known transaction") || l.contains("already imported") {
                (true, false, false, false, String::new())
            } else if l.contains("nonce too low") {
                (false, true, false, false, e.clone())
            } else {
                let network = !l.starts_with("rpc:");
                // refused before anything reached a node: connect / TLS / DoH failure, or an HTTP 4xx such as 429.
                // Only a failure after the request was written (read timeout, 5xx) may hide a delivered transaction.
                let refused = l.starts_with("net: tcp") || l.starts_with("net: tls") || l.starts_with("net: http 4") || l.starts_with("doh");
                (false, false, network, network && !refused, e.clone())
            }
        }
    }
}

pub struct Sender {
    signer: PrivateKeySigner,
    pub address: Address,
    pub reads: Rpc,
    endpoints: Vec<Rpc>,
    pub nonce: u64,
    pub base_fee: u128,
}

impl Sender {
    pub fn new(signer: PrivateKeySigner, urls: &[String], doh: bool) -> Result<Sender, String> {
        if urls.is_empty() {
            return Err("no RPC endpoints".into());
        }
        let reads = Rpc::new(&urls.join(","), doh)?;
        let endpoints = urls.iter().map(|u| Rpc::new(u, doh)).collect::<Result<Vec<_>, _>>()?;
        let address = signer.address();
        Ok(Sender { signer, address, reads, endpoints, nonce: 0, base_fee: 0 })
    }

    pub fn sync_nonce(&mut self) -> Result<u64, String> {
        let v = self.reads.call("eth_getTransactionCount", json!([format!("{:#x}", self.address), "pending"]))?;
        self.nonce = hex_u64(&v).ok_or("eth_getTransactionCount: bad response")?;
        Ok(self.nonce)
    }

    /// Number of transactions from our address included in blocks.
    pub fn mined_nonce(&self) -> Result<u64, String> {
        let v = self.reads.call_once("eth_getTransactionCount", json!([format!("{:#x}", self.address), "latest"]), Duration::from_secs(5))?;
        hex_u64(&v).ok_or_else(|| "eth_getTransactionCount: bad response".to_string())
    }

    pub fn refresh_base_fee(&mut self) -> Result<u128, String> {
        let b = self.reads.call_once("eth_getBlockByNumber", json!(["latest", false]), Duration::from_secs(5))?;
        let fee = b["baseFeePerGas"].as_str().and_then(|s| u128::from_str_radix(s.trim_start_matches("0x"), 16).ok()).ok_or("block without baseFeePerGas")?;
        self.base_fee = fee;
        Ok(fee)
    }

    pub fn base_fee_gwei(&self) -> f64 {
        self.base_fee as f64 / 1e9
    }

    /// maxFeePerGas = max(2 x base fee, base fee + 0.01 gwei); priority fee 0 (Arbitrum ignores tips).
    pub fn max_fee(&self) -> u128 {
        self.base_fee.saturating_mul(2).max(self.base_fee + 10_000_000)
    }

    pub fn eth_balance(&self) -> Result<U256, String> {
        let v = self.reads.call_once("eth_getBalance", json!([format!("{:#x}", self.address), "latest"]), Duration::from_secs(5))?;
        v.as_str().and_then(|s| U256::from_str_radix(s.trim_start_matches("0x"), 16).ok()).ok_or_else(|| "eth_getBalance: bad response".into())
    }

    /// Sign with the current local nonce (does not advance it).
    pub fn sign_next(&self, to: Address, value: U256, input: Bytes, gas_limit: u64) -> Result<SignedTx, String> {
        if self.base_fee == 0 {
            return Err("base fee unknown".into());
        }
        sign_eip1559(&self.signer, self.nonce, to, value, input, gas_limit, self.max_fee(), 0)
    }

    /// Broadcast to every endpoint in parallel. Advances the local nonce when the transaction was accepted.
    /// When every endpoint failed at the network level (HTTP 429, timeout), the very same signed bytes are sent
    /// again, twice at most: same hash and nonce, so a copy that did arrive just answers "already known".
    pub fn broadcast(&mut self, tx: &SignedTx) -> SendOutcome {
        let raw = format!("0x{}", hex::encode(&tx.raw));
        let mut outcome = SendOutcome::Rejected { error: "no endpoint".into(), maybe_sent: false };
        let mut delivered_before = false; // an earlier round may have reached a node
        for round in 0..3 {
            if round > 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            let results: Vec<Result<Value, String>> = std::thread::scope(|s| {
                let hs: Vec<_> = self
                    .endpoints
                    .iter()
                    .map(|ep| {
                        let raw = raw.clone();
                        s.spawn(move || ep.call_once("eth_sendRawTransaction", json!([raw]), Duration::from_secs(4)))
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("broadcast thread panicked".into()))).collect()
            });
            let mut nonce_low = false;
            let mut maybe = false;
            let mut all_network = !results.is_empty();
            let mut first_err = String::new();
            for r in &results {
                let (ok, low, net, may, msg) = classify(r, &tx.hash);
                if ok {
                    if tx.nonce == self.nonce {
                        self.nonce += 1;
                    }
                    return SendOutcome::Accepted;
                }
                nonce_low |= low;
                maybe |= may;
                all_network &= net;
                if first_err.is_empty() {
                    first_err = msg;
                }
            }
            if nonce_low && !maybe {
                if delivered_before {
                    // these very bytes may have gone out in an earlier round: "nonce too low" now almost certainly
                    // means our copy was included. Keep it in flight (receipts settle it); never sign a second one.
                    self.nonce = self.nonce.max(tx.nonce + 1);
                    return SendOutcome::Accepted;
                }
                return SendOutcome::NonceTooLow;
            }
            delivered_before |= maybe;
            outcome = SendOutcome::Rejected { error: first_err, maybe_sent: delivered_before };
            if !all_network {
                break; // a node actually answered with a refusal: sending the same bytes again cannot help
            }
        }
        outcome
    }

    /// Is the transaction known to the chain? Asks every endpoint (a second time if none answered):
    /// Some(true) = some node has it, Some(false) = a node answered and none has it, None = no node answered.
    /// A failed lookup must never read as "not on chain": that would drop a mined buy (tokens never tracked or sold).
    /// One answering node is enough for "no", so a dead endpoint cannot keep a transaction undecided forever.
    pub fn tx_known(&self, hash: &B256) -> Option<bool> {
        let p = json!([format!("{hash:#x}")]);
        for round in 0..2 {
            if round > 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            let mut answered = false;
            for ep in &self.endpoints {
                match ep.call_once("eth_getTransactionByHash", p.clone(), Duration::from_secs(4)) {
                    Ok(v) if !v.is_null() => return Some(true),
                    Ok(_) => answered = true,
                    Err(_) => {}
                }
            }
            if answered {
                return Some(false);
            }
        }
        None
    }

    /// Receipts for many hashes in one HTTP request. `None` = not mined yet (or the lookup failed).
    pub fn receipts(&self, hashes: &[B256]) -> Vec<Option<Value>> {
        if hashes.is_empty() {
            return Vec::new();
        }
        let calls: Vec<(&str, Value)> = hashes.iter().map(|h| ("eth_getTransactionReceipt", json!([format!("{h:#x}")]))).collect();
        match self.reads.batch(&calls, Duration::from_secs(5)) {
            Ok(v) => v.into_iter().map(|r| r.ok().filter(|x| !x.is_null())).collect(),
            Err(_) => vec![None; hashes.len()],
        }
    }

    pub fn eth_call(&self, to: Address, data: &Bytes, value: U256) -> Result<Bytes, String> {
        self.eth_call_with(to, data, value, None)
    }

    /// eth_call with the gas limit the real transaction would carry, so a call that would run out of gas on chain
    /// fails the simulation too.
    pub fn eth_call_gas(&self, to: Address, data: &Bytes, value: U256, gas: u64) -> Result<Bytes, String> {
        self.eth_call_with(to, data, value, Some(gas))
    }

    fn eth_call_with(&self, to: Address, data: &Bytes, value: U256, gas: Option<u64>) -> Result<Bytes, String> {
        let mut call = json!({"from": format!("{:#x}", self.address), "to": format!("{to:#x}"), "data": format!("0x{}", hex::encode(data)), "value": format!("{value:#x}")});
        if let Some(g) = gas {
            call["gas"] = json!(format!("{g:#x}"));
        }
        let v = self.reads.call_once("eth_call", json!([call, "latest"]), Duration::from_secs(5))?;
        let s = v.as_str().ok_or("eth_call: bad response")?;
        hex::decode(s).map(Bytes::from).map_err(|e| format!("eth_call: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::wallet::parse_key;
    use std::str::FromStr;

    // Hardhat/Anvil test account #0 (public test key, never funded).
    const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    /// Offline round trip: decode the signed bytes back as an EIP-2718 envelope, check every field and recover the
    /// signer from the signature. Nothing is sent anywhere.
    #[test]
    fn signed_tx_round_trips_and_recovers_signer() {
        use alloy_consensus::TxEnvelope;
        use alloy_eips::eip2718::Decodable2718;
        let s = parse_key(TEST_KEY).unwrap();
        let to = Address::from_str("0x000000000000000000000000000000000000dead").unwrap();
        let tx = sign_eip1559(&s, 7, to, U256::ZERO, Bytes::new(), 21_000, 40_000_000, 0).unwrap();
        assert_eq!(tx.raw[0], 0x02, "EIP-1559 transactions start with type byte 0x02");
        let env = TxEnvelope::decode_2718(&mut tx.raw.as_slice()).unwrap();
        let TxEnvelope::Eip1559(signed) = &env else { panic!("not an EIP-1559 envelope") };
        let t = signed.tx();
        assert_eq!((t.chain_id, t.nonce, t.gas_limit, t.max_fee_per_gas, t.max_priority_fee_per_gas), (CHAIN_ID, 7, 21_000, 40_000_000, 0));
        assert_eq!(t.to, TxKind::Call(to));
        assert_eq!(*signed.hash(), tx.hash);
        let who = signed.signature().recover_address_from_prehash(&t.signature_hash()).unwrap();
        assert_eq!(who, s.address());
    }

    #[test]
    fn classify_broadcast_responses() {
        let h = B256::repeat_byte(0xab);
        assert_eq!(classify(&Ok(json!(format!("{h:#x}"))), &h).0, true);
        assert_eq!(classify(&Err("rpc: already known".into()), &h).0, true);
        assert_eq!(classify(&Err("rpc: nonce too low: next nonce 5, tx nonce 4".into()), &h).1, true);
        let (ok, low, net, maybe, _) = classify(&Err("net: baca connection timed out".into()), &h);
        assert!(!ok && !low && net && maybe, "a timeout after the request was written may hide a delivered tx");
        let (ok, low, net, maybe, _) = classify(&Err("net: http 429 {\"error\":{\"code\":429}}".into()), &h);
        assert!(!ok && !low && net && !maybe, "a 429 is re-sent but was never delivered");
        let (_, _, net, maybe, _) = classify(&Err("net: tcp connection refused".into()), &h);
        assert!(net && !maybe, "nothing written before the connection failed");
        let (ok, low, net, maybe, _) = classify(&Err("rpc: insufficient funds for gas * price + value".into()), &h);
        assert!(!ok && !low && !net && !maybe);
    }
}
