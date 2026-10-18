//! PENGUNDUH DATA HISTORIS (untuk uji jendela acak). Hanya baca RPC. Hasil berformat sama dengan collect.js + extract_m1.js
//! + fetch_tr.js + dev_creator_m.js, sehingga replay Rust & `rhc-live sim` membacanya tanpa perubahan.
//!   rhc-live unduh  <dari_blok> <sampai_blok> <dir>   -> pools.csv, swaps_sorted.csv, liq.csv (pool lahir di jendela + ekor 9 jam)
//!   rhc-live menit1 <dir>                              -> swaps_m1.json, transfers.json, dev_creator.json (butuh trades.jsonl)
use crate::live::{dekode, Ev};
use crate::net::Rpc;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const PM: &str = "0x8366a39cc670b4001a1121b8f6a443a643e40951";
const ETH_POOL: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e";
const T_TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const ZERO32: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";
const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const EKOR: u64 = 330000; // 9 jam + sedikit: swap sampai batas pegang koin terakhir

fn hx(b: u64) -> String {
    format!("0x{b:x}")
}

/// panggilan dengan jeda & mundur saat 429 (RPC publik)
pub(crate) fn panggil(rpc: &Rpc, m: &str, p: Value) -> Result<Value, String> {
    let mut tunggu = 1000u64;
    for _ in 0..12 {
        match rpc.call_once(m, p.clone(), Duration::from_secs(90)) {
            Ok(v) => return Ok(v),
            Err(e) if e.starts_with("rpc:") && !e.contains("429") => return Err(e),
            Err(e) => {
                if !e.contains("429") {
                    eprintln!("  coba lagi: {}", e.chars().take(70).collect::<String>());
                }
                std::thread::sleep(Duration::from_millis(tunggu));
                tunggu = (tunggu * 2).min(30000);
            }
        }
    }
    Err("RPC gagal berulang".into())
}
pub(crate) fn batch(rpc: &Rpc, calls: &[(&str, Value)]) -> Result<Vec<Value>, String> {
    // Keep successful items between attempts; under rate limits (429) retry only the failed items,
    // and as a last resort fetch each remaining item alone with `panggil` (its own backoff).
    let mut out: Vec<Option<Value>> = vec![None; calls.len()];
    let mut tunggu = 1000u64;
    for _ in 0..4 {
        let todo: Vec<usize> = (0..calls.len()).filter(|&i| out[i].is_none()).collect();
        if todo.is_empty() {
            break;
        }
        let sub: Vec<(&str, Value)> = todo.iter().map(|&i| (calls[i].0, calls[i].1.clone())).collect();
        if let Ok(v) = rpc.batch(&sub, Duration::from_secs(120)) {
            for (k, r) in v.into_iter().enumerate() {
                if let (Some(&i), Ok(x)) = (todo.get(k), r) {
                    out[i] = Some(x);
                }
            }
        }
        if out.iter().any(|x| x.is_none()) {
            std::thread::sleep(Duration::from_millis(tunggu));
            tunggu = (tunggu * 2).min(30000);
        }
    }
    for i in 0..calls.len() {
        if out[i].is_none() {
            out[i] = Some(if calls[i].0 == "eth_getLogs" { logs_split(rpc, &calls[i].1)? } else { panggil(rpc, calls[i].0, calls[i].1.clone())? });
        }
    }
    Ok(out.into_iter().map(|x| x.unwrap()).collect())
}

