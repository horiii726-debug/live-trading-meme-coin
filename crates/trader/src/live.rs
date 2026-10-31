//! MODE LIVE (KERTAS) — dirancang jalan terus di VPS tanpa pengawasan.
//! Alur sama dengan paper_live.js; logika keputusan = bot.rs (terbukti sama dengan backtest):
//!  1. polling: SATU permintaan batch [eth_blockNumber, eth_getLogs(PoolManager, Initialize/Swap/ModifyLiquidity)]
//!  2. event disimpan ke store/ (format CSV collect.js) + state.json (blok terakhir)
//!  3. di menit-1 tiap koin: transfer token + dev pembuat diambil PARALEL di thread -> fitur -> skor
//!  4. gerbang beli & posisi: bot.rs + mesin keluar v4 Rust
//!  5. simulasi bayangan tiap koin -> labels.jsonl + filter adaptif (belajar.rs): DL belajar dari tiap hasil koin,
//!     ML CatBoost dilatih ulang oleh program C++ resmi saat perlu, dipasang SETELAH lolos cek kesamaan Rust vs CatBoost
//!  7. restart: muat checkpoint (dibuat saat akun tanpa posisi) lalu putar ulang event sesudahnya
//!     secara mengalir (tanpa jaringan) -> akun & posisi pulih persis; store dipadatkan bila besar
//! TIDAK ADA transaksi, dompet, atau kunci privat di kode ini.
use crate::belajar::{self, Label, Pembelajar};
use crate::bot::{Akun, Keputusan, Mesin, ModelOrder, SwapState};
use crate::catmodel::CatModel;
use crate::cfg::Cfg;
use crate::kelas::{self, Kelas};
use crate::net::{doh_resolve, Conn, Rpc};
use crate::radar::{self, MetaPool, Radar};
use std::sync::Mutex;
use num_bigint::BigInt;
use num_traits::{One, ToPrimitive};
use port_engine::engine::{PoolMeta, Swap};
use port_engine::model::Model;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSI: &str = "rhc-live 1.0";
const PM: &str = "0x8366a39cc670b4001a1121b8f6a443a643e40951"; // config.toml: pool_manager (terverifikasi)
const T_INIT: &str = "0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438";
const T_SWAP: &str = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f";
const T_MOD: &str = "0xf208f4912782fd25c7f114ca3723a2d5dd6f3bcc3ac8db5af63baa85f711d5ec";
const T_TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const ZERO32: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";
const ETH_POOL: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e"; // ETH/USDG fee 100
const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const Q96: f64 = 79228162514264337593543950336.0;
const HEAD_SWAP: &str = "pool_id,block,log_idx,tx_hash,a0,a1,sqrt_price,liquidity,tick,fee";
const HEAD_POOL: &str = "pool_id,c0,c1,fee,tick_spacing,hooks,init_block,log_idx";
const HEAD_LIQ: &str = "pool_id,block,log_idx,tick_lo,tick_hi,liq_delta";

// ---------- waktu, log, berkas ----------
pub fn sekarang_iso() -> String {
    let s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (d, r) = (s.div_euclid(86400), s.rem_euclid(86400));
    // tanggal sipil dari jumlah hari sejak 1970-01-01 (algoritma Howard Hinnant)
    let z = d + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1;
    let mm = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mm <= 2 { y + 1 } else { y };
    format!("{y:04}-{mm:02}-{dd:02}T{:02}:{:02}:{:02}Z", r / 3600, (r % 3600) / 60, r % 60)
}

struct Log {
    path: PathBuf,
    n: std::cell::Cell<u64>,
}
impl Log {
    fn tulis(&self, s: &str) {
        let l = format!("[{}] {s}", &sekarang_iso()[..19]);
        println!("{l}");
        self.n.set(self.n.get() + 1);
        if self.n.get() % 500 == 0 {
            // rotasi: live.log > 50 MB -> live.log.1 (yang lama ditimpa)
            if std::fs::metadata(&self.path).map(|m| m.len() > 50_000_000).unwrap_or(false) {
                let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
            }
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{l}");
        }
    }
}
fn tulis_aman(p: &Path, isi: &str) {
    let t = p.with_extension("tmp");
    if std::fs::write(&t, isi).is_ok() {
        let _ = std::fs::rename(&t, p);
    }
}
fn tambah(p: &Path, baris: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{baris}");
    }
}

/// Notifikasi Telegram (opsional: TELEGRAM_TOKEN + TELEGRAM_CHAT_ID). Dikirim di thread, tidak menahan bot.
#[derive(Clone)]
struct Kabar {
    token: String,
    chat: String,
    doh: bool,
    /// message prefix: "kertas" (paper), "SHADOW" or "LIVE"
    label: &'static str,
}
impl Kabar {
    fn kirim(&self, teks: String) {
        if self.token.is_empty() || self.chat.is_empty() {
            return;
        }
        let k = self.clone();
        let _ = std::thread::Builder::new().name("rhc-kabar".into()).spawn(move || {
            let host = "api.telegram.org";
            let ip = if k.doh {
                doh_resolve(host).ok()
            } else {
                use std::net::ToSocketAddrs;
                (host, 443).to_socket_addrs().ok().and_then(|mut a| a.next()).map(|a| a.ip())
            };
            if let Some(ip) = ip {
                let mut c = Conn::new(host, ip);
                let body = json!({"chat_id": k.chat, "text": format!("[RHC {}] {teks}", k.label)}).to_string();
                let _ = c.request(&format!("/bot{}/sendMessage", k.token), Some(&body), "", Duration::from_secs(15));
            }
        });
    }
}

// ---------- dekode ABI ----------
fn kata(data: &str, k: usize) -> &str {
    let d = data.trim_start_matches("0x");
    d.get(64 * k..64 * (k + 1)).unwrap_or("")
}
fn uint(w: &str) -> BigInt {
    BigInt::parse_bytes(w.as_bytes(), 16).unwrap_or_default()
}
/// int256 two's complement (int128/int24 di ABI sudah sign-extend ke 256 bit)
fn sint(w: &str) -> BigInt {
    let v = uint(w);
    if w.len() == 64 && w.as_bytes()[0] >= b'8' {
        v - (BigInt::one() << 256)
    } else {
        v
    }
}
fn alamat(t: &str) -> String {
    format!("0x{}", &t[t.len().saturating_sub(40)..]).to_lowercase()
}
fn hexnum(v: &Value) -> f64 {
    v.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).map(|x| x as f64).unwrap_or(f64::NAN)
}
fn big_f64(b: &BigInt) -> f64 {
    b.to_string().parse::<f64>().unwrap_or(f64::NAN)
}

#[derive(Clone)]
pub(crate) enum Ev {
    Init { id: String, c0: String, c1: String, fee: f64, ts: i64, hooks: String, blk: f64, li: f64 },
    Swap { id: String, blk: f64, li: f64, tx: String, a0: BigInt, a1: BigInt, sp: BigInt, l: BigInt, tick: i64, fee: f64 },
    Mod { id: String, blk: f64, li: f64, tlo: i64, thi: i64, d: BigInt },
}
impl Ev {
    fn blk(&self) -> f64 {
        match self {
            Ev::Init { blk, .. } | Ev::Swap { blk, .. } | Ev::Mod { blk, .. } => *blk,
        }
    }
    fn li(&self) -> f64 {
        match self {
            Ev::Init { li, .. } | Ev::Swap { li, .. } | Ev::Mod { li, .. } => *li,
        }
    }
    /// Deliver a late log at another position (see loggate.rs); the store records it where the engine saw it.
    fn restamp(&mut self, at_blk: f64, at_li: f64) {
        match self {
            Ev::Init { blk, li, .. } | Ev::Swap { blk, li, .. } | Ev::Mod { blk, li, .. } => {
                *blk = at_blk;
                *li = at_li;
            }
        }
    }
    pub(crate) fn csv(&self) -> (&'static str, String) {
        match self {
            Ev::Swap { id, blk, li, tx, a0, a1, sp, l, tick, fee } => ("swaps.csv", format!("{id},{blk},{li},{tx},{a0},{a1},{sp},{l},{tick},{fee}")),
            Ev::Init { id, c0, c1, fee, ts, hooks, blk, li } => ("pools.csv", format!("{id},{c0},{c1},{fee},{ts},{hooks},{blk},{li}")),
            Ev::Mod { id, blk, li, tlo, thi, d } => ("liq.csv", format!("{id},{blk},{li},{tlo},{thi},{d}")),
        }
    }
}
pub(crate) fn dekode(l: &Value) -> Option<Ev> {
    let tp: Vec<&str> = l["topics"].as_array()?.iter().filter_map(|t| t.as_str()).collect();
    let data = l["data"].as_str().unwrap_or("0x");
    let blk = hexnum(&l["blockNumber"]);
    let li = hexnum(&l["logIndex"]);
    let id = tp.get(1)?.to_lowercase();
    match *tp.first()? {
        T_SWAP => Some(Ev::Swap {
            id,
            blk,
            li,
            tx: l["transactionHash"].as_str().unwrap_or("").to_string(),
            a0: sint(kata(data, 0)),
            a1: sint(kata(data, 1)),
            sp: uint(kata(data, 2)),
            l: uint(kata(data, 3)),
            tick: sint(kata(data, 4)).to_i64().unwrap_or(0),
            fee: big_f64(&uint(kata(data, 5))),
        }),
        T_INIT => Some(Ev::Init {
            id,
            c0: alamat(tp.get(2)?),
            c1: alamat(tp.get(3)?),
            fee: big_f64(&uint(kata(data, 0))),
            ts: sint(kata(data, 1)).to_i64().unwrap_or(0),
            hooks: alamat(kata(data, 2)),
            blk,
            li,
        }),
        T_MOD => Some(Ev::Mod { id, blk, li, tlo: sint(kata(data, 0)).to_i64().unwrap_or(0), thi: sint(kata(data, 1)).to_i64().unwrap_or(0), d: sint(kata(data, 2)) }),
        _ => None,
    }
}
fn baris_ke_ev(f: &str, l: &str) -> Option<Ev> {
    let q: Vec<&str> = l.trim().split(',').collect();
    if q.len() < 6 || q[0] == "pool_id" {
        return None;
    }
    let n = |i: usize| q.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::NAN);
    let b = |i: usize| q.get(i).and_then(|s| port_engine::engine::parse_bigint(s).ok());
    let e = match f {
        "pools.csv" if q.len() >= 8 => Some(Ev::Init { id: q[0].into(), c0: q[1].into(), c1: q[2].into(), fee: n(3), ts: n(4) as i64, hooks: q[5].into(), blk: n(6), li: n(7) }),
        "swaps.csv" if q.len() >= 10 => match (b(4), b(5), b(6), b(7)) {
            (Some(a0), Some(a1), Some(sp), Some(lq)) => Some(Ev::Swap { id: q[0].into(), blk: n(1), li: n(2), tx: q[3].into(), a0, a1, sp, l: lq, tick: n(8) as i64, fee: n(9) }),
            _ => None,
        },
        "liq.csv" => b(5).map(|d| Ev::Mod { id: q[0].into(), blk: n(1), li: n(2), tlo: n(3) as i64, thi: n(4) as i64, d }),
        _ => None,
    }?;
    if e.blk().is_finite() && e.li().is_finite() {
        Some(e)
    } else {
        None
    }
}

