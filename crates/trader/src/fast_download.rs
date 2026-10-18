//! FAST DOWNLOAD — same output files as `unduh` (pools.csv, swaps_sorted.csv, liq.csv), far fewer RPC requests.
//!   rhc-live fast <from_block> <to_block> <dir>
//! Phase 1: Initialize events only (few logs -> wide block ranges).
//! Phase 2: Swap + ModifyLiquidity only for TRADABLE pools (`safe()`: ETH pair, fee <= 0.5%, beforeSwap hook off)
//!          plus the ETH/USDG price pool, filtered by pool id in topic[1].
//! Pools failing `safe()` are never traded by the engine, so trades are identical to a full download;
//! pools.csv still lists every pool born in the window.
use crate::live::{dekode, Ev};
use crate::net::Rpc;
use crate::unduh::panggil;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const PM: &str = "0x8366a39cc670b4001a1121b8f6a443a643e40951";
const ETH_POOL: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e";
const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const T_INIT: &str = "0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438";
const T_SWAP: &str = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f";
const T_MOD: &str = "0xf208f4912782fd25c7f114ca3723a2d5dd6f3bcc3ac8db5af63baa85f711d5ec";
const TAIL: u64 = 330000; // 9 h hold limit + margin, same as `unduh`
const IDS_PER_REQ: usize = 40;

type Row = (String, u64, u64, String);

fn hx(b: u64) -> String {
    format!("0x{b:x}")
}

const WETH: &str = "0x0bd7d308f8e1639fab988df18a8011f41eacad73";

fn base_rule(c0: &str, c1: &str, fee: f64, hook_ok: bool, quote_weth: bool, max_fee: f64) -> bool {
    let quote = c0 == NATIVE || c1 == NATIVE || (quote_weth && (c0 == WETH || c1 == WETH));
    quote && fee <= max_fee && hook_ok
}

/// Same rule as port_engine::engine::safe() by default.
/// EXT_ONLY=1: only the EXTRA pools of the widened universe (WETH as quote, fee <= 1%), not the base pools.
fn tradable(c0: &str, c1: &str, fee: f64, hooks: &str) -> bool {
    let h = hooks.trim_start_matches("0x");
    let low = u8::from_str_radix(&h[h.len().saturating_sub(2)..], 16).unwrap_or(0xff);
    let hook_ok = (low >> 7) & 1 == 0;
    let base = base_rule(c0, c1, fee, hook_ok, false, 5000.0);
    if std::env::var("EXT_ONLY").map_or(false, |v| v == "1") {
        return !base && base_rule(c0, c1, fee, hook_ok, true, 10000.0);
    }
    base
}

/// All logs matching `topics` in [a, b], adaptive range width (RPC cap: 10,000 logs per response).
fn fetch(rpc: &Rpc, a: u64, b: u64, topics: &Value, start_width: u64, max_width: u64, n_req: &AtomicUsize) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let (mut s, mut width) = (a, start_width);
    while s <= b {
        let e = (s + width - 1).min(b);
        n_req.fetch_add(1, Ordering::Relaxed);
        match panggil(rpc, "eth_getLogs", json!([{"address": PM, "topics": topics, "fromBlock": hx(s), "toBlock": hx(e)}])) {
            Ok(Value::Array(ls)) => {
                let n = ls.len() as u64;
                out.extend(ls);
                width = if n < 3000 { (width * 2).min(max_width) } else if n > 8000 { (width * 2 / 3).max(50) } else { width };
                s = e + 1;
            }
            Ok(_) => s = e + 1,
            Err(er) if er.contains("exceeds") || er.contains("limit") || er.contains("allowed") => width = (width / 2).max(50),
            Err(er) => return Err(er),
        }
    }
    Ok(out)
}

/// Run jobs on `lanes` threads, collecting results.
fn parallel<T: Send, J: Sync>(jobs: &[J], lanes: usize, f: impl Fn(&J) -> Result<T, String> + Sync) -> Result<Vec<T>, String> {
    let next = AtomicUsize::new(0);
    let res: Mutex<Vec<(usize, Result<T, String>)>> = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..lanes {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= jobs.len() {
                    break;
                }
                let r = f(&jobs[i]);
                res.lock().unwrap().push((i, r));
            });
        }
    });
    let mut v = res.into_inner().unwrap();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