/// eth_getLogs that halves the block range when the RPC 10,000-log cap is hit, so no logs are dropped.
fn logs_split(rpc: &Rpc, params: &Value) -> Result<Value, String> {
    match panggil(rpc, "eth_getLogs", params.clone()) {
        Err(e) if e.contains("exceeds limit") => {
            let f = &params[0];
            let num = |k: &str| u64::from_str_radix(f[k].as_str().unwrap_or("0x0").trim_start_matches("0x"), 16).unwrap_or(0);
            let (a, b) = (num("fromBlock"), num("toBlock"));
            if a >= b {
                return Err(e);
            }
            let m = a + (b - a) / 2;
            let mut lo = f.clone();
            lo["toBlock"] = json!(hx(m));
            let mut hi = f.clone();
            hi["fromBlock"] = json!(hx(m + 1));
            let mut v = logs_split(rpc, &json!([lo]))?.as_array().cloned().unwrap_or_default();
            v.extend(logs_split(rpc, &json!([hi]))?.as_array().cloned().unwrap_or_default());
            Ok(Value::Array(v))
        }
        r => r,
    }
}

type Baris = Vec<(String, u64, u64, String)>;
/// satu segmen rentang blok (dijalankan paralel)
fn segmen(rpc: &Rpc, dari: u64, akhir: u64, sampai: u64, t0: Instant) -> Result<(Vec<String>, HashSet<String>, Baris, Baris, u64), String> {
    let mut pools: Vec<String> = Vec::new();
    let mut lahir: HashSet<String> = HashSet::new();
    let mut swaps: Baris = Vec::new();
    let mut liq: Baris = Vec::new();
    let (mut b, mut lebar) = (dari, 3000u64);
    let mut n_req = 0u64;
    while b <= akhir {
        let e = (b + lebar - 1).min(akhir);
        n_req += 1;
        match panggil(rpc, "eth_getLogs", json!([{"address": PM, "fromBlock": hx(b), "toBlock": hx(e)}])) {
            Ok(Value::Array(ls)) => {
                for l in &ls {
                    match dekode(l) {
                        Some(Ev::Init { id, c0, c1, fee, ts, hooks, blk, .. }) => {
                            if (blk as u64) <= sampai {
                                lahir.insert(id.clone());
                                pools.push(format!("{id},{c0},{c1},{fee},{ts},{hooks},{blk}"));
                            }
                        }
                        Some(ev @ Ev::Swap { .. }) => {
                            if let Ev::Swap { ref id, blk, li, .. } = ev {
                                let (f, line) = ev.csv();
                                let _ = f;
                                swaps.push((id.clone(), blk as u64, li as u64, line));
                            }
                        }
                        Some(ev @ Ev::Mod { .. }) => {
                            if let Ev::Mod { ref id, blk, li, .. } = ev {
                                let (_, line) = ev.csv();
                                liq.push((id.clone(), blk as u64, li as u64, line));
                            }
                        }
                        None => {}
                    }
                }
                // rentang adaptif: sasaran ~8.000 log per permintaan (batas RPC 10.000)
                let n = ls.len() as u64;
                lebar = if n < 3000 { (lebar * 2).min(200_000) } else if n > 8000 { (lebar * 2 / 3).max(200) } else { lebar };
                b = e + 1;
                if n_req % 25 == 0 {
                    eprintln!("  segmen {dari}: {:.0}% | {} swap | {} permintaan | {:.0} dtk", 100.0 * (b - dari) as f64 / (akhir - dari + 1) as f64, swaps.len(), n_req, t0.elapsed().as_secs_f64());
                }
            }
            Ok(_) => b = e + 1,
            Err(er) if er.contains("exceeds") || er.contains("limit") => lebar = (lebar / 2).max(50),
            Err(er) => return Err(er),
        }
    }
    Ok((pools, lahir, swaps, liq, n_req))
}