/// Rows an older version stored twice (see loggate.rs) are replayed once. Only rows of the same block are compared.
fn once_each(g: Vec<Ev>, dropped: &mut u64) -> Vec<Ev> {
    let mut seen = HashSet::new();
    g.into_iter()
        .filter(|e| {
            let first = seen.insert(e.csv());
            *dropped += u64::from(!first);
            first
        })
        .collect()
}

/// Pembaca store MENGALIR: menggabungkan 3 berkas CSV (masing-masing berurutan waktu) menurut (blok, log).
/// Memori tetap kecil walau store berisi berbulan-bulan data. Baris rusak dilewati.
struct Aliran {
    src: Vec<(&'static str, std::io::Lines<BufReader<std::fs::File>>, Option<Ev>)>,
}
impl Aliran {
    fn buka(dir: &Path) -> Aliran {
        let mut src = Vec::new();
        for f in ["pools.csv", "swaps.csv", "liq.csv"] {
            if let Ok(fh) = std::fs::File::open(dir.join(f)) {
                let mut it = BufReader::with_capacity(1 << 20, fh).lines();
                let first = Self::maju(f, &mut it);
                src.push((f, it, first));
            }
        }
        Aliran { src }
    }
    fn maju(f: &str, it: &mut std::io::Lines<BufReader<std::fs::File>>) -> Option<Ev> {
        for l in it.by_ref().map_while(Result::ok) {
            if let Some(e) = baris_ke_ev(f, &l) {
                return Some(e);
            }
        }
        None
    }
    fn berikut(&mut self) -> Option<Ev> {
        let mut best: Option<usize> = None;
        for (i, s) in self.src.iter().enumerate() {
            if let Some(e) = &s.2 {
                let lebih_awal = match best.and_then(|b| self.src[b].2.as_ref()) {
                    None => true,
                    Some(o) => (e.blk(), e.li()) < (o.blk(), o.li()),
                };
                if lebih_awal {
                    best = Some(i);
                }
            }
        }
        let i = best?;
        let f = self.src[i].0;
        let nxt = Self::maju(f, &mut self.src[i].1);
        std::mem::replace(&mut self.src[i].2, nxt)
    }
}

// ---------- pengecekan menit-1 (thread) ----------
struct HasilCek {
    pool: String,
    k: Option<Keputusan>,
    err: Option<String>,
}

// ---------- dev pembuat token (aturan "dev pernah rug") ----------
/// Hasil pencarian dev per pool. Pencarian mulai saat pool lahir (Init, `pra_cek_dev`) sehingga punya ~70 dtk untuk
/// mengulang; keputusan menit-1 (`cek_koin`) tinggal membaca hasilnya.
#[derive(Clone)]
enum DevCek {
    Berjalan,
    Selesai(Result<Option<String>, String>),
}
type DevPra = Arc<Mutex<HashMap<String, DevCek>>>;
/// pra-cek: dari Init sampai keputusan menit-1 harus siap (menit-1 + ~10 dtk, lihat "cek telat" di bot.rs)
const DEV_BATAS_PRA: Duration = Duration::from_secs(68);
/// cek menit-1: menunggu pra-cek / mencari sendiri, sejajar dengan pengambilan transfer
const DEV_BATAS_CEK: Duration = Duration::from_secs(8);

/// Ulangi `f` dengan jeda bertambah (0,25 dtk .. 3 dtk) sampai berhasil atau `batas` lewat.
fn ulangi<T>(batas: Instant, mut f: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut jeda = Duration::from_millis(250);
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if Instant::now() + jeda >= batas {
                    return Err(e);
                }
                std::thread::sleep(jeda);
                jeda = (jeda * 2).min(Duration::from_secs(3));
            }
        }
    }
}

/// Dev = pengirim transaksi mint PERTAMA token di [init - 36000, init + 600] (definisi yang sama dengan riwayat dev).
/// Ok(None): memang tidak ada mint di rentang itu (aturan dev tidak berlaku, sama seperti backtest).
/// Err: RPC gagal sampai `batas` -> koin itu TIDAK dibeli (`cek_koin`), supaya aturan tidak mati diam-diam.
/// Maju dari blok terlama dalam batch kecil 6 x 1000 blok: satu batch 37 permintaan dijawab 429 oleh RPC publik
/// (terukur: dev selalu "?"), lalu dulu langsung menyerah.
fn cari_dev(rpc: &Rpc, token: &str, init: f64, batas: Instant) -> Result<Option<String>, String> {
    let filter = json!({"address": token, "topics": [T_TRANSFER, ZERO32]});
    let (mut a, sampai) = ((init - 36000.0).max(0.0) as u64, (init + 600.0) as u64);
    while a <= sampai {
        let b = a.saturating_add(5999).min(sampai);
        let mint = ulangi(batas, || rpc.logs_ranged(&filter, a, b, 1000, 4000))?;
        if let Some(h) = mint.first().and_then(|l| l["transactionHash"].as_str()) {
            let tx = ulangi(batas, || rpc.quick("eth_getTransactionByHash", json!([h]), 2000))?;
            return tx["from"].as_str().map(|s| Some(s.to_lowercase())).ok_or_else(|| "transaksi mint tanpa pengirim".to_string());
        }
        a = b + 1;
    }
    Ok(None)
}

/// Riwayat dev bawaan (models/dev_history.json: dev -> hasil trade-nya, terkumpul dari backtest) ke akun.dev_hist,
/// supaya aturan "dev pernah rug" sudah mengenal dev perug sejak menit pertama, bukan baru setelah bot sendiri kena
/// rug. Digabung tanpa duplikat: riwayat yang tersimpan di checkpoint sudah memuatnya sejak start pertama.
/// Hasil: (jumlah dev di berkas, jumlah dev yang kini tercatat pernah rug).
fn muat_riwayat_dev(akun: &mut Akun, p: &Path) -> Result<(usize, usize), String> {
    let t = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let j: Value = serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display()))?;
    let o = j.as_object().ok_or_else(|| format!("{}: harus berbentuk {{dev: [hasil, ...]}}", p.display()))?;
    for (d, v) in o {
        let h = akun.dev_hist.entry(d.to_lowercase()).or_default();
        for r in v.as_array().into_iter().flatten().filter_map(|x| x.as_f64()) {
            if !h.contains(&r) {
                h.push(r);
            }
        }
    }
    let rug = akun.dev_hist.values().filter(|h| h.iter().any(|r| *r < 0.3)).count();
    Ok((o.len(), rug))
}

fn cek_koin(rpc: &Rpc, cat: &CatModel, kc: Arc<Mutex<kelas::Cache>>, pra: DevPra, meta: PoolMeta, token: String, sw: Vec<Swap>, eth: f64) -> Result<Keputusan, String> {
    let init = meta.init;
    // KLASIFIKASI DULU: saham ter-tokenisasi & token yang gagal diklasifikasi tidak boleh dibeli
    let (rpc3, tok3, kc3) = (rpc.clone(), token.clone(), kc.clone());
    let th_kelas = std::thread::spawn(move || -> Option<String> {
        if let Some(k) = kc3.lock().unwrap().peta.get(&tok3).cloned() {
            return if k == Kelas::Meme { None } else { Some(format!("aset {}", k.nama())) };
        }
        match kelas::klasifikasi(&rpc3, &tok3) {
            Ok(k) => {
                kc3.lock().unwrap().peta.insert(tok3, k);
                if k == Kelas::Meme { None } else { Some(format!("aset {}", k.nama())) }
            }
            Err(_) => Some("klasifikasi gagal".into()),
        }
    });
    let hx = |b: f64| format!("0x{:x}", b as u64);
    // dev pembuat token (pengirim transaksi mint): hasil pra-cek yang mulai saat pool lahir. Kalau tidak ada (pool
    // lahir sebelum bot menyala) atau gagal, cari sekali lagi di sini. Paralel dengan pengambilan transfer menit-1.
    let (rpc2, tok2, id2) = (rpc.clone(), token.clone(), meta.id.clone());
    let th_dev = std::thread::spawn(move || -> Result<Option<String>, String> {
        let batas = Instant::now() + DEV_BATAS_CEK;
        loop {
            let st = pra.lock().unwrap().get(&id2).cloned();
            match st {
                Some(DevCek::Berjalan) if Instant::now() < batas => std::thread::sleep(Duration::from_millis(100)),
                Some(DevCek::Berjalan) => return Err("pencarian dev belum selesai".into()),
                Some(DevCek::Selesai(Ok(d))) => {
                    pra.lock().unwrap().remove(&id2);
                    return Ok(d);
                }
                _ => {
                    pra.lock().unwrap().remove(&id2);
                    return cari_dev(&rpc2, &tok2, init, batas);
                }
            }
        }
    });
    // retried with a pause, within the same budget as the dev lookup: three instant tries all hit the same 429 on
    // the public RPC and the coin was skipped ("CEK GAGAL")
    let batas_tr = Instant::now() + DEV_BATAS_CEK;
    let logs = ulangi(batas_tr, || rpc.quick("eth_getLogs", json!([{"address": token, "topics": [T_TRANSFER], "fromBlock": hx(init), "toBlock": hx(init + 600.0)}]), 3000))?;
    let mut tr = Vec::new();
    for l in logs.as_array().ok_or("transfer bukan array")? {
        let tp: Vec<&str> = l["topics"].as_array().map(|a| a.iter().filter_map(|t| t.as_str()).collect()).unwrap_or_default();
        if tp.len() < 3 {
            continue;
        }
        let amt = uint(l["data"].as_str().unwrap_or("0x0").trim_start_matches("0x"));
        tr.push(port_features::Transfer { blk: hexnum(&l["blockNumber"]), li: hexnum(&l["logIndex"]), from: alamat(tp[1]), to: alamat(tp[2]), amt: big_f64(&amt) });
    }
    let fm = port_features::Meta { c0: meta.c0.clone(), c1: meta.c1.clone(), fee: meta.fee, hooks: meta.hooks.clone(), init };
    let fsw: Vec<port_features::Swap> = sw.iter().filter(|x| x.blk <= init + 600.0).map(|x| port_features::Swap { blk: x.blk, li: x.li, a0: x.a0, a1: x.a1, sqrt_p: x.sqrt_p, l: x.l }).collect();
    let fe = port_features::features(&fm, &fsw, &tr, eth);
    // WAJIB -1 untuk nilai hilang (model dilatih dengan fillna(-1))
    let x: Vec<f64> = cat.feats.iter().map(|k| fe.get(k).filter(|v| v.is_finite()).unwrap_or(-1.0)).collect();
    let score = cat.predict(&x);
    let q1 = meta.c1 == NATIVE;
    let jual = fsw.iter().filter(|s| (if q1 { s.a0 } else { s.a1 }) < 0.0).count() as i64;
    let dikecualikan = th_kelas.join().unwrap_or(Some("klasifikasi gagal".into()));
    // the rug rule must never be skipped silently: a dev that could not be looked up = this coin is not bought
    let (dev, dikecualikan) = match th_dev.join().unwrap_or_else(|_| Err("thread dev panik".into())) {
        Ok(d) => (d, dikecualikan),
        Err(e) => (None, dikecualikan.or(Some(format!("dev tidak diketahui ({})", e.chars().take(60).collect::<String>())))),
    };
    Ok(Keputusan { score, ml_ok: false, dev, jual_m1: jual, lp_m1: fe.get("lp_m1").unwrap_or(f64::NAN), feats: x, dikecualikan })
}