pub fn download_fast(rpc: &Rpc, from: u64, to: u64, dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let lanes: usize = std::env::var("JALUR").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let n_req = AtomicUsize::new(0);

    // phase 1: pool births
    let step = ((to - from + 1) / lanes as u64).max(1);
    let spans: Vec<(u64, u64)> = (0..lanes as u64).map(|k| (from + k * step, if k + 1 == lanes as u64 { to } else { from + (k + 1) * step - 1 })).filter(|(a, b)| a <= b).collect();
    let init_topics = json!([T_INIT]);
    let logs = parallel(&spans, lanes, |&(a, b)| fetch(rpc, a, b, &init_topics, 200_000, 10_000_000, &n_req))?;
    let mut pools: Vec<String> = Vec::new();
    let mut births: Vec<(u64, String)> = Vec::new();
    for l in logs.iter().flatten() {
        if let Some(Ev::Init { id, c0, c1, fee, ts, hooks, blk, .. }) = dekode(l) {
            pools.push(format!("{id},{c0},{c1},{fee},{ts},{hooks},{blk}"));
            if tradable(&c0, &c1, fee, &hooks) {
                births.push((blk as u64, id));
            }
        }
    }
    births.sort();
    eprintln!("phase 1: {} pools born, {} tradable | {} requests | {:.0} s", pools.len(), births.len(), n_req.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());

    // phase 2: swaps + liquidity for tradable pools (chunks of ids, time-local) + ETH price pool
    let mut jobs: Vec<(u64, u64, Vec<String>)> = births
        .chunks(IDS_PER_REQ)
        .map(|c| (c.first().unwrap().0, (c.last().unwrap().0 + TAIL).min(to + TAIL), c.iter().map(|x| x.1.clone()).collect()))
        .collect();
    let eth_step = ((to + TAIL - from) / lanes as u64).max(1);
    for k in 0..lanes as u64 {
        let a = from + k * eth_step;
        let b = if k + 1 == lanes as u64 { to + TAIL } else { from + (k + 1) * eth_step - 1 };
        jobs.push((a, b, vec![ETH_POOL.to_string()]));
    }
    let done = AtomicUsize::new(0);
    let total = jobs.len();
    let parts = parallel(&jobs, lanes, |(a, b, ids)| {
        let topics = json!([[T_SWAP, T_MOD], ids]);
        // RPC allows at most 100,000 blocks per request when topics hold value lists
        let r = fetch(rpc, *a, *b, &topics, 20_000, 100_000, &n_req);
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % 10 == 0 || d == total {
            eprintln!("  phase 2: {d}/{total} chunks | {} requests | {:.0} s", n_req.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());
        }
        r
    })?;
    let keep: HashSet<&str> = births.iter().map(|x| x.1.as_str()).chain(std::iter::once(ETH_POOL)).collect();
    let (mut swaps, mut liq): (Vec<Row>, Vec<Row>) = (Vec::new(), Vec::new());
    for l in parts.iter().flatten() {
        match dekode(l) {
            Some(ev @ Ev::Swap { .. }) => {
                if let Ev::Swap { ref id, blk, li, .. } = ev {
                    if keep.contains(id.as_str()) {
                        swaps.push((id.clone(), blk as u64, li as u64, ev.csv().1));
                    }
                }
            }
            Some(ev @ Ev::Mod { .. }) => {
                if let Ev::Mod { ref id, blk, li, .. } = ev {
                    if keep.contains(id.as_str()) {
                        liq.push((id.clone(), blk as u64, li as u64, ev.csv().1));
                    }
                }
            }
            _ => {}
        }
    }
    let save = |name: &str, head: &str, mut v: Vec<Row>| -> Result<usize, String> {
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        v.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1 && a.2 == b.2);
        let mut f = std::io::BufWriter::new(std::fs::File::create(Path::new(dir).join(name)).map_err(|e| e.to_string())?);
        writeln!(f, "{head}").map_err(|e| e.to_string())?;
        for x in &v {
            writeln!(f, "{}", x.3).map_err(|e| e.to_string())?;
        }
        Ok(v.len())
    };
    let ns = save("swaps_sorted.csv", "pool_id,block,log_idx,tx_hash,a0,a1,sqrt_price,liquidity,tick,fee", swaps)?;
    let nl = save("liq.csv", "pool_id,block,log_idx,tick_lo,tick_hi,liq_delta", liq)?;
    pools.sort();
    pools.dedup();
    let mut f = std::fs::File::create(Path::new(dir).join("pools.csv")).map_err(|e| e.to_string())?;
    writeln!(f, "pool_id,c0,c1,fee,tick_spacing,hooks,init_block").map_err(|e| e.to_string())?;
    for p in &pools {
        writeln!(f, "{p}").map_err(|e| e.to_string())?;
    }
    eprintln!("FAST DOWNLOAD DONE {dir}: {} pools ({} tradable), {ns} swaps, {nl} liquidity, {} requests, {:.0} s", pools.len(), births.len(), n_req.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// FAST MINUTE-1 DATA — same files as `menit1` (swaps_m1.json, transfers.json, dev_creator.json).
//   rhc-live fast1 <dir>
// One eth_getLogs carries a list of up to TOKENS_PER_REQ token addresses (time-local groups), instead of
// one request per coin. Logs are filtered back to each coin's own block window, so content is identical.
const T_TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const ZERO32: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";
const TOKENS_PER_REQ: usize = 40;
const GROUP_SPAN: u64 = 20_000; // max init-block spread inside one group
const DEV_LOOKBACK: u64 = 36_000; // mint search starts 1 h before pool birth (same as menit1)

fn hexu(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.trim_start_matches("0x"), 16).ok()
}