pub fn unduh(rpc: &Rpc, dari: u64, sampai: u64, dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let akhir = sampai + EKOR;
    let jalur: u64 = std::env::var("JALUR").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let pot = (akhir - dari + 1) / jalur + 1;
    let hasil: Vec<Result<(Vec<String>, HashSet<String>, Baris, Baris, u64), String>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..jalur)
            .map(|k| {
                let (a, b) = (dari + k * pot, (dari + (k + 1) * pot - 1).min(akhir));
                sc.spawn(move || segmen(rpc, a, b, sampai, t0))
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or(Err("thread gagal".into()))).collect()
    });
    let (mut pools, mut lahir, mut swaps, mut liq, mut n_req) = (Vec::new(), HashSet::new(), Vec::new(), Vec::new(), 0u64);
    for h in hasil {
        let (p, l, s, q, n) = h?;
        pools.extend(p);
        lahir.extend(l);
        swaps.extend(s);
        liq.extend(q);
        n_req += n;
    }
    // hanya pool yang lahir di jendela (+ pool ETH/USDG untuk harga ETH); urut per pool lalu (blok, log)
    let simpan = |nama: &str, head: &str, mut v: Vec<(String, u64, u64, String)>| -> Result<usize, String> {
        v.retain(|x| lahir.contains(&x.0) || x.0 == ETH_POOL);
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        v.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1 && a.2 == b.2);
        let mut f = std::io::BufWriter::new(std::fs::File::create(Path::new(dir).join(nama)).map_err(|e| e.to_string())?);
        writeln!(f, "{head}").map_err(|e| e.to_string())?;
        for x in &v {
            writeln!(f, "{}", x.3).map_err(|e| e.to_string())?;
        }
        Ok(v.len())
    };
    let ns = simpan("swaps_sorted.csv", "pool_id,block,log_idx,tx_hash,a0,a1,sqrt_price,liquidity,tick,fee", swaps)?;
    let nl = simpan("liq.csv", "pool_id,block,log_idx,tick_lo,tick_hi,liq_delta", liq)?;
    pools.sort();
    pools.dedup();
    let mut f = std::fs::File::create(Path::new(dir).join("pools.csv")).map_err(|e| e.to_string())?;
    writeln!(f, "pool_id,c0,c1,fee,tick_spacing,hooks,init_block").map_err(|e| e.to_string())?;
    for p in &pools {
        writeln!(f, "{p}").map_err(|e| e.to_string())?;
    }
    eprintln!("UNDUH SELESAI {dir}: {} pool lahir, {ns} swap, {nl} likuiditas, {n_req} permintaan, {:.0} dtk", pools.len(), t0.elapsed().as_secs_f64());
    Ok(())
}

