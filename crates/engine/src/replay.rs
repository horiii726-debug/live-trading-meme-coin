//! Port 1:1 dari `rhc_vps/pipeline/replay.js`: putar ulang data historis lewat PoolSim blok demi blok.
//!
//! Pakai (sama dengan JS):  replay <dir> <fromInit> <endBlock> <outTrades.jsonl> [ethUsd|auto]
//! Env (sama dengan JS): AUDIT (JSON), DANGER_FILE, DANGER_TH, UP_FILE, UP_TH, TP_UP.
//! File relatif terhadap direktori kerja (sama dengan JS): eth_usd.json, danger_model.json, danger_check.json.
//! Env tambahan (khusus Rust, tidak mengubah hasil):
//!   REPLAY_THREADS=N  jumlah thread simulasi (bawaan 1). Tiap pool independen; hasil ditulis
//!                     menurut urutan pool di file swap, jadi keluaran identik untuk N berapa pun.
//!   REPLAY_PROF=1     cetak waktu per tahap ke stderr.

use crate::engine::{safe, PoolMeta, PoolSim, SimOpts, Swap, Thresholds, HZ};
use crate::jsnum::{self, js_max, js_min, to_number};
use crate::model::{to_num, truthy, Model};
use num_bigint::BigInt;
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::{mpsc, Arc, Mutex};

pub const ETH_POOL: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e";
const NL: u8 = 0x0A;
const CR: u8 = 0x0D;
/// Math.pow(2, 96) — pangkat dua, eksak
pub const Q96: f64 = 79228162514264337593543950336.0;

/// Pemecah baris seperti readline Node (crlfDelay: Infinity): pemisah \r\n, \n, atau \r.
pub struct Lines<'a> {
    s: &'a str,
    pos: usize,
}
impl<'a> Lines<'a> {
    pub fn new(s: &'a str) -> Self {
        Lines { s, pos: 0 }
    }
}
impl<'a> Iterator for Lines<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        let b = self.s.as_bytes();
        if self.pos >= b.len() {
            return None;
        }
        let start = self.pos;
        let mut i = match memchr::memchr2(NL, CR, &b[start..]) {
            Some(k) => start + k,
            None => b.len(),
        };
        let line = &self.s[start..i];
        if i < b.len() {
            if b[i] == CR && i + 1 < b.len() && b[i + 1] == NL {
                i += 2;
            } else {
                i += 1;
            }
        }
        self.pos = i;
        Some(line)
    }
}

/// Opsi AUDIT (JSON) dengan semantik JS: `AUD.x ?? d`, truthiness, dsb.
pub struct Audit {
    v: Value,
}
impl Audit {
    pub fn parse(s: &str) -> Result<Audit, String> {
        let v: Value = serde_json::from_str(s).map_err(|e| format!("AUDIT bukan JSON sah: {}", e))?;
        Ok(Audit { v })
    }
    fn get(&self, k: &str) -> Option<&Value> {
        self.v.get(k)
    }
    pub fn truthy(&self, k: &str) -> bool {
        self.get(k).map_or(false, truthy)
    }
    /// angka dengan semantik `??`: tidak ada / null -> None. String ditolak (di JS string akan
    /// disambung, bukan dijumlah — perilaku itu sengaja tidak ditiru).
    pub fn num(&self, k: &str) -> Result<Option<f64>, String> {
        match self.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(_)) => Err(format!("AUDIT.{} harus angka JSON, bukan string", k)),
            Some(v) => Ok(Some(to_num(v))),
        }
    }
    /// `AUD.x || d` untuk angka
    pub fn num_or(&self, k: &str, d: f64) -> Result<f64, String> {
        if self.truthy(k) {
            Ok(self.num(k)?.unwrap_or(d))
        } else {
            Ok(d)
        }
    }
}

struct LiqEv {
    blk: f64,
    li: f64,
    d: BigInt,
}