fn muat_model_filter(bot_dir: &str) -> Result<(CatModel, f64), String> {
    let cat = CatModel::load(&format!("{bot_dir}/filter_cat.json")).map_err(|e| e.to_string())?;
    let thr0 = serde_json::from_str::<Value>(&std::fs::read_to_string(format!("{bot_dir}/filter_cat.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?["thr_start"]
        .as_f64()
        .unwrap_or(0.195);
    cek_parity(bot_dir, "filter", &|x| cat.predict(x), "filter_cat_check.json")?;
    Ok((cat, thr0))
}
fn cek_parity(bot_dir: &str, nama: &str, pred: &dyn Fn(&[f64]) -> f64, f: &str) -> Result<(), String> {
    let ck: Value = serde_json::from_str(&std::fs::read_to_string(format!("{bot_dir}/{f}")).map_err(|e| format!("{f}: {e}"))?).map_err(|e| e.to_string())?;
    let (xs, ps) = (ck["X"].as_array().ok_or("X")?, ck["p"].as_array().ok_or("p")?);
    let mut mx = 0.0f64;
    for (x, p) in xs.iter().zip(ps) {
        let xv: Vec<f64> = x.as_array().map(|a| a.iter().map(|e| e.as_f64().unwrap_or(f64::NAN)).collect()).unwrap_or_default();
        mx = mx.max((pred(&xv) - p.as_f64().unwrap_or(f64::NAN)).abs());
    }
    if !(mx <= 1e-9) || xs.is_empty() {
        return Err(format!("model {nama} Rust != Python ({mx:e})"));
    }
    eprintln!("model {nama}: parity {mx:.2e} OK ({} sampel)", xs.len());
    Ok(())
}

// ---------- bot ----------
struct Live {
    cfg: Cfg,
    bot_dir: String,
    dir: PathBuf,
    store: PathBuf,
    log: Log,
    kabar: Kabar,
    m: Mesin,
    eth: f64,
    replaying: bool,
    salin_ke: Option<PathBuf>,
    sudah_dicek: HashSet<String>,
    keputusan_simpan: VecDeque<(f64, String, Value)>,
    tx: mpsc::Sender<HasilCek>,
    rx: mpsc::Receiver<HasilCek>,
    rpc: Rpc,
    cat: Arc<CatModel>,
    // adaptive filter (None = BELAJAR=0: the shipped model, frozen)
    pemb: Option<Pembelajar>,
    latih_tx: mpsc::Sender<HasilLatih>,
    latih_rx: mpsc::Receiver<HasilLatih>,
    label_disimpan: usize,
    last: f64,
    tip: f64,
    cp_blk: f64,
    ok_poll: Instant,
    aman: bool,
    pulih_sejak: Option<Instant>,
    n_basi: u64,
    n_cek_gagal: u64,
    n_label: u64,
    mulai: Instant,
    last_err: String,
    killed_dikabarkan: bool,
    ringkas_t: Instant,
    // radar (jalur terpisah dari trading)
    radar: Radar,
    kc: Arc<Mutex<kelas::Cache>>,
    minta_tx: mpsc::Sender<Minta>,
    hasil_rx: mpsc::Receiver<HasilLatar>,
    ds_mau: Arc<Mutex<Vec<String>>>,
    // live execution (None = paper only)
    exec: Option<crate::exec::ExecHandle>,
    pool_keys: HashMap<String, crate::exec::uniswap::Key>,
    // exactly-once log delivery and the largest getLogs block range the endpoint accepts
    gate: crate::loggate::LogGate,
    range_cap: u64,
    t_err_log: Instant,
    // blocks processed in MODE AMAN while live (from decisions.jsonl), replayed identically
    aman_simpan: HashSet<u64>,
    /// block of the first ETH price rejected by the jump guard (NaN = none)
    eth_tolak_sejak: f64,
    // dev lookups started when a pool is born, read by the minute-1 check
    dev_pra: DevPra,
}

/// Wait after a failed poll: doubles from 0.25 s up to 3 s, back to 0.25 s after a good poll (a rate limit costs a
/// fraction of a second of data, not a fixed 3 s).
const POLL_BACKOFF_MIN: Duration = Duration::from_millis(250);
const POLL_BACKOFF_MAX: Duration = Duration::from_secs(3);

/// A finished ML retrain (CatBoost program, in its own thread).
struct HasilLatih {
    r: Result<(Value, Value), String>,
    n_label: usize,
    alasan: &'static str,
}

/// labels.jsonl -> the learner's labels (features in model order; "lolos" = the coin passed the filter)
fn baca_label(p: &Path, feats: &[String]) -> Vec<(Label, bool)> {
    let mut out = Vec::new();
    let f = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return out,
    };
    for l in BufReader::new(f).lines().map_while(Result::ok) {
        let j: Value = match serde_json::from_str(&l) {
            Ok(j) => j,
            Err(_) => continue,
        };
        let (ret, blk) = match (j["ret"].as_f64(), j["decide"].as_f64()) {
            (Some(r), Some(b)) if r.is_finite() => (r, b),
            _ => continue,
        };
        let x: Option<Vec<f64>> = feats.iter().map(|k| j["f"].get(k).and_then(|v| v.as_f64())).collect();
        if let Some(x) = x {
            out.push((Label { x, ret, blk }, j["lolos"].as_bool().unwrap_or(false)));
        }
    }
    out
}

/// Pekerjaan latar radar (RPC baca saja)
enum Minta {
    Meta(String, Option<MetaPool>),
    Kelas(Vec<String>),
    Desimal(Vec<String>),
    Pabrik(String, bool, u64),
    BootV4(u64, u64),
}
enum HasilLatar {
    Meta(String, Option<MetaPool>),
    Kelas(Vec<String>, Result<Vec<(String, Kelas)>, String>),
    Desimal(Vec<String>, Option<Vec<(String, f64)>>),
    MetaBanyak(Vec<(String, MetaPool)>, String),
    Ds(Vec<Value>, Vec<(String, f64, bool)>),
}

impl Live {
    fn simpan_event(&self, e: &Ev) {
        let dir = match (&self.salin_ke, self.replaying) {
            (Some(d), true) => d.clone(),
            (_, true) => return,
            _ => self.store.clone(),
        };
        let (f, l) = e.csv();
        let p = dir.join(f);
        let baru = !p.exists();
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            if baru {
                let h = match f {
                    "swaps.csv" => HEAD_SWAP,
                    "pools.csv" => HEAD_POOL,
                    _ => HEAD_LIQ,
                };
                let _ = writeln!(fh, "{h}");
            }
            let _ = writeln!(fh, "{l}");
        }
    }
    /// Terapkan hasil cek (urutan kedatangan). Disimpan ke decisions.jsonl agar putar ulang identik.
    fn terapkan(&mut self, blk: f64, pool: &str, mut k: Keputusan) {
        // adaptive filter: the score is given here, at the decision (a replay re-applies the stored score instead)
        if !self.replaying && self.pemb.is_some() {
            if let Some(alasan) = self.pemb.as_ref().and_then(|p| p.perlu_latih()) {
                self.mulai_latih(alasan);
            }
            if let Some(p) = self.pemb.as_mut() {
                k.score = p.skor(&k.feats);
            }
            if !k.score.is_finite() {
                crate::kill::picu(&format!("skor filter ML+DL tidak valid ({}) untuk {pool}", k.score));
                return;
            }
        }
        let mut a = self.m.akun.lock().unwrap();
        a.terima_skor(pool, k.clone());
        let ok = a.kep.get(pool).map_or(false, |x| x.ml_ok);
        let n = a.recent.len();
        drop(a);
        if !self.replaying {
            let d = json!({"pool": pool, "pada": blk, "score": k.score, "ml_ok": ok, "dev": k.dev, "jual_m1": k.jual_m1, "lp_m1": k.lp_m1, "feats": k.feats, "dikecualikan": k.dikecualikan, "t": sekarang_iso()});
            tambah(&self.dir.join("decisions.jsonl"), &d.to_string());
            self.log.tulis(&format!(
                "SARING {} | skor {:.3} {} | dev {} | penjual menit-1 {} | LP ${:.0} | skor ke-{}{}",
                &pool[..10.min(pool.len())],
                k.score,
                if k.dikecualikan.is_some() { "DIKECUALIKAN" } else if ok { "LOLOS" } else { "tolak(ml)" },
                k.dev.as_deref().map(|d| &d[..8.min(d.len())]).unwrap_or("?"),
                k.jual_m1,
                k.lp_m1,
                n,
                k.dikecualikan.as_deref().map(|x| format!(" | {x}")).unwrap_or_default()
            ));
        }
    }
    /// Start looking up the token's dev the moment its pool is born: the minute-1 decision is ~60 s later and must
    /// not wait for (or give up on) a slow, rate-limited lookup.
    fn pra_cek_dev(&self, id: &str, c0: &str, c1: &str, init: f64) {
        let token = if c0 == NATIVE { c1.to_string() } else { c0.to_string() };
        let (rpc, pra, id) = (self.rpc.clone(), self.dev_pra.clone(), id.to_string());
        {
            let mut g = pra.lock().unwrap();
            if g.len() > 20_000 {
                g.clear(); // results never read (pools skipped before minute 1); a live one is simply looked up again
            }
            g.insert(id.clone(), DevCek::Berjalan);
        }
        std::thread::spawn(move || {
            let r = cari_dev(&rpc, &token, init, Instant::now() + DEV_BATAS_PRA);
            pra.lock().unwrap().insert(id, DevCek::Selesai(r));
        });
    }
    fn mulai_cek(&mut self, blk: f64) {
        let perlu: Vec<String> = self
            .m
            .sims
            .iter()
            .filter(|(id, s)| !id.starts_with("b:") && s.ei < 0 && !s.skip && blk >= s.m.init + 600.0 && !self.sudah_dicek.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in perlu {
            self.sudah_dicek.insert(id.clone());
            if self.replaying {
                continue; // putar ulang: keputusan tersimpan diterapkan pada blok yang sama seperti saat live
            }
            let s = &self.m.sims[&id];
            if self.tip - (s.m.init + 630.0) > self.cfg.fresh_blocks {
                self.n_basi += 1;
                continue; // pasti ditolak "data basi" -> jangan habiskan RPC
            }
            let (rpc, meta, sw, eth) = (self.rpc.clone(), s.m.clone(), s.sw.clone(), s.eth_usd);
            let token = if meta.c0 == NATIVE { meta.c1.clone() } else { meta.c0.clone() };
            let (tx, cat, pool, kc, pra) = (self.tx.clone(), self.cat.clone(), id.clone(), self.kc.clone(), self.dev_pra.clone());
            std::thread::spawn(move || {
                let r = cek_koin(&rpc, &cat, kc, pra, meta, token, sw, eth);
                let _ = match r {
                    Ok(k) => tx.send(HasilCek { pool, k: Some(k), err: None }),
                    Err(e) => tx.send(HasilCek { pool, k: None, err: Some(e) }),
                };
            });
        }
    }
    fn ambil_hasil(&mut self, blk: f64) {
        while let Ok(h) = self.rx.try_recv() {
            match h.k {
                Some(k) => self.terapkan(blk, &h.pool, k),
                None => {
                    self.n_cek_gagal += 1;
                    self.log.tulis(&format!("CEK GAGAL {} dilewati ({})", &h.pool[..10.min(h.pool.len())], h.err.unwrap_or_default().chars().take(50).collect::<String>()));
                }
            }
        }
    }
    fn proses_blok(&mut self, blk: f64, evs: &[Ev]) {
        let mut per: BTreeMap<String, (Vec<BigInt>, Vec<Swap>)> = BTreeMap::new();
        for e in evs {
            match e {
                Ev::Init { id, c0, c1, fee, ts, hooks, .. } => {
                    self.simpan_event(e);
                    let m = PoolMeta { id: id.clone(), c0: c0.clone(), c1: c1.clone(), fee: *fee, hooks: hooks.clone(), init: blk };
                    let tracked = self.m.pool_baru(m, self.eth);
                    if tracked && !self.replaying {
                        self.pra_cek_dev(id, c0, c1, blk);
                    }
                    if tracked && self.exec.is_some() {
                        if let (Ok(a0), Ok(a1), Ok(h)) = (c0.parse(), c1.parse(), hooks.parse()) {
                            let key = crate::exec::uniswap::Key { currency0: a0, currency1: a1, fee: *fee as u32, tick_spacing: *ts as i32, hooks: h };
                            self.pool_keys.insert(id.clone(), key);
                        }
                    }
                }
                Ev::Swap { id, sp, .. } if id == ETH_POOL => {
                    self.simpan_event(e);
                    let s = big_f64(sp) / Q96;
                    let baru = s * s * 1e12;
                    // batas kewajaran: satu swap manipulatif di pool ETH/USDG tidak boleh menggeser seluruh valuasi. Harga
                    // yang menjauh selama 2 menit adalah harga pasar (bot mati saat ETH bergerak >10%): tanpa ini harga
                    // ETH beku selamanya, juga setelah restart, karena checkpoint menyimpan harga lama itu.
                    if baru > 500.0 && baru < 20000.0 {
                        let wajar = !self.eth.is_finite() || (baru / self.eth - 1.0).abs() < 0.10;
                        if !wajar && self.eth_tolak_sejak.is_nan() {
                            self.eth_tolak_sejak = blk;
                        }
                        if wajar || blk - self.eth_tolak_sejak >= 1200.0 {
                            self.eth_tolak_sejak = f64::NAN;
                            self.eth = baru;
                            for s in self.m.sims.values_mut() {
                                s.eth_usd = baru;
                            }
                        }
                    }
                }
                Ev::Swap { id, li, tx, a0, a1, sp, l, fee, .. } => {
                    if !self.m.sims.contains_key(id) {
                        continue;
                    }
                    self.simpan_event(e);
                    per.entry(id.clone()).or_default().1.push(Swap { blk, li: *li, tx: tx.clone(), a0: big_f64(a0), a1: big_f64(a1), sqrt_p: big_f64(sp) / Q96, l: big_f64(l), fee: *fee });
                }
                Ev::Mod { id, d, .. } => {
                    if !self.m.sims.contains_key(id) {
                        continue;
                    }
                    self.simpan_event(e);
                    per.entry(id.clone()).or_default().0.push(d.clone());
                }
            }
        }
        if self.replaying {
            // keputusan tersimpan diterapkan pada blok yang sama seperti saat live
            while self.keputusan_simpan.front().map_or(false, |x| x.0 <= blk) {
                let (_, pool, v) = self.keputusan_simpan.pop_front().unwrap();
                let k = Keputusan {
                    score: v["score"].as_f64().unwrap_or(f64::NAN),
                    ml_ok: false,
                    dev: v["dev"].as_str().map(|s| s.to_string()),
                    jual_m1: v["jual_m1"].as_i64().unwrap_or(0),
                    lp_m1: v["lp_m1"].as_f64().unwrap_or(f64::NAN),
                    feats: v["feats"].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(-1.0)).collect()).unwrap_or_default(),
                    dikecualikan: v["dikecualikan"].as_str().map(|s| s.to_string()),
                };
                self.terapkan(blk, &pool, k);
            }
        } else {
            self.ambil_hasil(blk);
        }
        {
            let mut a = self.m.akun.lock().unwrap();
            a.tip = if self.replaying { blk } else { self.tip };
            a.live = !self.replaying;
            a.replay = self.replaying;
            // MODE AMAN: RPC tidak sehat -> tidak boleh membeli (posisi terbuka tetap dikelola). Each such block is
            // recorded in decisions.jsonl, so a replay drops the same decisions at the same blocks.
            let aman = if self.replaying { self.aman_simpan.contains(&(blk as u64)) } else { self.aman };
            if aman {
                let ids: Vec<String> = self.m.sims.iter().filter(|(k, s)| !k.starts_with("b:") && s.ei < 0).map(|(k, _)| k.clone()).collect();
                for id in ids {
                    a.kep.remove(&id);
                }
                if !self.replaying {
                    tambah(&self.dir.join("decisions.jsonl"), &json!({"aman": blk, "t": sekarang_iso()}).to_string());
                }
            }
        }
        self.m.blok(blk, &per);
        self.mulai_cek(blk);
        self.lapor();
    }
    /// Hand the account's new orders to the executor first (lowest latency). Replayed history is never executed.
    fn kirim_order(&mut self) {
        if self.m.orders.is_empty() {
            return;
        }
        let orders: Vec<ModelOrder> = self.m.orders.drain(..).collect();
        if crate::kill::aktif() {
            return;
        }
        let Some(ex) = &self.exec else { return };
        if self.replaying {
            return;
        }
        let now = Instant::now();
        let ps = |s: Option<SwapState>| s.map(|s| crate::exec::amm::PoolState { sqrt_p: s.sqrt_p, liquidity: s.l, fee_pips: s.fee, blk: s.blk });
        for o in orders {
            let (pool, kind, state) = match o {
                ModelOrder::Buy { pool, size_usd, lp_cap_usd, model_equity_usd, state } => (pool, crate::exec::OrderKind::Buy { size_usd, model_equity_usd, lp_cap_usd }, state),
                ModelOrder::TakeProfit { pool, frac, state } => (pool, crate::exec::OrderKind::TakeProfit { frac }, state),
                ModelOrder::Exit { pool, why, state } => (pool, crate::exec::OrderKind::Exit { why }, state),
                ModelOrder::Closed { pool, reason, ret, state } => (pool, crate::exec::OrderKind::Closed { reason, ret }, state),
            };
            let key = self.pool_keys.get(&pool).copied();
            let order = crate::exec::Order { pool, kind, key, state: ps(state), eth_usd: self.eth, created: now };
            if !ex.submit(order) {
                self.log.tulis("EXEC: executor thread is not running; order dropped");
            }
        }
    }
    fn lapor(&mut self) {
        self.kirim_order();
        for e in self.m.event_log.drain(..) {
            if !self.replaying {
                tambah(&self.dir.join("events.jsonl"), &e);
            }
        }
        let selesai: Vec<_> = self.m.selesai.drain(..).collect();
        for (t, r) in selesai {
            if self.replaying {
                continue;
            }
            tambah(&self.dir.join("trades.jsonl"), &t.to_json());
            if let Some((pos, back)) = r {
                self.log.tulis(&format!(
                    "TUTUP {} {:+.0}% | akun ${:.2} -> ${:.2} ({:+.2}) | {:.0} mnt | {}",
                    &t.pool[..10.min(t.pool.len())],
                    100.0 * (t.ret - 1.0),
                    pos,
                    back,
                    back - pos,
                    t.hold,
                    t.reason
                ));
            }
        }
        // label latihan: hasil simulasi bayangan + fitur menit-1 koin itu
        let labels: Vec<_> = self.m.label_selesai.drain(..).collect();
        if !labels.is_empty() && !self.replaying {
            let a = self.m.akun.lock().unwrap();
            let mut out = String::new();
            let mut belajar: Vec<(Vec<f64>, f64, f64, bool)> = Vec::new();
            for t in &labels {
                if let Some(k) = a.kep.get(&t.pool) {
                    if k.feats.len() == self.cat.feats.len() {
                        let f: serde_json::Map<String, Value> = self.cat.feats.iter().cloned().zip(k.feats.iter().map(|v| json!(v))).collect();
                        let j = json!({"pool": t.pool, "init": t.init, "decide": t.init + 600.0, "entry": t.entry_blk, "exit": t.exit_blk,
                            "ret": t.ret, "reason": t.reason, "dev": k.dev, "jual_m1": k.jual_m1, "score": k.score, "lolos": k.ml_ok, "f": f});
                        out.push_str(&j.to_string());
                        out.push('\n');
                        self.n_label += 1;
                        if t.ret.is_finite() {
                            belajar.push((k.feats.clone(), t.ret, t.init + 600.0, k.ml_ok));
                        }
                    }
                }
            }
            drop(a);
            if let Some(p) = self.pemb.as_mut() {
                let dl_sebelum = p.dl_aktif;
                for (x, ret, blk, pick) in belajar {
                    p.label(x, ret, blk, pick);
                }
                if let Err(e) = p.cek_sehat() {
                    crate::kill::picu(&format!("pembelajar ML/DL rusak: {e}"));
                }
                if p.dl_aktif != dl_sebelum {
                    let (d, m) = p.akurasi().unwrap_or((f64::NAN, f64::NAN));
                    let s = if p.dl_aktif {
                        format!("PENGAMAN: DL dipakai lagi (akurasi data belum dilihat DL AUC {d:.3}, ML {m:.3})")
                    } else {
                        format!("PENGAMAN OVERFIT: DL dimatikan, skor = ML saja (DL AUC {d:.3} setara tebakan acak, ML {m:.3})")
                    };
                    self.log.tulis(&s);
                    self.kabar.kirim(s);
                }
                if p.labels.len() >= self.label_disimpan + 25 {
                    self.simpan_belajar();
                }
            }
            if !out.is_empty() {
                if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("labels.jsonl")) {
                    let _ = fh.write_all(out.as_bytes());
                }
            }
        }
    }
    fn status(&mut self) {
        // account numbers that cannot be right = an internal error (checked only while the switch is off)
        if !crate::kill::aktif() {
            let a = self.m.akun.lock().unwrap();
            let (cash, eb, ep, kel) = (a.cash, a.ekuitas_beli(), a.ekuitas_pasar(), a.kelly());
            let max_k = a.cfg.kelly_maks.max(a.cfg.kelly_min);
            drop(a);
            let salah = if !(cash.is_finite() && eb.is_finite() && ep.is_finite() && kel.is_finite()) {
                Some(format!("angka akun tidak valid: kas {cash}, equity {eb}/{ep}, kelly {kel}"))
            } else if cash < -1.0 || eb < -1.0 {
                Some(format!("akun negatif: kas ${cash:.2}, equity ${eb:.2}"))
            } else if !(0.0..=max_k + 1e-9).contains(&kel) {
                Some(format!("fraksi Kelly {kel} di luar 0..{max_k}"))
            } else {
                None
            };
            if let Some(s) = salah {
                crate::kill::picu(&s);
            }
        }
        let a = self.m.akun.lock().unwrap();
        let killed = a.killed;
        let j = json!({
            "versi": VERSI, "updated": sekarang_iso(),
            "mode": match self.kabar.label { "LIVE" => "LIVE (real transactions) — Rust engine", "SHADOW" => "SHADOW (simulated execution, no transactions) — Rust engine", _ => "KERTAS (tanpa transaksi) — mesin Rust" },
            "last_block": self.last, "tip": self.tip, "lag_blocks": self.tip - self.last, "checkpoint_blok": self.cp_blk,
            "rpc_diam_dtk": self.ok_poll.elapsed().as_secs(), "mode_aman": self.aman, "eth_usd": self.eth,
            "pools_dipantau": self.m.sims.keys().filter(|k| !k.starts_with("b:")).count(), "uptime_menit": self.mulai.elapsed().as_secs() / 60, "last_error": self.last_err,
            "akun": {
                "modal_awal": self.cfg.start_usd, "kas": a.cash, "posisi_terbuka": a.open.len(),
                "equity_harga_beli": a.ekuitas_beli(), "equity_harga_pasar": a.ekuitas_pasar(),
                "drawdown_maks_pct": 100.0 * a.dd_max, "stop_darurat": a.killed, "kelly": a.kelly(),
                "trade_dibeli": a.n_beli, "trade_selesai": a.n_tutup, "menang": a.n_menang,
            },
            "saringan": { "skor_tersimpan": a.recent.len(), "ambang_awal": a.thr0, "tolak": a.tolak, "data_basi": self.n_basi, "cek_gagal": self.n_cek_gagal },
            "kill_switch": {"aktif": crate::kill::aktif(), "alasan": crate::kill::alasan()},
            "label_latihan_ditulis": self.n_label,
            "filter": match &self.pemb {
                Some(p) => json!({"mode": "adaptif: ML CatBoost (latih ulang C++) + DL Rust", "hasil_koin_dipelajari": p.labels.len(),
                                  "ml_dilatih_ulang": p.n_latih, "sedang_melatih_ml": p.sedang_latih, "koin_dinilai": p.n_skor,
                                  "akurasi_data_belum_dilihat_500": p.akurasi().map(|(d, m)| json!({"dl_auc": d, "ml_auc": m})),
                                  "dl_aktif": p.dl_aktif, "dl_dimatikan_pengaman": p.n_dl_mati}),
                None => json!({"mode": "beku (BELAJAR=0)"}),
            },
            "log_gate": { "reread_ok": self.gate.stats.reread, "duplicate_dropped": self.gate.stats.duplicate, "outside_range_dropped": self.gate.stats.outside_range,
                          "removed_dropped": self.gate.stats.removed, "late_delivered": self.gate.stats.late, "range_cap_blocks": self.range_cap },
            "radar": { "pool_dikenal": self.radar.meta.len(), "swap_tercatat": self.radar.n_swap, "token_terklasifikasi": self.radar.kelas.len(), "label_radar": self.radar.n_label },
            "setelan": { "MAX_POS": self.cfg.max_pos, "EXP_MAX": self.cfg.exp_max, "LP_CAP": self.cfg.lp_cap, "KELLY_MAKS": self.cfg.kelly_maks,
                "KILL_DD": self.cfg.kill_dd, "QTHR": self.cfg.qthr, "DUMP_FRAC": self.cfg.dump_frac, "TRAIL_PUNCAK": self.cfg.trail, "TUTUP_SAAT_RUG": self.cfg.rug_exit },
        });
        let ringkas = format!(
            "equity ${:.2} (kas ${:.2}, {} posisi) | {} trade, menang {} | DD maks {:.1}% | lag {} blok",
            a.ekuitas_pasar(),
            a.cash,
            a.open.len(),
            a.n_tutup,
            a.n_menang,
            100.0 * a.dd_max,
            self.tip - self.last
        );
        let akun_json = a.ke_json().to_string();
        drop(a);
        tulis_aman(&self.dir.join("status.json"), &serde_json::to_string_pretty(&j).unwrap_or_default());
        tulis_aman(&self.dir.join("account_rust.json"), &akun_json);
        if killed && !self.killed_dikabarkan {
            self.killed_dikabarkan = true;
            let s = format!("STOP DARURAT aktif (drawdown > {:.0}%). Bot berhenti membeli. {ringkas}", 100.0 * self.cfg.kill_dd);
            self.log.tulis(&s);
            self.kabar.kirim(s);
        }
        if self.ringkas_t.elapsed() > Duration::from_secs(86400) {
            self.ringkas_t = Instant::now();
            self.kabar.kirim(format!("ringkasan harian: {ringkas}"));
        }
    }
    /// Checkpoint hanya saat akun tanpa posisi & tanpa pesanan: cukup simpan akun (posisi kosong = tidak ada yang hilang).
    fn coba_checkpoint(&mut self) {
        if self.last - self.cp_blk < self.cfg.cp_tiap {
            return;
        }
        let a = self.m.akun.lock().unwrap();
        if !a.open.is_empty() || !a.pending.is_empty() {
            return;
        }
        let j = json!({"block": self.last, "eth": self.eth, "akun": a.ke_json(), "t": sekarang_iso()});
        drop(a);
        tulis_aman(&self.dir.join("checkpoint.json"), &j.to_string());
        self.cp_blk = self.last;
        self.simpan_belajar();
        // bersihkan keputusan koin yang sudah tidak dipantau (memori tetap kecil)
        let sims = &self.m.sims;
        let mut a = self.m.akun.lock().unwrap();
        let open: HashSet<String> = a.open.keys().cloned().collect();
        a.kep.retain(|k, _| sims.contains_key(k) || sims.contains_key(&format!("b:{k}")) || open.contains(k));
        a.mtm.retain(|k, _| open.contains(k));
        drop(a);
        self.pool_keys.retain(|k, _| sims.contains_key(k) || open.contains(k));
    }
    /// A failed poll: keep the error for status.json, log it at most every 10 s, then back off.
    fn poll_error(&mut self, e: &str, backoff: &mut Duration) {
        self.last_err = format!("{}: {e}", self.rpc.current_host()).chars().take(160).collect();
        if self.t_err_log.elapsed() > Duration::from_secs(10) {
            self.t_err_log = Instant::now();
            self.log.tulis(&format!("error: {}", self.last_err));
        }
        std::thread::sleep(*backoff);
        *backoff = (*backoff * 2).min(POLL_BACKOFF_MAX);
    }
    /// One getLogs answer for `[from, to]` (every block `<= last` already delivered) -> the events to deliver: each
    /// chain log exactly once; logs a lagging node returned only now go first into block `last + 1`, in chain order.
    fn terima_log(&mut self, logs: &[Value], from: u64, last: u64, to: u64) -> Vec<Ev> {
        use crate::loggate::{late_log_index, Admit};
        let mut fresh = Vec::new();
        let mut late = Vec::new();
        for l in logs {
            let (blk, li) = (hexnum(&l["blockNumber"]), hexnum(&l["logIndex"]));
            if !(blk.is_finite() && li.is_finite() && blk >= 0.0 && li >= 0.0) {
                continue;
            }
            let removed = l["removed"].as_bool().unwrap_or(false);
            match self.gate.classify(blk as u64, li as u64, removed, from, last, to) {
                Admit::New => fresh.extend(dekode(l)),
                Admit::Late => late.extend(dekode(l)),
                Admit::Drop => {}
            }
        }
        if !late.is_empty() {
            late.sort_by(|a, b| a.blk().partial_cmp(&b.blk()).unwrap().then(a.li().partial_cmp(&b.li()).unwrap()));
            let n = late.len();
            self.log.tulis(&format!("log gate: {n} log(s) from block {} on that a lagging RPC node had not returned are delivered at block {}", late[0].blk(), last + 1));
            for (rank, e) in late.iter_mut().enumerate() {
                e.restamp((last + 1) as f64, late_log_index(rank, n));
            }
            fresh.extend(late);
        }
        fresh
    }
    fn proses_daftar(&mut self, mut ev: Vec<Ev>) {
        ev.sort_by(|a, b| a.blk().partial_cmp(&b.blk()).unwrap().then(a.li().partial_cmp(&b.li()).unwrap()));
        let mut i = 0;
        while i < ev.len() {
            let b = ev[i].blk();
            let mut j = i;
            while j < ev.len() && ev[j].blk() == b {
                j += 1;
            }
            let grp: Vec<Ev> = ev[i..j].to_vec();
            self.proses_blok(b, &grp);
            i = j;
        }
    }
    fn cek_rpc_sehat(&mut self) {
        let diam = self.ok_poll.elapsed();
        if diam > Duration::from_secs(60) {
            if !self.aman {
                self.aman = true;
                self.pulih_sejak = None;
                let s = format!("MODE AMAN AKTIF: RPC tidak sehat {} dtk. BERHENTI MEMBELI. Posisi terbuka tetap dipantau.", diam.as_secs());
                self.log.tulis(&s);
                self.kabar.kirim(s);
            }
        } else if self.aman {
            match self.pulih_sejak {
                None => self.pulih_sejak = Some(Instant::now()),
                Some(t) if t.elapsed() > Duration::from_secs(30) => {
                    self.aman = false;
                    self.log.tulis("MODE AMAN SELESAI: RPC sehat kembali. Boleh membeli lagi.");
                    self.kabar.kirim("RPC sehat kembali, mode aman selesai.".into());
                }
                _ => {}
            }
        }
    }
    /// Umpan radar dengan event v4 (setelah jalur trading memprosesnya) — radar tidak memengaruhi trading.
    fn radar_v4(&mut self, ev: &[Ev]) {
        self.radar.eth = self.eth;
        if self.radar.mulai_blk.is_nan() {
            self.radar.mulai_blk = self.last;
        }
        for e in ev {
            match e {
                Ev::Init { id, c0, c1, fee, blk, .. } => self.radar.v4_lahir(id, c0, c1, *fee, *blk),
                Ev::Swap { id, blk, a0, a1, sp, l, tx, .. } if id != ETH_POOL => self.radar.v4_swap(id, *blk, big_f64(a0), big_f64(a1), big_f64(sp), big_f64(l), tx),
                _ => {}
            }
        }
    }
    fn radar_latar(&mut self) {
        while let Ok(h) = self.hasil_rx.try_recv() {
            match h {
                HasilLatar::Meta(p, Some(m)) => self.radar.meta_satu(p, m),
                HasilLatar::Meta(p, None) => self.radar.lupakan_meta(&p),
                HasilLatar::Kelas(_, Ok(v)) => {
                    let mut kc = self.kc.lock().unwrap();
                    for (t, k) in v {
                        self.radar.kelas.insert(t.clone(), k);
                        kc.peta.insert(t, k);
                    }
                }
                HasilLatar::Kelas(t, Err(_)) => {
                    for x in t {
                        self.radar.lupakan_kelas(&x);
                    }
                }
                HasilLatar::Desimal(_, Some(v)) => self.radar.desimal_masuk(v),
                HasilLatar::Desimal(t, None) => {
                    for x in t {
                        self.radar.lupakan_desimal(&x);
                    }
                }
                HasilLatar::MetaBanyak(v, asal) => {
                    self.log.tulis(&format!("radar: {} meta {} dimuat sekaligus", v.len(), asal));
                    self.radar.meta_banyak(v);
                }
                HasilLatar::Ds(pairs, tanda) => {
                    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
                    for p in &pairs {
                        self.radar.gabung_ds(p, ms);
                    }
                    for (t, b, pr) in tanda {
                        self.radar.tandai_token_ds(&t, b, pr);
                    }
                }
            }
        }
        // kelas dari cache bersama (hasil cek trading) ikut dipakai radar
        {
            let kc = self.kc.lock().unwrap();
            for (t, k) in kc.peta.iter() {
                self.radar.kelas.entry(t.clone()).or_insert(*k);
            }
        }
        // pabrik baru -> semua pool-nya diambil sekaligus (sekali per pabrik)
        while let Some((f, v3)) = self.radar.perlu_pabrik.pop_front() {
            let _ = self.minta_tx.send(Minta::Pabrik(f, v3, self.last as u64));
        }
        for _ in 0..6 {
            let Some(p) = self.radar.perlu_meta.pop_front() else { break };
            let lama = self.radar.meta.get(&p).cloned();
            let _ = self.minta_tx.send(Minta::Meta(p, lama));
        }
        for _ in 0..3 {
            let t: Vec<String> = (0..50).filter_map(|_| self.radar.perlu_desimal.pop_front()).collect();
            if t.is_empty() {
                break;
            }
            let _ = self.minta_tx.send(Minta::Desimal(t));
        }
        for _ in 0..2 {
            let t: Vec<String> = (0..50).filter_map(|_| self.radar.perlu_kelas.pop_front()).filter(|t| !self.radar.kelas.contains_key(t)).collect();
            if t.is_empty() {
                break;
            }
            let _ = self.minta_tx.send(Minta::Kelas(t));
        }
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0);
        *self.ds_mau.lock().unwrap() = self.radar.perlu_ds(ms, 30);
    }
    fn simpan_belajar(&mut self) {
        if let Some(p) = &self.pemb {
            match p.simpan(&self.dir.join("belajar.json")) {
                Ok(()) => self.label_disimpan = p.labels.len(),
                Err(e) => {
                    self.log.tulis(&format!("simpan keadaan filter adaptif gagal: {e}"));
                    crate::kill::picu(&format!("keadaan pembelajar ML/DL gagal disimpan: {e}"));
                }
            }
        }
    }
    /// Start an ML retrain in its own thread (the bot keeps trading with the current model meanwhile).
    fn mulai_latih(&mut self, alasan: &'static str) {
        let bin = self.cfg.catboost_bin.clone();
        let (tip, kerja, tx) = (self.tip, self.dir.join("catboost_kerja"), self.latih_tx.clone());
        let p = match self.pemb.as_mut() {
            Some(p) => p,
            None => return,
        };
        let d = p.data_latih(tip, alasan);
        if !Path::new(&bin).exists() {
            let _ = p.selesai_latih(Err("program CatBoost tidak ada".into()), d.n_label, d.alasan);
            self.log.tulis(&format!("latih ulang ML tidak bisa: {bin} tidak ada (pasang ulang: sudo ./rhc.sh pasang)"));
            crate::kill::picu(&format!("program CatBoost {bin} tidak ada: ML tidak bisa dilatih ulang"));
            return;
        }
        let feats = p.feats.clone();
        self.log.tulis(&format!("LATIH ULANG ML mulai ({alasan}): {} baris ({} hasil koin live)", d.x.len(), d.n_label));
        let _ = std::thread::Builder::new().name("rhc-latih".into()).spawn(move || {
            let r = belajar::latih_catboost(&bin, &kerja, &feats, &d);
            let _ = tx.send(HasilLatih { r, n_label: d.n_label, alasan: d.alasan });
        });
    }
    /// A retrain finished: the new model is used only after the Rust predictor matched CatBoost itself (checked in
    /// latih_catboost); it is also written to filter_cat.json (old one archived) so a restart keeps it.
    fn cek_latih(&mut self) {
        while let Ok(h) = self.latih_rx.try_recv() {
            let thr0 = self.m.akun.lock().unwrap().thr0;
            let p = match self.pemb.as_mut() {
                Some(p) => p,
                None => return,
            };
            match h.r {
                Ok((mut model, cek)) => {
                    let cm = CatModel::from_value(&model);
                    let n_pohon = cm.as_ref().map(|m| m.trees.len()).unwrap_or(0);
                    match p.selesai_latih(cm, h.n_label, h.alasan) {
                        Ok(()) => {
                            model["thr_start"] = json!(thr0);
                            model["waktu"] = json!(sekarang_iso());
                            model["alasan"] = json!(h.alasan);
                            model["n_label"] = json!(h.n_label);
                            let bot = PathBuf::from(&self.bot_dir);
                            let arsip = bot.join("arsip_model");
                            let _ = std::fs::create_dir_all(&arsip);
                            let tag = sekarang_iso().replace([':', '-'], "");
                            for f in ["filter_cat.json", "filter_cat_check.json"] {
                                let _ = std::fs::copy(bot.join(f), arsip.join(format!("{tag}_{f}")));
                            }
                            // check first, then the model: a restart in between still finds a matching pair
                            tulis_aman(&bot.join("filter_cat_check.json"), &cek.to_string());
                            tulis_aman(&bot.join("filter_cat.json"), &model.to_string());
                            let s = format!("MODEL ML BARU dipasang (latih ulang #{}, {}): {n_pohon} pohon, {} hasil koin live, Rust = CatBoost (selisih {:.1e})",
                                p.n_latih, h.alasan, h.n_label, cek["selisih_catboost"].as_f64().unwrap_or(f64::NAN));
                            self.log.tulis(&s);
                            self.kabar.kirim(s);
                        }
                        Err(e) => {
                            self.log.tulis(&format!("model ML baru DITOLAK ({e}); model lama tetap dipakai"));
                            crate::kill::picu(&format!("model ML hasil latih ulang ditolak: {e}"));
                        }
                    }
                }
                Err(e) => {
                    let _ = p.selesai_latih(Err(e.clone()), h.n_label, h.alasan);
                    self.log.tulis(&format!("latih ulang ML GAGAL ({}); model lama tetap dipakai", e.chars().take(200).collect::<String>()));
                    crate::kill::picu(&format!("latih ulang ML gagal: {}", e.chars().take(200).collect::<String>()));
                }
            }
            self.simpan_belajar();
        }
    }
}

