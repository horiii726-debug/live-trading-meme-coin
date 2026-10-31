//! Test only (branch uji-siap, never merged): drives the REAL executor thread with real signed transactions against a
//! forked copy of the chain, one scenario per run. Prints one line "SKENARIO <name>: LULUS ..." or fails.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::{Address, Bytes, U256};
use serde_json::{json, Value};

use super::amm::PoolState;
use super::chain::ETH_USDG_POOL_ID;
use super::config::ExecConfig;
use super::sender::Sender;
use super::uniswap::Key;
use super::{pools, start, wallet, ExecHandle, Order, OrderKind};
use crate::net::Rpc;

struct Uji {
    h: ExecHandle,
    dir: PathBuf,
    rpc: Rpc,
    me: Address,
    eth_usd: f64,
}

fn hexu(v: &Value) -> U256 {
    v.as_str().and_then(|s| U256::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(U256::ZERO)
}

fn angka(v: &Value) -> U256 {
    v.as_str().map(|s| if s.starts_with("0x") { hexu(v) } else { U256::from_str_radix(s, 10).unwrap_or(U256::ZERO) }).unwrap_or(U256::ZERO)
}

impl Uji {
    fn snapshot(&self) -> Value {
        std::fs::read_to_string(self.dir.join("positions.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
    }
    fn pos(&self, pool: &str) -> Value {
        self.snapshot()["positions"][pool].clone()
    }
    fn jurnal(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("journal.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
    fn jurnal_n(&self, event: &str) -> usize {
        self.jurnal().iter().filter(|j| j["event"] == event).count()
    }
    fn ditutup(&self, pool: &str) -> Option<Value> {
        std::fs::read_to_string(self.dir.join("closed.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|c| c["pool"] == pool)
    }
    fn tunggu(&self, apa: &str, detik: u64, mut f: impl FnMut(&Uji) -> bool) -> Result<(), String> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(detik) {
            if f(self) {
                println!("   ok {:.1} dtk: {apa}", t0.elapsed().as_secs_f64());
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Err(format!("TIDAK TERCAPAI dalam {detik} dtk: {apa}\nposisi: {}\nexec_status: {}", self.snapshot(), std::fs::read_to_string(self.dir.join("exec_status.json")).unwrap_or_default()))
    }
    fn order(&self, pool: &str, kind: OrderKind, key: Key, state: Option<PoolState>) {
        assert!(self.h.submit(Order { pool: pool.to_string(), kind, key: Some(key), state, eth_usd: self.eth_usd, created: Instant::now() }));
    }
    fn beli(&self, pool: &str, key: Key, state: PoolState) {
        self.order(pool, OrderKind::Buy { size_usd: 13.125, model_equity_usd: 100.0, lp_cap_usd: 197.0 }, key, Some(state));
    }
    fn saldo_token(&self, token: Address) -> Result<U256, String> {
        let data = format!("0x70a08231{:0>64}", format!("{:x}", self.me));
        Ok(hexu(&self.rpc.call("eth_call", json!([{"to": format!("{token:#x}"), "data": data}, "latest"]))?))
    }
    fn saldo_eth(&self) -> Result<U256, String> {
        Ok(hexu(&self.rpc.call("eth_getBalance", json!([format!("{:#x}", self.me), "latest"]))?))
    }
    fn state(&self, pool: &str) -> Result<PoolState, String> {
        pools::read_state(&self.rpc, pool, "latest")
    }
    fn terbuka(&self, pool: &str) -> Result<(), String> {
        self.tunggu("posisi terbuka, token diterima, kedua approval selesai", 60, |u| {
            let p = u.pos(pool);
            p["status"] == "open" && p["approvals_ready"] == true
        })
    }
    fn habis(&self, pool: &str) -> Result<(), String> {
        self.tunggu("posisi ditutup dan diarsipkan", 200, |u| u.ditutup(pool).is_some())
    }
    /// Close the model side too, as the strategy does after an exit.
    fn model_tutup(&self, pool: &str, key: Key) {
        self.order(pool, OrderKind::Closed { reason: "uji".into(), ret: 1.0 }, key, None);
    }
}

fn lulus(nama: &str, u: &Uji, token: Address, pool: &str) -> Result<(), String> {
    let sisa = u.saldo_token(token)?;
    let c = u.ditutup(pool).unwrap_or(Value::Null);
    println!("SKENARIO {nama}: LULUS | saldo token {sisa} | ETH wallet {} wei | catatan {c}", u.saldo_eth()?);
    if !sisa.is_zero() {
        return Err(format!("{nama}: token masih tersisa di wallet: {sisa}"));
    }
    Ok(())
}

/// "pool@init_block" -> the pool key, as the bot gets it from the pool's birth
fn kunci(rpc: &Rpc, p: &str) -> Result<(String, Key), String> {
    let (pool, blk) = p.split_once('@').ok_or("pool@blok")?;
    let pool = pool.to_lowercase();
    let blk: u64 = blk.parse().map_err(|_| "blok")?;
    let mut last = String::new();
    for _ in 0..12 {
        match pools::fetch_key(rpc, &pool, Some(blk)) {
            Ok(k) => return Ok((pool, k)),
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    Err(last)
}

fn siapkan(env_file: Option<&str>, bot_dir: &str, cfg: &crate::cfg::Cfg) -> Result<Uji, String> {
    let ec = ExecConfig::load(env_file, bot_dir, &cfg.rpc_urls, cfg.doh, cfg.max_pos)?;
    let dir = ec.state_dir.clone();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = crate::kill::siapkan(dir.parent().unwrap_or(Path::new(".")));
    let signer = wallet::load_signer(ec.key_file.as_ref().ok_or("EXEC_KEY_FILE")?)?;
    let me = signer.address();
    let rpc = Rpc::new(&ec.rpc_urls.join(","), ec.doh)?;
    let s = pools::read_state(&rpc, ETH_USDG_POOL_ID, "latest")?;
    let eth_usd = s.sqrt_p * s.sqrt_p * 1e12;
    let alert: super::Alert = Arc::new(|m: String| println!("   [alert] {m}"));
    let h = start(ec, alert)?.ok_or("mode bukan live")?;
    Ok(Uji { h, dir, rpc, me, eth_usd })
}

pub fn run(cfg: &crate::cfg::Cfg, env_file: Option<&str>, bot_dir: &str, skenario: &str, pool: &str, pool2: Option<&str>) -> Result<(), String> {
    let u = siapkan(env_file, bot_dir, cfg)?;
    let kunci_rpc = Rpc::new(&std::env::var("E2E_KEY_RPC").map_err(|_| "E2E_KEY_RPC")?, false)?;
    let (pool, key) = kunci(&kunci_rpc, pool)?;
    let token = key.token();
    println!("--- {skenario}: pool {pool} token {token:#x} fee {} hooks {:#x}, ETH ${:.0}, saldo {} wei", key.fee, key.hooks, u.eth_usd, u.saldo_eth()?);
    match skenario {
        // the whole life of a position: buy, both approvals, take profit 60%, exit, model closed
        "normal" => {
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            let bought = angka(&u.pos(&pool)["tokens_bought"]);
            if u.saldo_token(token)? != bought {
                return Err(format!("wallet {} != token tercatat {bought}", u.saldo_token(token)?));
            }
            u.order(&pool, OrderKind::TakeProfit { frac: 0.6 }, key, Some(u.state(&pool)?));
            u.tunggu("take profit 60% terjual", 30, |u| u.pos(&pool)["tp_done"] == true)?;
            let sisa = u.saldo_token(token)?;
            println!("   sisa token setelah TP: {sisa} dari {bought}");
            u.order(&pool, OrderKind::Exit { why: "model bahaya".into() }, key, Some(u.state(&pool)?));
            u.tunggu("posisi habis terjual", 30, |u| u.pos(&pool)["status"] == "closed")?;
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // two positions at once: consecutive nonces (buy, approve, approve, buy, approve, approve)
        "banyak" => {
            let (p2, k2) = kunci(&kunci_rpc, pool2.ok_or("pool2")?)?;
            u.beli(&pool, key, u.state(&pool)?);
            u.beli(&p2, k2, u.state(&p2)?);
            u.terbuka(&pool)?;
            u.terbuka(&p2)?;
            u.order(&pool, OrderKind::Exit { why: "rug terdeteksi".into() }, key, Some(u.state(&pool)?));
            u.order(&p2, OrderKind::Exit { why: "tanda pucuk".into() }, k2, Some(u.state(&p2)?));
            u.tunggu("dua posisi habis", 30, |u| u.pos(&pool)["status"] == "closed" && u.pos(&p2)["status"] == "closed")?;
            u.model_tutup(&pool, key);
            u.model_tutup(&p2, k2);
            u.habis(&pool)?;
            u.habis(&p2)?;
            lulus(skenario, &u, k2.token(), &p2)?;
            lulus(skenario, &u, token, &pool)
        }
        // price ran away between the decision and the buy: min_out not met, the buy reverts, no tokens, only gas lost
        "beli_gagal" => {
            let mut s = u.state(&pool)?;
            let awal = u.saldo_eth()?;
            s.sqrt_p *= 3.0;
            u.beli(&pool, key, s);
            u.tunggu("beli revert tercatat gagal dan diarsipkan", 60, |u| u.ditutup(&pool).is_some())?;
            let c = u.ditutup(&pool).unwrap_or(Value::Null);
            if c["status"] != "buy_failed" {
                return Err(format!("status bukan buy_failed: {c}"));
            }
            println!("   biaya gas beli gagal: {} wei", awal.saturating_sub(u.saldo_eth()?));
            lulus(skenario, &u, token, &pool)
        }
        // the local quote is stale so the first sell reverts: the next try must use a fresh quote and succeed
        "jual_basi" => {
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            let mut s = u.state(&pool)?;
            s.sqrt_p /= 3.0;
            u.order(&pool, OrderKind::Exit { why: "model bahaya".into() }, key, Some(s));
            u.tunggu("jual pertama revert, lalu laku dengan kuotasi baru", 60, |u| u.pos(&pool)["status"] == "closed")?;
            let kirim = u.jurnal_n("sell_sent");
            if kirim < 2 {
                return Err(format!("harusnya minimal 2 kali kirim jual, tercatat {kirim}"));
            }
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // something else used the wallet's nonce: the bot must resync and still buy
        "nonce" => {
            let signer = wallet::load_signer(Path::new(&std::env::var("E2E_KEY_FILE").map_err(|_| "E2E_KEY_FILE")?))?;
            let urls: Vec<String> = cfg.rpc_urls.split(',').map(|s| s.trim().to_string()).collect();
            let mut s = Sender::new(signer, &urls, false)?;
            s.refresh_base_fee()?;
            s.sync_nonce()?;
            let t = s.sign_next(u.me, U256::from(1u8), Bytes::new(), 50_000)?;
            let r = s.broadcast(&t);
            println!("   transaksi lain memakai nonce {}: {r:?}", t.nonce);
            std::thread::sleep(Duration::from_secs(2));
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            println!("   nonce_resynced tercatat: {}", u.jurnal_n("nonce_resynced"));
            u.order(&pool, OrderKind::Exit { why: "model bahaya".into() }, key, Some(u.state(&pool)?));
            u.tunggu("habis", 30, |u| u.pos(&pool)["status"] == "closed")?;
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // a token that stops being sellable (honeypot after the buy): no gas burned forever, sold when it works again
        "tidak_bisa_jual" => {
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            let lama = u.rpc.call("eth_getCode", json!([format!("{token:#x}"), "latest"]))?;
            u.rpc.call("anvil_setCode", json!([format!("{token:#x}"), "0x60006000fd"]))?;
            u.order(&pool, OrderKind::Exit { why: "model bahaya".into() }, key, Some(u.state(&pool)?));
            u.tunggu("posisi jadi stuck", 180, |u| u.pos(&pool)["status"] == "stuck")?;
            let n1 = u.jurnal_n("sell_sent");
            std::thread::sleep(Duration::from_secs(45));
            let n2 = u.jurnal_n("sell_sent");
            println!("   jual dikirim saat koin tak bisa dijual: {n1}, 45 dtk kemudian: {n2} (simulasi gagal: {})", u.jurnal_n("sell_presim_reverted"));
            if n2 != n1 {
                return Err("masih mengirim jual (membakar gas) pada koin yang tidak bisa dijual".into());
            }
            u.rpc.call("anvil_setCode", json!([format!("{token:#x}"), lama]))?;
            u.tunggu("laku lagi setelah koin normal", 60, |u| u.pos(&pool)["status"] == "closed")?;
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // STOP_BUYING blocks buys, an open position can still be sold
        "stop_beli" => {
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            std::fs::write(u.dir.join("STOP_BUYING"), "").map_err(|e| e.to_string())?;
            let (p2, k2) = kunci(&kunci_rpc, pool2.ok_or("pool2")?)?;
            let n = u.jurnal_n("buy_skipped");
            u.beli(&p2, k2, u.state(&p2)?);
            u.tunggu("beli ditolak karena STOP_BUYING", 10, |u| u.jurnal_n("buy_skipped") > n)?;
            u.order(&pool, OrderKind::Exit { why: "model bahaya".into() }, key, Some(u.state(&pool)?));
            u.tunggu("posisi lama tetap bisa dijual", 30, |u| u.pos(&pool)["status"] == "closed")?;
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // FLATTEN_ALL sells everything
        "jual_semua" => {
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            std::fs::write(u.dir.join("FLATTEN_ALL"), "").map_err(|e| e.to_string())?;
            u.tunggu("FLATTEN_ALL menjual posisi", 30, |u| u.pos(&pool)["status"] == "closed")?;
            let n = u.jurnal_n("buy_skipped");
            u.beli(&pool, key, u.state(&pool)?);
            u.tunggu("selama FLATTEN_ALL tidak ada beli baru", 10, |u| u.jurnal_n("buy_skipped") > n)?;
            std::fs::remove_file(u.dir.join("FLATTEN_ALL")).map_err(|e| e.to_string())?;
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // an open position when the kill switch fires: nothing at all may be sent
        "kill_nyala" => {
            u.beli(&pool, key, u.state(&pool)?);
            u.terbuka(&pool)?;
            crate::kill::picu("uji: kesalahan buatan");
            let n = u.jurnal_n("sell_sent");
            let nonce0 = u.rpc.call("eth_getTransactionCount", json!([format!("{:#x}", u.me), "latest"]))?;
            u.order(&pool, OrderKind::Exit { why: "model bahaya".into() }, key, Some(u.state(&pool)?));
            u.beli(&pool, key, u.state(&pool)?);
            std::thread::sleep(Duration::from_secs(10));
            let nonce1 = u.rpc.call("eth_getTransactionCount", json!([format!("{:#x}", u.me), "latest"]))?;
            println!("   nonce {nonce0} -> {nonce1}, sell_sent {n} -> {}", u.jurnal_n("sell_sent"));
            if nonce0 != nonce1 {
                return Err("kill switch aktif tapi ada transaksi terkirim".into());
            }
            println!("SKENARIO {skenario}: LULUS | tidak ada transaksi selama kill switch, posisi tetap terbuka untuk dilanjutkan");
            Ok(())
        }
        // after the owner clears the kill switch and asks to sell everything (what `rhc.sh jual-semua` does)
        "kill_lanjut" => {
            std::fs::write(u.dir.join("FLATTEN_ALL"), "").map_err(|e| e.to_string())?;
            u.tunggu("posisi dijual setelah kill switch dibuka", 40, |u| u.pos(&pool)["status"] == "closed")?;
            u.model_tutup(&pool, key);
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // the process dies right after the buy and both approvals went out, nothing mined yet
        "mati" => {
            u.rpc.call("evm_setAutomine", json!([false]))?;
            u.rpc.call("evm_setIntervalMining", json!([6]))?;
            u.beli(&pool, key, u.state(&pool)?);
            u.tunggu("beli dan 2 approval terkirim, belum ditambang", 20, |u| u.jurnal_n("approve_sent") >= 2)?;
            println!("SKENARIO {skenario}: proses dimatikan paksa sekarang");
            std::process::abort();
        }
        // the next start finds the position in its ledger: pending txs settle, the strategy no longer holds it, sold
        "pulih" => {
            println!("   buku saat mulai: {}", u.snapshot());
            u.h.reconcile(vec![]);
            u.tunggu("posisi lama selesai dijual setelah restart", 90, |u| u.pos(&pool)["status"] == "closed")?;
            u.rpc.call("evm_setAutomine", json!([true]))?;
            u.habis(&pool)?;
            lulus(skenario, &u, token, &pool)
        }
        // the sequencer lost the buy and its approvals: the bot must notice, keep the nonce right, and buy again later
        "tx_hilang" => {
            u.rpc.call("evm_setAutomine", json!([false]))?;
            u.beli(&pool, key, u.state(&pool)?);
            u.tunggu("beli dan 2 approval terkirim", 20, |u| u.jurnal_n("approve_sent") >= 2)?;
            u.rpc.call("anvil_dropAllTransactions", json!([]))?;
            u.tunggu("transaksi hilang dikenali, beli gagal", 120, |u| u.pos(&pool)["status"] == "buy_failed")?;
            u.rpc.call("evm_setAutomine", json!([true]))?;
            u.rpc.call("evm_mine", json!([]))?;
            u.tunggu("posisi gagal diarsipkan", 120, |u| u.ditutup(&pool).is_some())?;
            let txs = u.ditutup(&pool).unwrap_or(Value::Null);
            println!("   approval dikirim ulang untuk beli yang gagal: {}", u.jurnal_n("approve_sent"));
            let _ = txs;
            println!("   catatan: {}", u.ditutup(&pool).unwrap_or(Value::Null));
            let (p2, k2) = kunci(&kunci_rpc, pool2.ok_or("pool2")?)?;
            std::thread::sleep(Duration::from_secs(4));
            u.beli(&p2, k2, u.state(&p2)?);
            u.terbuka(&p2)?;
            u.order(&p2, OrderKind::Exit { why: "model bahaya".into() }, k2, Some(u.state(&p2)?));
            u.tunggu("beli berikutnya normal dan terjual", 30, |u| u.pos(&p2)["status"] == "closed")?;
            u.model_tutup(&p2, k2);
            u.habis(&p2)?;
            lulus(skenario, &u, k2.token(), &p2)
        }
        _ => Err(format!("skenario tidak dikenal: {skenario}")),
    }
}