/// eth_getLogs for many addresses; on the 10,000-log cap split the address list, then the block range.
fn logs_multi(rpc: &Rpc, addrs: &[String], topics: &Value, a: u64, b: u64, n_req: &AtomicUsize) -> Result<Vec<Value>, String> {
    n_req.fetch_add(1, Ordering::Relaxed);
    match panggil(rpc, "eth_getLogs", json!([{"address": addrs, "topics": topics, "fromBlock": hx(a), "toBlock": hx(b)}])) {
        Ok(Value::Array(v)) => Ok(v),
        Ok(_) => Ok(Vec::new()),
        Err(e) if e.contains("exceeds") || e.contains("limit") || e.contains("allowed") => {
            if addrs.len() > 1 {
                let (x, y) = addrs.split_at(addrs.len() / 2);
                let mut v = logs_multi(rpc, x, topics, a, b, n_req)?;
                v.extend(logs_multi(rpc, y, topics, a, b, n_req)?);
                Ok(v)
            } else if a < b {
                let m = a + (b - a) / 2;
                let mut v = logs_multi(rpc, addrs, topics, a, m, n_req)?;
                v.extend(logs_multi(rpc, addrs, topics, m + 1, b, n_req)?);
                Ok(v)
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

/// Same row shape as menit1: [blk, li, from, to, amount]
fn transfer_row(l: &Value) -> Option<Value> {
    let tp = l["topics"].as_array()?;
    let amt = num_bigint::BigInt::parse_bytes(l["data"].as_str()?.trim_start_matches("0x").as_bytes(), 16).unwrap_or_default();
    Some(json!([
        hexu(&l["blockNumber"])?,
        hexu(&l["logIndex"])?,
        format!("0x{}", &tp.get(1)?.as_str()?[26..]),
        format!("0x{}", &tp.get(2)?.as_str()?[26..]),
        amt.to_string()
    ]))
}

pub fn minute1_fast(rpc: &Rpc, dir: &str) -> Result<(), String> {
    use std::collections::{BTreeMap, HashMap};
    let t0 = Instant::now();
    let lanes: usize = std::env::var("JALUR").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
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
    // swaps_m1.json — identical to menit1 (local data, no RPC)
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

    // coins sorted by birth, grouped time-locally
    let mut list: Vec<(String, String, u64)> = init.iter().filter_map(|(p, i)| token.get(p).map(|t| (p.clone(), t.clone(), *i))).collect();
    list.sort_by_key(|x| x.2);
    let mut groups: Vec<Vec<(String, String, u64)>> = Vec::new();
    for c in list.iter().cloned() {
        match groups.last_mut() {
            Some(g) if g.len() < TOKENS_PER_REQ && c.2 - g[0].2 <= GROUP_SPAN => g.push(c),
            _ => groups.push(vec![c]),
        }
    }
    let n_req = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    type Out = Vec<(String, Value, Option<String>)>; // (pool, transfers entry, first-mint tx hash)
    let parts: Vec<Out> = parallel(&groups, lanes, |g| {
        let mut addrs: Vec<String> = g.iter().map(|x| x.1.clone()).collect();
        addrs.sort();
        addrs.dedup();
        let (lo, hi) = (g.first().unwrap().2, g.last().unwrap().2 + 600);
        let tr = logs_multi(rpc, &addrs, &json!([T_TRANSFER]), lo, hi, &n_req)?;
        let mint = logs_multi(rpc, &addrs, &json!([T_TRANSFER, ZERO32]), lo.saturating_sub(DEV_LOOKBACK), hi, &n_req)?;
        let mut out = Vec::new();
        for (pool, tok, i) in g {
            let mine = |l: &&Value| l["address"].as_str().map(|a| a.to_lowercase()) == Some(tok.clone());
            let in_win = |l: &&Value, a: u64, b: u64| hexu(&l["blockNumber"]).map_or(false, |x| x >= a && x <= b);
            let mut rows: Vec<(u64, u64, Value)> = tr
                .iter()
                .filter(|l| mine(l) && in_win(l, *i, i + 600))
                .filter_map(|l| Some((hexu(&l["blockNumber"])?, hexu(&l["logIndex"])?, transfer_row(l)?)))
                .collect();
            rows.sort_by_key(|x| (x.0, x.1));
            let first_mint = mint
                .iter()
                .filter(|l| mine(l) && in_win(l, i.saturating_sub(DEV_LOOKBACK), i + 600))
                .min_by_key(|l| (hexu(&l["blockNumber"]).unwrap_or(u64::MAX), hexu(&l["logIndex"]).unwrap_or(u64::MAX)))
                .and_then(|l| l["transactionHash"].as_str().map(|s| s.to_string()));
            out.push((pool.clone(), json!({"token": tok, "tr": rows.into_iter().map(|x| x.2).collect::<Vec<_>>()}), first_mint));
        }
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % 20 == 0 {
            eprintln!("  minute-1: {d}/{} groups | {} requests | {:.0} s", groups.len(), n_req.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());
        }
        Ok(out)
    })?;
    let all: Vec<(String, Value, Option<String>)> = parts.into_iter().flatten().collect();
    // dev = sender of the first mint transaction (batched lookups)
    let hashes: Vec<String> = all.iter().filter_map(|x| x.2.clone()).collect();
    let mut from_of: HashMap<String, String> = HashMap::new();
    let chunks: Vec<&[String]> = hashes.chunks(20).collect();
    let got = parallel(&chunks, lanes, |c| {
        let calls: Vec<(&str, Value)> = c.iter().map(|h| ("eth_getTransactionByHash", json!([h]))).collect();
        crate::unduh::batch(rpc, &calls).map(|v| c.iter().cloned().zip(v).collect::<Vec<_>>())
    })?;
    for (h, tx) in got.into_iter().flatten() {
        if let Some(f) = tx["from"].as_str() {
            from_of.insert(h, f.to_lowercase());
        }
    }
    let mut tf = serde_json::Map::new();
    let mut dev = serde_json::Map::new();
    for (pool, entry, h) in all {
        tf.insert(pool.clone(), entry);
        dev.insert(pool, match h.and_then(|h| from_of.get(&h).cloned()) { Some(d) => json!({"dev": d, "why": "ok"}), None => json!({"dev": null, "why": "tanpa mint"}) });
    }
    std::fs::write(Path::new(dir).join("transfers.json"), Value::Object(tf).to_string()).map_err(|e| e.to_string())?;
    std::fs::write(Path::new(dir).join("dev_creator.json"), Value::Object(dev).to_string()).map_err(|e| e.to_string())?;
    eprintln!("MINUTE-1 DONE {dir}: {} coins, {} requests, {:.0} s", list.len(), n_req.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());
    Ok(())
}