/// Thread DEX Screener (sumber terindeks, untuk penemuan & pengayaan — BUKAN untuk waktu eksekusi).
fn ds_loop(tx: mpsc::Sender<HasilLatar>, mau: Arc<Mutex<Vec<String>>>, doh: bool) {
    let host = "api.dexscreener.com";
    let mut conn: Option<Conn> = None;
    let mut t_prof: Option<Instant> = None;
    loop {
        if conn.is_none() {
            let ip = if doh {
                doh_resolve(host).ok()
            } else {
                use std::net::ToSocketAddrs;
                (host, 443).to_socket_addrs().ok().and_then(|mut a| a.next()).map(|a| a.ip())
            };
            conn = ip.map(|ip| Conn::new(host, ip));
        }
        if let Some(c) = conn.as_mut() {
            let mut get = |path: &str| -> Option<Value> { c.request(path, None, "accept: application/json\r\n", Duration::from_secs(15)).ok().and_then(|t| serde_json::from_str(&t).ok()) };
            let mut tanda: Vec<(String, f64, bool)> = Vec::new();
            if t_prof.map_or(true, |t| t.elapsed() > Duration::from_secs(60)) {
                t_prof = Some(Instant::now());
                for ep in ["/token-profiles/latest/v1", "/token-boosts/latest/v1", "/token-boosts/top/v1"] {
                    if let Some(Value::Array(a)) = get(ep) {
                        for x in a.iter().filter(|x| x["chainId"].as_str() == Some("robinhood")) {
                            if let Some(t) = x["tokenAddress"].as_str() {
                                let b = x["totalAmount"].as_f64().or(x["amount"].as_f64()).unwrap_or(0.0);
                                tanda.push((t.to_lowercase(), b, ep.contains("profiles")));
                            }
                        }
                    }
                }
            }
            let mut daftar: Vec<String> = mau.lock().unwrap().clone();
            for (t, _, _) in &tanda {
                if !daftar.contains(t) {
                    daftar.push(t.clone());
                }
            }
            daftar.truncate(30);
            let mut pairs = Vec::new();
            if !daftar.is_empty() {
                if let Some(Value::Array(a)) = get(&format!("/tokens/v1/robinhood/{}", daftar.join(","))) {
                    pairs = a;
                }
            }
            if !pairs.is_empty() || !tanda.is_empty() {
                if tx.send(HasilLatar::Ds(pairs, tanda)).is_err() {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(20));
    }
}

pub fn jalan(cfg: Cfg, bot_dir: &str, env_file: Option<&str>) -> Result<(), String> {
    // execution settings are validated before anything else runs (fail closed on a bad live configuration)
    let exec_cfg = crate::exec::config::ExecConfig::load(env_file, bot_dir, &cfg.rpc_urls, cfg.doh, cfg.max_pos)?;
    let label: &'static str = match exec_cfg.mode {
        crate::exec::config::Mode::Off => "kertas",
        crate::exec::config::Mode::Shadow => "SHADOW",
        crate::exec::config::Mode::Live => "LIVE",
    };
    let dir = PathBuf::from(bot_dir).join("rust_live");
    let store = dir.join("store");
    std::fs::create_dir_all(&store).map_err(|e| e.to_string())?;
    let log = Log { path: dir.join("live.log"), n: std::cell::Cell::new(0) };
    if dir.join("stop.txt").exists() {
        return Err("stop.txt ada — hapus dulu untuk menjalankan bot".into());
    }
    // TOTAL KILL SWITCH (kill.rs): read first, so a switch left on by a crash keeps every order blocked
    let kill_lama = crate::kill::siapkan(&dir);
    let lp = dir.join("live.log");
    std::panic::set_hook(Box::new(move |info| {
        let nama = std::thread::current().name().unwrap_or("").to_string();
        tambah(&lp, &format!("[{}] PANIK (thread '{nama}'): {info}", &sekarang_iso()[..19]));
        // side threads that never decide or send a trade (pool radar, DEX Screener, Telegram) only log; any other
        // panic is an error of our trading system: stop everything
        if !(nama.starts_with("rhc-radar") || nama == "rhc-ds" || nama == "rhc-kabar") {
            crate::kill::picu(&format!("panic di thread '{}': {info}", if nama.is_empty() { "pengecekan koin" } else { nama.as_str() }));
        }
    }));
    // model + cek kesamaan model (WAJIB, sama dengan paper_live.js)
    let (cat, thr0) = muat_model_filter(bot_dir)?;
    // adaptive filter: fail closed on its data (the installer ships latih_dasar.csv), so the bot never trades on a
    // half-built filter
    let pemb = if cfg.belajar && cfg.filter {
        let dasar = if cfg.latih_dasar.is_empty() { format!("{bot_dir}/latih_dasar.csv") } else { cfg.latih_dasar.clone() };
        let (dx, dy) = belajar::baca_dasar(Path::new(&dasar), &cat.feats)?;
        let ml = CatModel::load(&format!("{bot_dir}/filter_cat.json")).map_err(|e| e.to_string())?;
        let mut p = Pembelajar::baru(ml, dx, dy);
        p.label_baru = cfg.belajar_label_baru;
        let labels = baca_label(&dir.join("labels.jsonl"), &p.feats);
        let dikejar = p.pulihkan(&dir.join("belajar.json"), labels)?;
        Some((p, dikejar))
    } else {
        None
    };
    let model = Model::load(&format!("{bot_dir}/danger_model.json"))?;
    cek_parity(bot_dir, "p_bahaya", &|x| model.predict(x), "danger_check.json")?;
    let model_up = Model::load(&format!("{bot_dir}/pnaik_model.json")).ok();
    if let Some(mu) = &model_up {
        cek_parity(bot_dir, "p_naik", &|x| mu.predict(x), "pnaik_check.json")?;
    }
    let rpc = Rpc::new(&cfg.rpc_urls, cfg.doh)?;
    let mut akun = Akun::new(cfg.clone(), thr0);
    let kabar = Kabar { token: cfg.telegram_token.clone(), chat: cfg.telegram_chat.clone(), doh: cfg.doh, label };
    // live executor (started before the replay so a bad wallet/RPC setup stops the bot before it trades)
    let exec = {
        let k = kabar.clone();
        let logp = dir.join("live.log");
        let alert: crate::exec::Alert = Arc::new(move |msg: String| {
            tambah(&logp, &format!("[{}] EXEC: {msg}", &sekarang_iso()[..19]));
            k.kirim(format!("EXEC: {msg}"));
        });
        crate::exec::start(exec_cfg, alert)?
    };
    // radar: cache klasifikasi & meta pool, 4 pekerja RPC latar, thread DEX Screener
    let kc = Arc::new(Mutex::new(kelas::Cache::muat(&dir.join("kelas_token.json"))));
    let (minta_tx, minta_rx) = mpsc::channel::<Minta>();
    let (hasil_tx, hasil_rx) = mpsc::channel::<HasilLatar>();
    let minta_rx = Arc::new(Mutex::new(minta_rx));
    let laju = Arc::new(radar::Laju::new());
    // the radar never trades: its background reads go to the last endpoint first (the public RPC), so its wide scans
    // do not use up the paid RPC's quota, which the trading path needs
    let rpc_radar = Rpc::new(&cfg.rpc_urls.split(',').rev().collect::<Vec<_>>().join(","), cfg.doh)?;
    for _ in 0..2 {
        let (rx, tx2, rpc2, laju2) = (minta_rx.clone(), hasil_tx.clone(), rpc_radar.clone(), laju.clone());
        let _ = std::thread::Builder::new().name("rhc-radar".into()).spawn(move || loop {
            let m = { rx.lock().unwrap().recv() };
            let r = match m {
                Ok(Minta::Meta(p, lama)) => {
                    let r = radar::ambil_meta(&rpc2, &laju2, &p, lama);
                    HasilLatar::Meta(p, r)
                }
                Ok(Minta::Kelas(t)) => {
                    laju2.tunggu();
                    let r = kelas::klasifikasi_batch(&rpc2, &t);
                    if let Err(e) = &r {
                        if e.contains("429") {
                            laju2.mundur();
                        }
                    }
                    HasilLatar::Kelas(t, r)
                }
                Ok(Minta::Desimal(t)) => {
                    let r = radar::ambil_desimal(&rpc2, &laju2, &t);
                    HasilLatar::Desimal(t, r)
                }
                Ok(Minta::Pabrik(f, v3, sampai)) => {
                    let r = radar::boot_pabrik(&rpc2, &laju2, &f, v3, sampai);
                    HasilLatar::MetaBanyak(r, format!("pabrik {}", &f[..10.min(f.len())]))
                }
                Ok(Minta::BootV4(dari, sampai)) => {
                    let r = radar::boot_v4(&rpc2, &laju2, dari, sampai);
                    HasilLatar::MetaBanyak(r, "pool v4".into())
                }
                Err(_) => return,
            };
            if tx2.send(r).is_err() {
                return;
            }
        });
    }
    let ds_mau = Arc::new(Mutex::new(Vec::<String>::new()));
    if std::env::var("DS").map(|v| v == "1").unwrap_or(false) {
        let (tx3, mau, doh) = (hasil_tx.clone(), ds_mau.clone(), cfg.doh);
        let _ = std::thread::Builder::new().name("rhc-ds".into()).spawn(move || ds_loop(tx3, mau, doh));
    }
    let mut radar_obj = Radar::new();
    radar_obj.mulai_blk = f64::NAN;
    radar_obj.muat_meta(&dir.join("radar_meta.json"));
    {
        let hari: f64 = std::env::var("RADAR_BOOT_HARI").ok().and_then(|s| s.parse().ok()).unwrap_or(if radar_obj.meta.len() > 1000 { 2.0 } else { 14.0 });
        let tip = hexnum(&rpc.call("eth_blockNumber", json!([]))?);
        let _ = minta_tx.send(Minta::BootV4((tip - hari * 864000.0).max(0.0) as u64, tip as u64));
        for m in radar_obj.meta.values() {
            if !m.pabrik.is_empty() {
                radar_obj.pabrik_dikenal.insert(m.pabrik.clone());
            }
        }
    }
    // checkpoint (akun saat tanpa posisi) -> putar ulang cukup dari sana
    let cp: Value = std::fs::read_to_string(dir.join("checkpoint.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let cp_blk = cp["block"].as_f64().unwrap_or(f64::NEG_INFINITY);
    if cp_blk.is_finite() {
        akun.dari_json(&cp["akun"]);
    }
    // known rugging devs from the shipped history, merged before the replay so live and replay see the same history.
    // Fail closed: with DEV_RULE=1 a missing or broken file would leave the rug rule blind.
    let riwayat_dev = if cfg.dev_rule { Some(muat_riwayat_dev(&mut akun, &PathBuf::from(bot_dir).join("dev_history.json"))?) } else { None };
    let mut m = Mesin::new(akun, model, model_up);
    // the adaptive filter learns from the shadow trades' outcomes
    m.bayangan = cfg.label || pemb.is_some();
    m.emit_orders = exec.is_some();
    let (tx, rx) = mpsc::channel();
    let (latih_tx, latih_rx) = mpsc::channel();
    let pemb_info = pemb.as_ref().map(|(p, n)| (p.labels.len(), *n, p.n_latih));
    let mut b = Live {
        cfg: cfg.clone(),
        bot_dir: bot_dir.to_string(),
        dir: dir.clone(),
        store: store.clone(),
        log,
        kabar,
        m,
        eth: cp["eth"].as_f64().unwrap_or(f64::NAN),
        replaying: false,
        salin_ke: None,
        sudah_dicek: HashSet::new(),
        keputusan_simpan: VecDeque::new(),
        tx,
        rx,
        rpc: rpc.clone(),
        cat: Arc::new(cat),
        label_disimpan: pemb.as_ref().map_or(0, |(p, _)| p.labels.len()),
        pemb: pemb.map(|(p, _)| p),
        latih_tx,
        latih_rx,
        last: 0.0,
        tip: 0.0,
        cp_blk: if cp_blk.is_finite() { cp_blk } else { 0.0 },
        ok_poll: Instant::now(),
        aman: false,
        pulih_sejak: None,
        n_basi: 0,
        n_cek_gagal: 0,
        n_label: 0,
        mulai: Instant::now(),
        last_err: String::new(),
        killed_dikabarkan: false,
        ringkas_t: Instant::now(),
        radar: radar_obj,
        kc,
        minta_tx,
        hasil_rx,
        ds_mau,
        exec,
        pool_keys: HashMap::new(),
        gate: crate::loggate::LogGate::new(crate::loggate::OVERLAP_BLOCKS),
        range_cap: cfg.chunk.max(2 * crate::loggate::OVERLAP_BLOCKS),
        t_err_log: Instant::now() - Duration::from_secs(60),
        aman_simpan: HashSet::new(),
        eth_tolak_sejak: f64::NAN,
        dev_pra: Arc::new(Mutex::new(HashMap::new())),
    };
    match riwayat_dev {
        Some((n, rug)) => b.log.tulis(&format!("aturan dev AKTIF: {n} dev dari dev_history.json, {rug} dev tercatat pernah rug (tidak akan dibeli)")),
        None => b.log.tulis("PERINGATAN: DEV_RULE=0 — aturan \"dev pernah rug\" MATI"),
    }
    b.tip = hexnum(&rpc.call("eth_blockNumber", json!([]))?);
    let state_p = dir.join("state.json");
    if !state_p.exists() {
        let start = std::env::var("START_BLOCK").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(b.tip);
        // harga ETH awal: swap terakhir pool ETH/USDG sebelum blok mulai
        // newest ETH/USDG swap before `start`, searched backwards 20,000 blocks at a time in 1000-block parts
        let mut seed: Vec<Value> = Vec::new();
        let mut hi = (start - 1.0).max(0.0) as u64;
        while seed.is_empty() && hi > 0 && (start as u64).saturating_sub(hi) <= 520_000 {
            let lo = hi.saturating_sub(19_999);
            // a public RPC answering 429 right at start must not stop the bot: retry with backoff for 3 minutes
            seed = ulangi(Instant::now() + Duration::from_secs(180), || rpc.logs_ranged(&json!({"address": PM, "topics": [T_SWAP, ETH_POOL]}), lo, hi, 1000, 20_000))?;
            hi = lo.saturating_sub(1);
        }
        let e = seed.last().and_then(dekode).ok_or("harga ETH awal tidak ketemu")?;
        b.simpan_event(&e);
        tulis_aman(&state_p, &json!({"start": start, "last": start - 1.0}).to_string());
        let mode_txt = match b.kabar.label {
            "LIVE" => "Mode LIVE: transaksi nyata dengan dompet EXEC_KEY_FILE.",
            "SHADOW" => "Mode SHADOW: transaksi disimulasikan, tidak dikirim.",
            _ => "Mode kertas: tidak ada transaksi.",
        };
        b.log.tulis(&format!("MULAI paper trading ({VERSI}) dari blok {start} (tip {}). {mode_txt}", b.tip));
    }
    let state: Value = serde_json::from_str(&std::fs::read_to_string(&state_p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    b.last = state["last"].as_f64().ok_or("state.json rusak")?;
    // ---------- putar ulang (tanpa jaringan) ----------
    if let Ok(t) = std::fs::read_to_string(dir.join("decisions.jsonl")) {
        for l in t.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(l) {
                if let (Some(p), Some(pada)) = (v["pool"].as_str(), v["pada"].as_f64()) {
                    if pada > b.cp_blk {
                        b.keputusan_simpan.push_back((pada, p.to_string(), v.clone()));
                    }
                } else if let Some(x) = v["aman"].as_f64() {
                    if x > b.cp_blk {
                        b.aman_simpan.insert(x as u64);
                    }
                }
            }
        }
    }
    let besar: u64 = ["pools.csv", "swaps.csv", "liq.csv"].iter().filter_map(|f| std::fs::metadata(store.join(f)).ok()).map(|m| m.len()).sum();
    let padatkan = b.cp_blk > 0.0 && besar > cfg.kompaksi_mb * 1_000_000;
    if padatkan {
        let baru = dir.join("store_baru");
        let _ = std::fs::remove_dir_all(&baru);
        std::fs::create_dir_all(&baru).map_err(|e| e.to_string())?;
        b.salin_ke = Some(baru);
    }
    b.replaying = true;
    let mut al = Aliran::buka(&store);
    let (mut grp, mut cur, mut n_ev, mut n_dup): (Vec<Ev>, f64, u64, u64) = (Vec::new(), f64::NAN, 0, 0);
    while let Some(e) = al.berikut() {
        if e.blk() > b.last {
            break;
        }
        // harga ETH terakhir sebelum checkpoint tetap disalin (dibutuhkan saat mulai lagi tanpa checkpoint)
        if e.blk() <= b.cp_blk {
            if let Ev::Swap { id, sp, .. } = &e {
                if id == ETH_POOL {
                    let s = big_f64(sp) / Q96;
                    if !b.eth.is_finite() {
                        b.eth = s * s * 1e12;
                    }
                }
            }
            continue;
        }
        if !b.eth.is_finite() {
            if let Ev::Swap { id, sp, .. } = &e {
                if id == ETH_POOL {
                    let s = big_f64(sp) / Q96;
                    b.eth = s * s * 1e12;
                }
            }
        }
        if e.blk() != cur && !grp.is_empty() {
            let g = once_each(std::mem::take(&mut grp), &mut n_dup);
            b.proses_blok(cur, &g);
        }
        cur = e.blk();
        grp.push(e);
        n_ev += 1;
    }
    if !grp.is_empty() {
        let g = once_each(std::mem::take(&mut grp), &mut n_dup);
        b.proses_blok(cur, &g);
    }
    if n_dup > 0 {
        b.log.tulis(&format!("replay: {n_dup} duplicate store rows (written by an older version) were replayed once"));
    }
    b.replaying = false;
    b.keputusan_simpan.clear();
    // real positions the strategy no longer holds (it exited while the bot was down) are sold now
    if let Some(ex) = &b.exec {
        let open: Vec<String> = b.m.akun.lock().unwrap().open.keys().cloned().collect();
        ex.reconcile(open);
    }
    if let Some(baru) = b.salin_ke.take() {
        // pemadatan: store lama diarsipkan (bahan uji ulang/latih ulang), store baru hanya berisi data sesudah checkpoint
        let arsip = dir.join(format!("arsip_store_{}", b.cp_blk as u64));
        let _ = std::fs::rename(&store, &arsip);
        let _ = std::fs::rename(&baru, &store);
        // harga ETH terakhir ditulis supaya store baru mandiri
        b.log.tulis(&format!("store dipadatkan ({} MB); yang lama diarsipkan ke {}", besar / 1_000_000, arsip.display()));
        // only the newest archive stays: the disk must not fill up (a full disk stops the ledger, then the kill switch)
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("arsip_store_") && e.path() != arsip) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    {
        let a = b.m.akun.lock().unwrap();
        b.log.tulis(&format!(
            "putar ulang {n_ev} event sejak blok {} -> {} trade tutup total, {} posisi terbuka, kas ${:.2}. Lanjut dari blok {}.",
            b.cp_blk,
            a.n_tutup,
            a.open.len(),
            a.cash,
            b.last + 1.0
        ));
    }
    match pemb_info {
        Some((n, dikejar, n_latih)) => b.log.tulis(&format!(
            "filter adaptif AKTIF: ML CatBoost + DL | {n} hasil koin sudah dipelajari ({dikejar} dikejar sekarang), ML sudah dilatih ulang {n_latih}x | program CatBoost {}: {}",
            cfg.catboost_bin,
            if Path::new(&cfg.catboost_bin).exists() { "ada" } else { "TIDAK ADA (ML tidak bisa dilatih ulang, DL tetap belajar)" }
        )),
        None => b.log.tulis("PERINGATAN: BELAJAR=0 — filter beku (model bawaan, tidak belajar)"),
    }
    if b.pemb.is_some() && !Path::new(&cfg.catboost_bin).exists() {
        crate::kill::picu(&format!("program CatBoost {} tidak ada: filter ML tidak bisa dilatih ulang (pasang ulang: sudo ./rhc.sh pasang)", cfg.catboost_bin));
    }
    if let Some(r) = &kill_lama {
        b.log.tulis(&format!("KILL SWITCH MASIH AKTIF (sejak {r}). Bot TIDAK mengirim order apa pun. Cek: sudo ./rhc.sh status | lanjutkan: sudo ./rhc.sh lanjut"));
    }
    b.simpan_belajar();
    b.kabar.kirim(format!("bot mulai ({VERSI}), blok {}", b.last + 1.0));
    b.status();
    let mut t_status = Instant::now();
    let mut t_radar = Instant::now();
    let mut t_simpan = Instant::now();
    let mut t_meta = Instant::now();
    let mut backoff = POLL_BACKOFF_MIN;
    let mut kill_dicatat = false;
    let mut uji_panik = cfg.uji_panik_detik == 0;
    loop {
        if dir.join("stop.txt").exists() {
            b.log.tulis("stop.txt ditemukan -> berhenti dengan rapi.");
            b.simpan_belajar();
            b.status();
            return Ok(());
        }
        // TOTAL KILL SWITCH: the strategy stops too (no new decisions, no orders); status.json keeps saying why
        if crate::kill::aktif() {
            if !kill_dicatat {
                kill_dicatat = true;
                b.log.tulis(&format!("KILL SWITCH AKTIF: {} | semua order dihentikan. Cek: sudo ./rhc.sh status | lanjutkan: sudo ./rhc.sh lanjut", crate::kill::alasan().unwrap_or_default()));
                b.simpan_belajar();
            }
            b.status();
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        // test only (UJI_PANIK_DETIK): an injected error proves the kill switch on a running bot
        if !uji_panik && b.mulai.elapsed().as_secs() >= cfg.uji_panik_detik {
            uji_panik = true;
            b.log.tulis("UJI: panic sengaja dipicu (UJI_PANIK_DETIK) — kill switch harus menyala");
            let _ = std::thread::Builder::new().name("rhc-uji".into()).spawn(|| panic!("UJI: panic sengaja (UJI_PANIK_DETIK)"));
        }
        // 1) tip; 2) logs for an EXPLICIT block range, re-reading the last blocks (loggate.rs). A node that does not
        // have `to` yet answers with an error (no partial answer is taken as complete); repeats and out-of-range logs
        // from lagging nodes are dropped, so every chain log reaches the engine exactly once and in order.
        let tip_ok = match rpc.call_once("eth_blockNumber", json!([]), Duration::from_secs(5)) {
            Ok(v) => {
                let tip = hexnum(&v);
                if tip.is_finite() && tip >= b.last {
                    b.tip = tip;
                }
                true
            }
            Err(e) => {
                b.poll_error(&e, &mut backoff);
                false
            }
        };
        let last = b.last as u64;
        if tip_ok && b.tip > b.last {
            let from = b.gate.from_block(last);
            let to = (b.tip as u64).min(from + b.range_cap - 1);
            let r = rpc.batch(
                &[
                    ("eth_getLogs", json!([{"address": PM, "topics": [[T_INIT, T_SWAP, T_MOD]], "fromBlock": format!("{from:#x}"), "toBlock": format!("{to:#x}")}])),
                    ("eth_getLogs", json!([{"topics": radar::topik_radar(), "fromBlock": format!("{:#x}", last + 1), "toBlock": format!("{to:#x}")}])),
                ],
                Duration::from_secs(20),
            );
            match r {
                Ok(v) if v.len() == 2 => match &v[0] {
                    Ok(ls) => {
                        let ev = b.terima_log(ls.as_array().map(|a| a.as_slice()).unwrap_or(&[]), from, last, to);
                        // radar: every DEX's events (a failure is skipped, it never holds trading up)
                        let radar_logs: Vec<Value> = v[1].as_ref().ok().and_then(|x| x.as_array().cloned()).unwrap_or_default();
                        b.radar_v4(&ev);
                        b.proses_daftar(ev);
                        for l in radar_logs.iter().filter(|l| {
                            let k = hexnum(&l["blockNumber"]);
                            k > last as f64 && k <= to as f64 && !l["removed"].as_bool().unwrap_or(false)
                        }) {
                            b.radar.log_lain(l);
                        }
                        b.gate.delivered(to);
                        b.last = to as f64;
                        tulis_aman(&state_p, &json!({"start": state["start"], "last": b.last}).to_string());
                        b.ok_poll = Instant::now();
                        b.last_err.clear();
                        backoff = POLL_BACKOFF_MIN;
                        b.coba_checkpoint();
                    }
                    Err(e) => {
                        let floor = 2 * crate::loggate::OVERLAP_BLOCKS;
                        if crate::net::is_range_error(e) && b.range_cap > floor {
                            b.range_cap = (b.range_cap / 2).max(floor);
                            b.log.tulis(&format!("RPC limits the getLogs block range: now {} blocks per request", b.range_cap));
                        } else {
                            // e.g. -32014 "toBlock not yet available on the node": retried after a short wait
                            b.poll_error(e, &mut backoff);
                        }
                    }
                },
                Ok(v) => b.poll_error(&format!("unexpected batch answer {:?}", v.first()), &mut backoff),
                Err(e) => b.poll_error(&e, &mut backoff),
            }
        }
        // hasil cek yang datang di antara polling berlaku mulai blok BERIKUTNYA
        let lb = b.last + 0.5;
        b.ambil_hasil(lb);
        b.cek_rpc_sehat();
        if t_status.elapsed() > Duration::from_secs(5) {
            b.status();
            t_status = Instant::now();
        }
        b.cek_latih();
        if t_radar.elapsed() > Duration::from_secs(10) {
            b.radar_latar();
            {
                let a = b.m.akun.lock().unwrap();
                for (k, kp) in a.kep.iter() {
                    b.radar.skor_ml.insert(k.clone(), kp.score);
                }
            }
            let lb2 = b.last;
            b.radar.tulis(&dir, lb2);
            t_radar = Instant::now();
        }
        if t_simpan.elapsed() > Duration::from_secs(300) {
            b.kc.lock().unwrap().simpan(&dir.join("kelas_token.json"));
            t_simpan = Instant::now();
        }
        // the radar's pool list reaches 190 MB: written by its own thread, so polling (and every exit) never waits for it
        if t_meta.elapsed() > Duration::from_secs(1800) {
            t_meta = Instant::now();
            if let Some((m, d)) = b.radar.salinan_meta() {
                let p = dir.join("radar_meta.json");
                let _ = std::thread::Builder::new().name("rhc-radar-simpan".into()).spawn(move || Radar::simpan_meta(&m, &d, &p));
            }
        }
        std::thread::sleep(Duration::from_millis(cfg.poll_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_dev_history_is_merged_once_and_matched_lowercase() {
        let p = std::env::temp_dir().join(format!("rhc_dev_history_{}.json", std::process::id()));
        std::fs::write(&p, r#"{"0xAbC": [1.2, 0.1], "0xdef": [2.0]}"#).unwrap();
        let mut a = Akun::new(Cfg::load(None), 0.5);
        a.dev_hist.insert("0xdef".into(), vec![0.9]); // learned live: kept
        assert_eq!(muat_riwayat_dev(&mut a, &p).unwrap(), (2, 1));
        assert_eq!(muat_riwayat_dev(&mut a, &p).unwrap(), (2, 1), "loading again (every restart) adds nothing");
        assert_eq!(a.dev_hist["0xabc"], vec![1.2, 0.1], "addresses are matched lowercase, like the live lookup");
        assert_eq!(a.dev_hist["0xdef"], vec![0.9, 2.0]);
        let _ = std::fs::remove_file(&p);
        // the file the installer copies to /opt/rhc/bot
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/dev_history.json");
        let mut b = Akun::new(Cfg::load(None), 0.5);
        let (n, rug) = muat_riwayat_dev(&mut b, &shipped).unwrap();
        assert!(n >= 600 && rug >= 50, "models/dev_history.json: {n} devs, {rug} ruggers");
        assert!(muat_riwayat_dev(&mut b, Path::new("/tidak/ada.json")).is_err(), "a missing file must stop the bot");
    }
}