/// pembanding sort JS `(a, b) => a.blk - b.blk || a.li - b.li` (NaN -> dianggap sama)
fn js_cmp(ab: f64, al: f64, bb: f64, bl: f64) -> Ordering {
    let c = ab - bb;
    let c = if c != 0.0 && !c.is_nan() { c } else { al - bl };
    if c < 0.0 {
        Ordering::Less
    } else if c > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

fn field<'a>(p: &[&'a str], k: usize, what: &str) -> Result<&'a str, String> {
    p.get(k).copied().ok_or_else(|| format!("TypeError: kolom {} hilang ({})", k, what))
}

/// `+p[k]` dengan p[k] mungkin undefined (-> NaN)
fn numf(p: &[&str], k: usize) -> f64 {
    p.get(k).map_or(f64::NAN, |s| to_number(s))
}

/// line.split(",") tanpa alokasi; kolom ke-13 dst. tidak dipakai (kolom terpakai: 0..=9)
fn split_fields<'a>(line: &'a str, f: &mut [&'a str; 12]) -> usize {
    let mut n = 0;
    for part in line.split(',') {
        if n == f.len() {
            break;
        }
        f[n] = part;
        n += 1;
    }
    n
}

/// toLowerCase() tanpa alokasi kalau sudah huruf kecil ASCII
fn lower(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase() || b >= 0x80) {
        std::borrow::Cow::Owned(s.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

pub struct ReplayCfg {
    pub dir: String,
    pub from_init: f64,
    pub end_blk: f64,
    pub out: String,
    pub eth_mode: String,
    pub threads: usize,
}

pub struct Summary {
    pub n_pools: u64,
    pub n_trades: u64,
    pub n_snap: u64,
}

/// Data baca-saja yang dipakai runPool (dibagi antar-thread).
struct Ctx {
    model: Arc<Model>,
    model_up: Option<Arc<Model>>,
    th: Thresholds,
    aud: Audit,
    fixed_eth: f64,
    auto: bool,
    eth_series: Vec<(f64, f64)>,
    /// sudah diurutkan (blk, li) — JS mengurutkan di runPool; hasil urut sama karena sort stabil & deterministik
    liq: HashMap<String, Vec<LiqEv>>,
    end_blk: f64,
}

struct PoolOut {
    jsonl: String,
    n_trades: u64,
    n_snap: u64,
}

impl Ctx {
    fn eth_at(&self, blk: f64) -> f64 {
        let s = &self.eth_series;
        if s.is_empty() {
            return self.fixed_eth;
        }
        let (mut lo, mut hi) = (0i64, s.len() as i64 - 1);
        let mut ans = s[0].1;
        while lo <= hi {
            let mid = (lo + hi) >> 1;
            if s[mid as usize].0 <= blk {
                ans = s[mid as usize].1;
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        ans
    }

    /// runPool(m, sw) di replay.js
    fn run_pool(&self, m: &PoolMeta, mut sw: Vec<Swap>) -> Result<PoolOut, String> {
        sw.sort_by(|a, b| js_cmp(a.blk, a.li, b.blk, b.li));
        let empty: Vec<LiqEv> = Vec::new();
        let lq: &Vec<LiqEv> = self.liq.get(&m.id).unwrap_or(&empty);
        let aud = &self.aud;
        let hz = if aud.truthy("hz") {
            aud.num("hz")?.unwrap_or(HZ)
        } else if aud.truthy("noCap") {
            self.end_blk - m.init
        } else {
            HZ
        };
        let opts = SimOpts {
            model_up: self.model_up.clone(),
            eth_usd: if self.auto { self.eth_at(m.init) } else { self.fixed_eth },
            model: Some(self.model.clone()),
            th: self.th,
            latb: Some(aud.num("latb")?.unwrap_or(30.0)),
            strict_snap: aud.truthy("strictSnap"),
            entry_lat: aud.num("entryLat")?.unwrap_or(0.0),
            keep_no_after: aud.truthy("keepNoAfter"),
            lp_adjust: aud.truthy("lpAdjust"),
            gas_usd: aud.num("gasUsd")?,
            approve_usd: aud.num("approveUsd")?,
            fail_buy: aud.num("failBuy")?,
            fail_sell: aud.num("failSell")?,
            worst_order: aud.truthy("worstOrder"),
            hz: Some(hz),
            stale_min: aud.num("staleMin")?.unwrap_or(0.0),
            quiet_min: aud.num("quietMin")?.unwrap_or(0.0),
            fill_on_time: aud.truthy("fillOnTime"),
            quiet_all: aud.truthy("quietAll"),
            mom_min: aud.num("momMin")?.unwrap_or(0.0),
            mom_hold: aud.truthy("momHold"),
            ..SimOpts::default()
        };
        let mut sim = PoolSim::new(m.clone(), opts)?;
        let (mut i, mut j) = (0usize, 0usize);
        let lim = m.init + hz;
        let mut l: Vec<BigInt> = Vec::new();
        while i < sw.len() || j < lq.len() {
            let bi = if i < sw.len() { sw[i].blk } else { f64::INFINITY };
            let bj = if j < lq.len() { lq[j].blk } else { f64::INFINITY };
            let blk = js_min(bi, bj);
            if blk > lim {
                break;
            }
            if blk.is_nan() {
                // JS akan berputar tanpa henti di sini; port memilih berhenti dengan galat
                return Err(format!("blok NaN di pool {}", m.id));
            }
            let mut d = sim.next_deadline();
            let mut g = 0;
            while d < blk && d <= lim && g < 50 {
                sim.tick(d);
                d = sim.next_deadline();
                g += 1;
            }
            l.clear();
            while j < lq.len() && lq[j].blk == blk {
                l.push(lq[j].d.clone());
                j += 1;
            }
            let s0 = i;
            while i < sw.len() && sw[i].blk == blk {
                i += 1;
            }
            sim.process_block(blk, &l, &sw[s0..i]);
            if sim.all_done() {
                break;
            }
        }
        let mut d = sim.next_deadline();
        let mut g = 0;
        while d <= lim && !sim.all_done() && g < 50 {
            sim.tick(d);
            d = sim.next_deadline();
            g += 1;
        }
        sim.finalize();
        // trade bisa muncul saat processBlock/tick/finalize; urutan antrean = urutan onTrade di JS
        let mut jsonl = String::new();
        let n_trades = sim.trades.len() as u64;
        for t in sim.trades.drain(..) {
            jsonl.push_str(&t.to_json());
            jsonl.push('\n');
        }
        Ok(PoolOut { jsonl, n_trades, n_snap: sim.n_snap })
    }
}

pub fn run(cfg: &ReplayCfg) -> Result<Summary, String> {
    let prof = std::env::var("REPLAY_PROF").map_or(false, |v| v == "1");
    let t0 = std::time::Instant::now();
    let fixed_eth = {
        let t = std::fs::read_to_string("eth_usd.json").map_err(|e| format!("baca eth_usd.json: {}", e))?;
        let v: Value = serde_json::from_str(&t).map_err(|e| format!("eth_usd.json: {}", e))?;
        v.get("ethUsd").map_or(f64::NAN, to_num)
    };
    let danger_file = std::env::var("DANGER_FILE").ok().filter(|s| !s.is_empty());
    let model = Arc::new(Model::load(danger_file.as_deref().unwrap_or("danger_model.json"))?);
    let model_up = match std::env::var("UP_FILE").ok().filter(|s| !s.is_empty()) {
        Some(f) => Some(Arc::new(Model::load(&f)?)),
        None => None,
    };
    let aud = Audit::parse(&std::env::var("AUDIT").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "{}".to_string()))?;
    if aud.truthy("pdMode") && aud.get("pdMode").and_then(|v| v.as_str()) == Some("random") {
        return Err("pdMode \"random\" memakai Math.random (tidak deterministik) — tidak didukung port Rust".into());
    }
    let th = Thresholds::from_env();

    // cek implementasi pohon == sklearn (hanya kalau DANGER_FILE tidak diisi, sama dengan JS)
    if danger_file.is_none() {
        let t = std::fs::read_to_string("danger_check.json").map_err(|e| format!("baca danger_check.json: {}", e))?;
        let ck: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
        let xs = ck.get("X").and_then(|v| v.as_array()).ok_or("danger_check.json tanpa X")?;
        let ps = ck.get("p").and_then(|v| v.as_array()).ok_or("danger_check.json tanpa p")?;
        let mut mx = 0.0f64;
        for (k, x) in xs.iter().enumerate() {
            let xv: Vec<f64> = x
                .as_array()
                .map(|a| a.iter().map(|e| if e.is_null() { f64::NAN } else { to_num(e) }).collect())
                .unwrap_or_default();
            let pk = ps.get(k).map_or(f64::NAN, to_num);
            mx = js_max(mx, (model.predict(&xv) - pk).abs());
        }
        eprintln!("cek model JS vs sklearn (2000 sampel): selisih maks {}", jsnum::to_exponential(mx, 2));
        if mx > 1e-9 {
            return Err("MODEL JS TIDAK SAMA. Berhenti.".into());
        }
    }

    let aud_hz_or = aud.num_or("hz", HZ)?;
    let mut pools: HashMap<String, PoolMeta> = HashMap::new();
    {
        let txt = std::fs::read_to_string(format!("{}/pools.csv", cfg.dir)).map_err(|e| format!("pools.csv: {}", e))?;
        for l in jsnum::js_trim(&txt).split('\n').skip(1) {
            let p: Vec<&str> = jsnum::js_trim(l).split(',').collect();
            let m = PoolMeta {
                id: field(&p, 0, "pools")?.to_lowercase(),
                c0: field(&p, 1, "pools")?.to_lowercase(),
                c1: field(&p, 2, "pools")?.to_lowercase(),
                fee: numf(&p, 3),
                hooks: field(&p, 5, "pools")?.to_lowercase(),
                init: numf(&p, 6),
            };
            if safe(&m)? && m.init >= cfg.from_init && m.init + aud_hz_or <= cfg.end_blk {
                pools.insert(m.id.clone(), m);
            }
        }
    }
    let mut liq: HashMap<String, Vec<LiqEv>> = HashMap::new();
    {
        let txt = std::fs::read_to_string(format!("{}/liq.csv", cfg.dir)).map_err(|e| format!("liq.csv: {}", e))?;
        for line in jsnum::js_trim(&txt).split('\n').skip(1) {
            let p: Vec<&str> = jsnum::js_trim(line).split(',').collect();
            let pid = p[0].to_lowercase();
            if !pools.contains_key(&pid) {
                continue;
            }
            let d = crate::engine::parse_bigint(field(&p, 5, "liq")?)?;
            liq.entry(pid).or_default().push(LiqEv { blk: numf(&p, 1), li: numf(&p, 2), d });
        }
    }
    for v in liq.values_mut() {
        v.sort_by(|a, b| js_cmp(a.blk, a.li, b.blk, b.li));
    }
    eprintln!("pool lolos saringan & horizon lengkap: {}", pools.len());
    if prof {
        eprintln!("[prof] model+pools+liq: {:?}", t0.elapsed());
    }

    let sorted = format!("{}/swaps_sorted.csv", cfg.dir);
    let swap_file = if std::path::Path::new(&sorted).exists() { sorted } else { format!("{}/swaps.csv", cfg.dir) };
    let out_f = std::fs::File::create(&cfg.out).map_err(|e| format!("buat {}: {}", cfg.out, e))?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, out_f);

    let raw = std::fs::read(&swap_file).map_err(|e| format!("baca {}: {}", swap_file, e))?;
    let content = String::from_utf8_lossy(&raw);

    // harga ETH historis (mode auto)
    let auto = cfg.eth_mode == "auto";
    let mut eth_series: Vec<(f64, f64)> = Vec::new();
    if auto {
        // baris yang diawali ETH_POOL (line.startsWith di JS); dicari dengan memmem lalu dicek awal baris
        let bytes = content.as_bytes();
        for pos in memchr::memmem::find_iter(bytes, ETH_POOL.as_bytes()) {
            if pos != 0 && bytes[pos - 1] != NL && bytes[pos - 1] != CR {
                continue;
            }
            let end = memchr::memchr2(NL, CR, &bytes[pos..]).map_or(bytes.len(), |k| pos + k);
            let line = &content[pos..end];
            let mut f = [""; 12];
            let nf = split_fields(line, &mut f);
            let p = &f[..nf];
            let sp = numf(p, 6) / Q96;
            eth_series.push((numf(p, 1), sp * sp * 1e12));
        }
        // sort stabil, pembanding a[0] - b[0]
        eth_series.sort_by(|a, b| js_cmp(a.0, 0.0, b.0, 0.0));
        eprintln!("harga ETH historis: {} titik", eth_series.len());
    }
    if prof {
        eprintln!("[prof] baca swap + seri ETH: {:?}", t0.elapsed());
    }

    let ctx = Ctx { model, model_up, th, aud, fixed_eth, auto, eth_series, liq, end_blk: cfg.end_blk };
    let mut sum = Summary { n_pools: 0, n_trades: 0, n_snap: 0 };

    // Pemindai baris: memanggil `emit(meta, swaps)` setiap kali id pool berganti (sama dengan loop readline JS).
    let scan = |emit: &mut dyn FnMut(&PoolMeta, Vec<Swap>) -> Result<(), String>| -> Result<(), String> {
        let mut cur: Option<String> = None;
        let mut sw: Vec<Swap> = Vec::new();
        for line in Lines::new(&content) {
            if line.is_empty() {
                continue;
            }
            let pid_raw = match line.find(',') {
                Some(c) => &line[..c],
                None => {
                    // slice(0, -1)
                    let mut it = line.char_indices();
                    match it.next_back() {
                        Some((k, _)) => &line[..k],
                        None => "",
                    }
                }
            };
            let pid = lower(pid_raw);
            if pid == "pool_id" {
                continue;
            }
            if cur.as_deref() != Some(&*pid) {
                if let Some(c) = cur.as_deref() {
                    if !c.is_empty() {
                        if let Some(m) = pools.get(c) {
                            emit(m, std::mem::take(&mut sw))?;
                        }
                    }
                }
                cur = Some(pid.to_string());
                sw = Vec::new();
            }
            if !pools.contains_key(&*pid) {
                continue;
            }
            let mut f = [""; 12];
            let nf = split_fields(line, &mut f);
            let p = &f[..nf];
            sw.push(Swap {
                blk: numf(p, 1),
                li: numf(p, 2),
                tx: p.get(3).map_or(String::new(), |s| s.to_string()),
                a0: numf(p, 4),
                a1: numf(p, 5),
                sqrt_p: numf(p, 6) / Q96,
                l: numf(p, 7),
                fee: numf(p, 9),
            });
        }
        if let Some(c) = cur.as_deref() {
            if !c.is_empty() {
                if let Some(m) = pools.get(c) {
                    emit(m, std::mem::take(&mut sw))?;
                }
            }
        }
        Ok(())
    };

    if cfg.threads <= 1 {
        // urutan persis seperti JS: tiap pool disimulasikan begitu barisnya selesai dibaca
        scan(&mut |m, sw| {
            let r = ctx.run_pool(m, sw)?;
            out.write_all(r.jsonl.as_bytes()).map_err(|e| e.to_string())?;
            sum.n_trades += r.n_trades;
            sum.n_pools += 1;
            sum.n_snap += r.n_snap;
            Ok(())
        })?;
        out.flush().map_err(|e| e.to_string())?;
    } else {
        // paralel per pool; hasil ditulis berurutan menurut indeks pool (keluaran identik dengan 1 thread)
        let nth = cfg.threads;
        let res: Result<(), String> = std::thread::scope(|sc| {
            let (job_tx, job_rx) = mpsc::sync_channel::<(usize, PoolMeta, Vec<Swap>)>(nth * 4);
            let job_rx = Arc::new(Mutex::new(job_rx));
            let (res_tx, res_rx) = mpsc::channel::<(usize, Result<PoolOut, String>)>();
            for _ in 0..nth {
                let job_rx = Arc::clone(&job_rx);
                let res_tx = res_tx.clone();
                let ctx = &ctx;
                sc.spawn(move || loop {
                    let job = { job_rx.lock().unwrap().recv() };
                    match job {
                        Ok((idx, m, sw)) => {
                            let r = ctx.run_pool(&m, sw);
                            if res_tx.send((idx, r)).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                });
            }
            drop(res_tx);
            drop(job_rx); // penerima hanya dipegang pekerja: kalau semua pekerja berhenti, send() gagal (tidak macet)
            let writer = sc.spawn(move || -> Result<Summary, String> {
                let mut s = Summary { n_pools: 0, n_trades: 0, n_snap: 0 };
                let mut next = 0usize;
                let mut buf: BTreeMap<usize, Result<PoolOut, String>> = BTreeMap::new();
                for (idx, r) in res_rx {
                    buf.insert(idx, r);
                    while let Some(r) = buf.remove(&next) {
                        let r = r?; // galat pertama menurut urutan pool = sama dengan mode 1 thread
                        out.write_all(r.jsonl.as_bytes()).map_err(|e| e.to_string())?;
                        s.n_trades += r.n_trades;
                        s.n_pools += 1;
                        s.n_snap += r.n_snap;
                        next += 1;
                    }
                }
                out.flush().map_err(|e| e.to_string())?;
                Ok(s)
            });
            let mut idx = 0usize;
            let scan_res = scan(&mut |m, sw| {
                job_tx.send((idx, m.clone(), sw)).map_err(|_| "thread pekerja berhenti".to_string())?;
                idx += 1;
                Ok(())
            });
            drop(job_tx);
            let w = writer.join().map_err(|_| "thread penulis panik".to_string())?;
            let s = w?;
            scan_res?;
            sum = s;
            Ok(())
        });
        res?;
    }
    if prof {
        eprintln!("[prof] total: {:?} (thread simulasi: {})", t0.elapsed(), cfg.threads.max(1));
    }
    eprintln!("SELESAI: pool {} | trade {} | potret model {}", sum.n_pools, sum.n_trades, sum.n_snap);
    Ok(sum)
}

pub fn main_cli() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("pakai: replay <dir> <fromInit> <endBlock> <outTrades.jsonl> [ethUsd|auto]");
        eprintln!("env  : AUDIT='{{...}}' DANGER_FILE DANGER_TH UP_FILE UP_TH TP_UP (sama dengan replay.js)");
        eprintln!("       REPLAY_THREADS=N (bawaan 1; hasil identik), REPLAY_PROF=1");
        return 2;
    }
    let threads = std::env::var("REPLAY_THREADS").ok().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
    let cfg = ReplayCfg {
        dir: args[0].clone(),
        from_init: to_number(&args[1]),
        end_blk: to_number(&args[2]),
        out: args[3].clone(),
        eth_mode: args.get(4).cloned().unwrap_or_else(|| "file".to_string()),
        threads,
    };
    match run(&cfg) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}