/// swap menit-1, transfer token menit-1, dev pembuat — untuk koin yang punya trade v4 di trades.jsonl
pub fn menit1(rpc: &Rpc, dir: &str) -> Result<(), String> {
    let t0 = Instant::now();
    let rd = |n: &str| std::fs::read_to_string(Path::new(dir).join(n)).map_err(|e| format!("{n}: {e}"));
    let mut init: BTreeMap<String, u64> = BTreeMap::new();
    for l in rd("trades.jsonl")?.lines() {
        if let Ok(t) = serde_json::from_str::<Value>(l) {
            if t["plan"] == "v4" {
                init.insert(t["pool"].as_str().unwrap_or("").to_string(), t["init"].as_f64().unwrap_or(0.0) as u64);
            }
        }
    }
    let mut token: HashMap<String, String> = HashMap::new();
    for l in rd("pools.csv")?.lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() >= 3 {
            let (c0, c1) = (p[1].to_lowercase(), p[2].to_lowercase());
            token.insert(p[0].to_lowercase(), if c0 == NATIVE { c1 } else { c0 });
        }
    }
    // swaps_m1.json: [blk, li, a0, a1, sqrtPX96, L, fee]
    let mut m1: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for l in rd("swaps_sorted.csv")?.lines().skip(1) {
        let id = &l[..l.find(',').unwrap_or(0)];
        let Some(&i0) = init.get(id) else { continue };
        let p: Vec<&str> = l.split(',').collect();
        let blk: u64 = p[1].parse().unwrap_or(0);
        if blk > i0 + 600 {
            continue;
        }
        m1.entry(id.to_string()).or_default().push(json!([blk, p[2].parse::<u64>().unwrap_or(0), p[4], p[5], p[6], p[7], p[9].trim().parse::<f64>().unwrap_or(0.0)]));
    }
    std::fs::write(Path::new(dir).join("swaps_m1.json"), serde_json::to_string(&m1).unwrap()).map_err(|e| e.to_string())?;
    // transfers.json + dev_creator.json (20 koin per permintaan batch)
    let daftar: Vec<(String, String, u64)> = init.iter().filter_map(|(p, i)| token.get(p).map(|t| (p.clone(), t.clone(), *i))).collect();
    let mut tf = serde_json::Map::new();
    let mut dev = serde_json::Map::new();
    for (k, part) in daftar.chunks(20).enumerate() {
        let calls: Vec<(&str, Value)> = part.iter().map(|(_, t, i)| ("eth_getLogs", json!([{"address": t, "topics": [T_TRANSFER], "fromBlock": hx(*i), "toBlock": hx(i + 600)}]))).collect();
        let res = batch(rpc, &calls)?;
        for ((p, t, _), r) in part.iter().zip(res.iter()) {
            let tr: Vec<Value> = r
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|l| {
                            let tp = l["topics"].as_array()?;
                            let amt = num_bigint::BigInt::parse_bytes(l["data"].as_str()?.trim_start_matches("0x").as_bytes(), 16).unwrap_or_default();
                            Some(json!([
                                u64::from_str_radix(l["blockNumber"].as_str()?.trim_start_matches("0x"), 16).ok()?,
                                u64::from_str_radix(l["logIndex"].as_str()?.trim_start_matches("0x"), 16).ok()?,
                                format!("0x{}", &tp.get(1)?.as_str()?[26..]),
                                format!("0x{}", &tp.get(2)?.as_str()?[26..]),
                                amt.to_string()
                            ]))
                        })
                        .collect()
                })
                .unwrap_or_default();
            tf.insert(p.clone(), json!({"token": t, "tr": tr}));
        }
        let calls: Vec<(&str, Value)> = part.iter().map(|(_, t, i)| ("eth_getLogs", json!([{"address": t, "topics": [T_TRANSFER, ZERO32], "fromBlock": hx(i.saturating_sub(36000)), "toBlock": hx(i + 600)}]))).collect();
        let mint = batch(rpc, &calls)?;
        let perlu: Vec<(usize, String)> = mint.iter().enumerate().filter_map(|(j, r)| r.as_array()?.first()?["transactionHash"].as_str().map(|h| (j, h.to_string()))).collect();
        let txs = if perlu.is_empty() { Vec::new() } else { batch(rpc, &perlu.iter().map(|(_, h)| ("eth_getTransactionByHash", json!([h]))).collect::<Vec<_>>())? };
        let mut devmap: HashMap<usize, String> = HashMap::new();
        for ((j, _), tx) in perlu.iter().zip(txs.iter()) {
            if let Some(f) = tx["from"].as_str() {
                devmap.insert(*j, f.to_lowercase());
            }
        }
        for (j, (p, _, _)) in part.iter().enumerate() {
            dev.insert(p.clone(), match devmap.get(&j) { Some(d) => json!({"dev": d, "why": "ok"}), None => json!({"dev": null, "why": "tanpa mint"}) });
        }
        if k % 10 == 0 {
            eprintln!("  menit-1: {}/{} koin | {:.0} dtk", (k + 1) * 20, daftar.len(), t0.elapsed().as_secs_f64());
        }
    }
    std::fs::write(Path::new(dir).join("transfers.json"), Value::Object(tf).to_string()).map_err(|e| e.to_string())?;
    std::fs::write(Path::new(dir).join("dev_creator.json"), Value::Object(dev).to_string()).map_err(|e| e.to_string())?;
    eprintln!("MENIT-1 SELESAI {dir}: {} koin, {:.0} dtk", daftar.len(), t0.elapsed().as_secs_f64());
    Ok(())
}
