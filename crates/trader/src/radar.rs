//! RADAR PASAR multi-sumber (Robinhood Chain 4663). TIDAK membeli apa pun: hanya memantau, menggabung, memberi peringkat,
//! dan mengumpulkan data latihan (snapshot berkala + label masa depan). Strategi trading v4 berjalan terpisah (bot.rs).
//!  - Sumber on-chain real-time (prioritas): event Swap semua DEX — Uniswap v4 (PoolManager), Uniswap v3 & turunannya
//!    (Ramses/Up/Alandale), PancakeSwap-v3 (Giga), Uniswap v2 (0swap) — plus PoolCreated/PairCreated & Sync.
//!  - Sumber terindeks: DEX Screener (chainId "robinhood"): simbol, market cap, umur pasangan, boost, profil.
//!  - GMGN: menolak akses program (HTTP 403) -> belum dipakai.
//! Kunci gabung (dedupe): alamat token (huruf kecil) dan id pool (v4 = PoolId 32 byte, lainnya = alamat kontrak pool).
use crate::kelas::Kelas;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::Path;

pub const V3_SWAP: &str = "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67";
pub const PCS3_SWAP: &str = "0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83";
pub const V2_SWAP: &str = "0xd78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822";
pub const V2_SYNC: &str = "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1";
pub const V3_CREATED: &str = "0x783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118";
pub const V2_CREATED: &str = "0x0d3648bd0f6ba80134a33ba9275ac585d9d315f0ad8355cddefde31afa28d0e9";
pub const V4_SWAP: &str = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f";
pub const V4_INIT: &str = "0xdd466e674ea557f56295e2d0218a125ea4b4f0f6f3307b95f85e6110838d6438";
pub const PM: &str = "0x8366a39cc670b4001a1121b8f6a443a643e40951";
pub const NATIVE: &str = "0x0000000000000000000000000000000000000000";
pub const WETH: &str = "0x0bd7d308f8e1639fab988df18a8011f41eacad73";
pub const USDG: &str = "0x5fc5360d0400a0fd4f2af552add042d716f1d168";
const Q96: f64 = 79228162514264337593543950336.0;
const MENIT: f64 = 600.0; // blok per menit (0,1 dtk/blok)
const HORIZON: f64 = 216000.0; // label 6 jam

/// hash 64-bit sederhana (FNV-1a) untuk menghitung pelaku unik tanpa menyimpan alamat
pub fn hash_pelaku(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn topik_radar() -> Value {
    json!([[V3_SWAP, PCS3_SWAP, V2_SWAP, V2_SYNC, V3_CREATED, V2_CREATED]])
}

fn word(data: &str, k: usize) -> &str {
    let d = data.trim_start_matches("0x");
    d.get(64 * k..64 * (k + 1)).unwrap_or("")
}
/// kata ABI -> f64 (bertanda kalau `signed`); presisi f64 cukup untuk statistik radar
fn w_f64(w: &str, signed: bool) -> f64 {
    if w.len() != 64 {
        return f64::NAN;
    }
    let neg = signed && w.as_bytes()[0] >= b'8';
    let mut v = 0.0f64;
    for c in w.bytes() {
        let d = (c as char).to_digit(16).unwrap_or(0) as f64;
        v = v * 16.0 + d;
    }
    if neg {
        v - 2f64.powi(256)
    } else {
        v
    }
}
fn addr(t: &str) -> String {
    format!("0x{}", &t[t.len().saturating_sub(40)..]).to_lowercase()
}
fn hexnum(v: &Value) -> f64 {
    v.as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).map(|x| x as f64).unwrap_or(f64::NAN)
}

#[derive(Clone, Debug, Default)]
pub struct MetaPool {
    pub dex: String,
    pub t0: String,
    pub t1: String,
    pub fee: f64,
    pub dec0: f64,
    pub dec1: f64,
    pub lahir: f64, // blok lahir (NaN = belum tahu)
    pub ok: bool,
    pub pabrik: String, // factory (v2/v3) untuk pengambilan massal
}

#[derive(Clone, Debug, Default)]
struct Ember {
    menit: i64,
    beli: u32,
    jual: u32,
    usd_beli: f64,
    usd_jual: f64,
    tx: u32,
    tutup: f64, // harga penutupan menit (USD per token dasar)
    pelaku: Vec<u64>, // hash pelaku unik di menit ini (agregat, tanpa profil dompet)
}

#[derive(Clone, Debug, Default)]
struct StatPool {
    ember: VecDeque<Ember>,
    harga: f64,
    likuid: f64,
    terakhir: f64,
    pertama_dilihat: f64,
    sepi_sejak: f64, // blok awal masa tidur (>= 6 jam tanpa swap) yang terakhir diamati
    bangun: f64,     // blok bangun dari tidur
    sumber: u8,      // bit0 on-chain, bit1 DEX Screener
}

#[derive(Clone, Debug, Default)]
pub struct InfoToken {
    pub simbol: String,
    pub nama: String,
    pub mcap: f64,
    pub dibuat_ms: f64,
    pub boost: f64,
    pub profil: bool,
    pub ds_waktu: f64,
}

struct Snap {
    pool: String,
    blk: f64,
    harga: f64,
    fitur: Value,
    maks1: f64,
    maks6: f64,
    min6: f64,
    akhir: f64,
}

pub struct Radar {
    pub meta: HashMap<String, MetaPool>,
    stat: HashMap<String, StatPool>,
    pub token: HashMap<String, InfoToken>,
    pub perlu_meta: VecDeque<String>,
    diminta: HashSet<String>,
    snap: Vec<Snap>,
    snap_blk: f64,
    pub eth: f64,
    pub skor_ml: HashMap<String, f64>,
    pub n_swap: u64,
    pub n_label: u64,
    /// klasifikasi token dasar: hanya Kelas::Meme yang boleh diberi peringkat / dijadikan data latih
    pub kelas: HashMap<String, Kelas>,
    pub perlu_kelas: VecDeque<String>,
    kelas_diminta: HashSet<String>,
    /// harga USD token yang terlihat di pool berharga (dipakai menilai pasangan meme/meme)
    usd_token: HashMap<String, f64>,
    /// swap yang datang sebelum meta pool siap: DITAHAN lalu diputar ulang (tidak ada transaksi yang hilang)
    tunda_v4: HashMap<String, Vec<(f64, f64, f64, f64, f64, String)>>,
    tunda_lain: HashMap<String, Vec<Value>>,
    /// semua pool yang terlihat ber-swap (termasuk yang meta/kelasnya belum siap) -> ukuran cakupan jujur
    terlihat: HashMap<String, f64>,
    /// stablecoin terdeteksi on-chain: (jumlah swap di pita $0,98-1,02, jumlah di luar pita)
    stabil: HashMap<String, (u32, u32)>,
    pub mulai_blk: f64,
    /// desimal token (cache) + antrean pengambilan massal (50 token / permintaan)
    pub desimal: HashMap<String, f64>,
    pub perlu_desimal: VecDeque<String>,
    desimal_diminta: HashSet<String>,
    /// pabrik pool v2/v3 yang sudah/akan diambil massal
    pub pabrik_dikenal: HashSet<String>,
    pub perlu_pabrik: VecDeque<(String, bool)>,
}

impl Radar {
    pub fn new() -> Radar {
        Radar {
            meta: HashMap::new(),
            stat: HashMap::new(),
            token: HashMap::new(),
            perlu_meta: VecDeque::new(),
            diminta: HashSet::new(),
            snap: Vec::new(),
            snap_blk: 0.0,
            eth: f64::NAN,
            skor_ml: HashMap::new(),
            n_swap: 0,
            n_label: 0,
            kelas: HashMap::new(),
            perlu_kelas: VecDeque::new(),
            kelas_diminta: HashSet::new(),
            usd_token: HashMap::new(),
            tunda_v4: HashMap::new(),
            tunda_lain: HashMap::new(),
            terlihat: HashMap::new(),
            stabil: HashMap::new(),
            mulai_blk: 0.0,
            desimal: [(NATIVE.to_string(), 18.0), (WETH.to_string(), 18.0), (USDG.to_string(), 6.0)].into_iter().collect(),
            perlu_desimal: VecDeque::new(),
            desimal_diminta: HashSet::new(),
            pabrik_dikenal: HashSet::new(),
            perlu_pabrik: VecDeque::new(),
        }
    }
    pub fn muat_meta(&mut self, p: &Path) {
        if let Ok(t) = std::fs::read_to_string(p) {
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&t) {
                for (k, v) in o {
                    let f = |n: &str| v[n].as_f64().unwrap_or(f64::NAN);
                    let s = |n: &str| v[n].as_str().unwrap_or("").to_string();
                    if k == "__desimal" {
                        if let Some(o) = v.as_object() {
                            for (t, d) in o {
                                if let Some(d) = d.as_f64() {
                                    self.desimal.insert(t.clone(), d);
                                }
                            }
                        }
                        continue;
                    }
                    let (d0, d1) = (f("dec0"), f("dec1"));
                    self.meta.insert(k, MetaPool { dex: s("dex"), t0: s("t0"), t1: s("t1"), fee: f("fee"), dec0: d0, dec1: d1, lahir: f("lahir"), ok: d0.is_finite() && d1.is_finite(), pabrik: s("pabrik") });
                }
            }
        }
    }
    pub fn simpan_meta(&self, p: &Path) {
        let mut o: serde_json::Map<String, Value> = self
            .meta
            .iter()
            .filter(|(_, m)| !m.t0.is_empty() && !m.t1.is_empty())
            .map(|(k, m)| (k.clone(), json!({"dex": m.dex, "t0": m.t0, "t1": m.t1, "fee": m.fee, "dec0": m.dec0, "dec1": m.dec1, "lahir": m.lahir, "pabrik": m.pabrik})))
            .collect();
        let d: serde_json::Map<String, Value> = self.desimal.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        o.insert("__desimal".into(), Value::Object(d));
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, Value::Object(o).to_string()).is_ok() {
            let _ = std::fs::rename(tmp, p);
        }
    }
    fn minta_meta(&mut self, pool: &str) {
        if let Some(m) = self.meta.get(pool) {
            if !m.t0.is_empty() && !m.t1.is_empty() {
                self.lengkapi(pool);
                return;
            }
        }
        if self.diminta.insert(pool.to_string()) {
            self.perlu_meta.push_back(pool.to_string());
        }
    }
    fn minta_desimal(&mut self, t: &str) {
        if !self.desimal.contains_key(t) && self.desimal_diminta.insert(t.to_string()) {
            self.perlu_desimal.push_back(t.to_string());
        }
    }
    /// Lengkapi desimal meta dari cache; true kalau meta siap dipakai.
    pub fn lengkapi(&mut self, pool: &str) -> bool {
        let (t0, t1) = match self.meta.get(pool) {
            Some(m) if m.ok => return true,
            Some(m) if !m.t0.is_empty() && !m.t1.is_empty() => (m.t0.clone(), m.t1.clone()),
            _ => return false,
        };
        let (d0, d1) = (self.desimal.get(&t0).cloned(), self.desimal.get(&t1).cloned());
        if d0.is_none() {
            self.minta_desimal(&t0);
        }
        if d1.is_none() {
            self.minta_desimal(&t1);
        }
        if let (Some(d0), Some(d1)) = (d0, d1) {
            let m = self.meta.get_mut(pool).unwrap();
            m.dec0 = d0;
            m.dec1 = d1;
            m.ok = true;
            return true;
        }
        false
    }
    /// Banyak meta sekaligus (pemuatan awal v4 / per pabrik). Pool yang swap-nya tertahan langsung diputar ulang.
    pub fn meta_banyak(&mut self, v: Vec<(String, MetaPool)>) {
        for (p, m) in v {
            let ganti = match self.meta.get(&p) {
                None => true,
                Some(lama) => !lama.ok && (lama.t0.is_empty() || lama.t1.is_empty()),
            };
            if ganti {
                self.meta.insert(p, m);
            }
        }
        self.coba_tunda();
    }
    pub fn desimal_masuk(&mut self, v: Vec<(String, f64)>) {
        for (t, d) in v {
            self.desimal.insert(t, d);
        }
        self.coba_tunda();
    }
    fn coba_tunda(&mut self) {
        let ids: Vec<String> = self.tunda_v4.keys().chain(self.tunda_lain.keys()).cloned().collect();
        for p in ids {
            if self.meta.contains_key(&p) && self.lengkapi(&p) {
                self.putar_tunda(&p);
            }
        }
    }
    /// meta pool v2/v3 dari RPC sudah datang: catat pabriknya untuk diambil massal sekali.
    pub fn meta_satu(&mut self, p: String, m: MetaPool) {
        if !m.pabrik.is_empty() && self.pabrik_dikenal.insert(m.pabrik.clone()) {
            self.perlu_pabrik.push_back((m.pabrik.clone(), m.dex.starts_with("v3")));
        }
        self.meta.insert(p.clone(), m);
        if self.lengkapi(&p) {
            self.putar_tunda(&p);
        }
    }
    /// harga USD satu unit token quote (ETH/WETH/USDG pasti; token lain dari propagasi harga)
    fn usd_quote(&self, q: &str) -> f64 {
        match q {
            NATIVE | WETH => self.eth,
            USDG => 1.0,
            _ => {
                if self.kelas.get(q) == Some(&Kelas::Quote) {
                    return 1.0; // stablecoin terdeteksi on-chain
                }
                self.usd_token.get(q).cloned().unwrap_or(f64::NAN)
            }
        }
    }
    /// Sisi quote pool: Some(true) = token1 quote (dasar token0), Some(false) = token0 quote. SEMUA pasangan ikut
    /// (ETH, WETH, USDG, antar-token). None = bukan objek radar: quote/quote (mis. ETH/USDG) atau ada sisi SAHAM.
    fn quote_sisi(&self, m: &MetaPool) -> Option<bool> {
        let isq = |a: &str| a == NATIVE || a == WETH || a == USDG || self.kelas.get(a) == Some(&Kelas::Quote);
        let saham = |a: &str| self.kelas.get(a) == Some(&Kelas::Ekuitas);
        if saham(&m.t0) || saham(&m.t1) {
            return None;
        }
        match (isq(&m.t0), isq(&m.t1)) {
            (false, true) => Some(true),
            (true, false) => Some(false),
            (true, true) => None,
            (false, false) => {
                // antar-token: quote = sisi yang harganya sudah diketahui; kalau dua-duanya / tidak ada -> token1
                let (u0, u1) = (self.usd_token.contains_key(&m.t0), self.usd_token.contains_key(&m.t1));
                Some(!(u0 && !u1))
            }
        }
    }
    /// v4: Initialize yang terlihat langsung (meta lengkap, desimal ETH 18; token dasar dianggap 18 sampai terbukti lain)
    pub fn v4_lahir(&mut self, id: &str, c0: &str, c1: &str, fee: f64, blk: f64) {
        self.meta.insert(id.to_string(), MetaPool { dex: "uniswap-v4".into(), t0: c0.into(), t1: c1.into(), fee, dec0: f64::NAN, dec1: f64::NAN, lahir: blk, ok: false, pabrik: String::new() });
        self.lengkapi(id);
        self.stat.entry(id.to_string()).or_default().pertama_dilihat = blk;
    }
    /// Satu catatan swap: arah dari sisi PEDAGANG (beli = pedagang menerima token dasar).
    #[allow(clippy::too_many_arguments)]
    fn catat(&mut self, pool: &str, blk: f64, dasar_masuk_pedagang: f64, quote_abs: f64, harga_quote_per_dasar: f64, likuid_quote: f64, qaddr: &str, decq: f64, dasar: &str, pelaku: u64) {
        let px = self.usd_quote(qaddr);
        let usd = if px.is_finite() { quote_abs / 10f64.powf(decq) * px } else { f64::NAN };
        let harga = if px.is_finite() { harga_quote_per_dasar * px } else { f64::NAN };
        if harga.is_finite() && harga > 0.0 && !dasar.is_empty() {
            self.usd_token.insert(dasar.to_string(), harga);
            // stablecoin: harga konsisten di $0,98-1,02 dengan likuiditas besar -> dianggap quote, bukan meme
            if (USDG == qaddr || qaddr == NATIVE || qaddr == WETH) && self.kelas.get(dasar) != Some(&Kelas::Ekuitas) {
                let liq_ok = likuid_quote.is_finite() && px.is_finite() && 2.0 * likuid_quote / 10f64.powf(decq) * px > 20000.0;
                let e = self.stabil.entry(dasar.to_string()).or_insert((0, 0));
                if (0.98..=1.02).contains(&harga) && liq_ok {
                    e.0 += 1;
                } else {
                    e.1 += 1;
                }
                if e.0 >= 20 && e.1 == 0 {
                    self.kelas.insert(dasar.to_string(), Kelas::Quote);
                }
            }
        }
        let s = self.stat.entry(pool.to_string()).or_default();
        if s.pertama_dilihat == 0.0 {
            s.pertama_dilihat = blk;
        }
        s.sumber |= 1;
        // tidur: >= 6 jam tanpa swap lalu ada swap lagi -> bangun
        if s.terakhir > 0.0 && blk - s.terakhir >= HORIZON {
            s.sepi_sejak = s.terakhir;
            s.bangun = blk;
        }
        s.terakhir = blk;
        if harga.is_finite() && harga > 0.0 {
            s.harga = harga;
        }
        if likuid_quote.is_finite() && px.is_finite() {
            s.likuid = 2.0 * likuid_quote / 10f64.powf(decq) * px;
        }
        let mnt = (blk / MENIT).floor() as i64;
        if s.ember.back().map_or(true, |e| e.menit != mnt) {
            s.ember.push_back(Ember { menit: mnt, ..Default::default() });
        }
        while s.ember.front().map_or(false, |e| e.menit < mnt - 1440) {
            s.ember.pop_front();
        }
        let e = s.ember.back_mut().unwrap();
        e.tx += 1;
        if pelaku != 0 && !e.pelaku.contains(&pelaku) {
            e.pelaku.push(pelaku);
        }
        if dasar_masuk_pedagang > 0.0 {
            e.beli += 1;
            if usd.is_finite() {
                e.usd_beli += usd;
            }
        } else {
            e.jual += 1;
            if usd.is_finite() {
                e.usd_jual += usd;
            }
        }
        if harga.is_finite() && harga > 0.0 {
            e.tutup = harga;
        }
        self.n_swap += 1;
        // perbarui label snapshot yang masih berjalan
        if harga.is_finite() && harga > 0.0 {
            for sn in self.snap.iter_mut().filter(|x| x.pool == pool) {
                let r = harga / sn.harga;
                if blk <= sn.blk + 36000.0 {
                    sn.maks1 = sn.maks1.max(r);
                }
                if blk <= sn.blk + HORIZON {
                    sn.maks6 = sn.maks6.max(r);
                    sn.min6 = sn.min6.min(r);
                    sn.akhir = r;
                }
            }
        }
    }
    /// Swap v4 dari PoolManager (data sudah didekode di live.rs). a0/a1 sisi pedagang. Pelaku = hash tx (router menutupi alamat).
    pub fn v4_swap(&mut self, id: &str, blk: f64, a0: f64, a1: f64, sp: f64, l: f64, tx: &str) {
        self.terlihat.insert(id.to_string(), blk);
        let siap = self.meta.contains_key(id) && self.lengkapi(id);
        let m = match self.meta.get(id) {
            Some(m) if siap => m.clone(),
            _ => {
                self.minta_meta(id);
                let v = self.tunda_v4.entry(id.to_string()).or_default();
                if v.len() < 400 {
                    v.push((blk, a0, a1, sp, l, tx.to_string()));
                }
                return;
            }
        };
        let Some(q1) = self.quote_sisi(&m) else { return };
        let s = sp / Q96;
        let p1per0 = s * s * 10f64.powf(m.dec0 - m.dec1);
        let (dasar, qabs, harga, lq, qa, dq, bt) = if q1 {
            (a0, a1.abs(), p1per0, l * s, m.t1.clone(), m.dec1, m.t0.clone())
        } else {
            (a1, a0.abs(), 1.0 / p1per0, l / s, m.t0.clone(), m.dec0, m.t1.clone())
        };
        self.catat(id, blk, dasar, qabs, harga, lq, &qa, dq, &bt, hash_pelaku(tx));
    }
    /// Meta pool baru tiba: putar ulang swap yang ditahan (urutan asli).
    pub fn putar_tunda(&mut self, pool: &str) {
        if let Some(v) = self.tunda_v4.remove(pool) {
            for (blk, a0, a1, sp, l, tx) in v {
                self.v4_swap(pool, blk, a0, a1, sp, l, &tx);
            }
        }
        if let Some(v) = self.tunda_lain.remove(pool) {
            for l in v {
                self.log_lain(&l);
            }
        }
    }
    /// Log non-v4 (v3, PancakeSwap-v3, v2, pembuatan pool).
    pub fn log_lain(&mut self, l: &Value) {
        let tp: Vec<String> = l["topics"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
        let Some(t0) = tp.first() else { return };
        let data = l["data"].as_str().unwrap_or("0x");
        let blk = hexnum(&l["blockNumber"]);
        let pool = l["address"].as_str().unwrap_or("").to_lowercase();
        match t0.as_str() {
            V3_CREATED if tp.len() >= 4 => {
                // PoolCreated(token0, token1, fee, tickSpacing, pool)
                let p = addr(word(data, 1));
                let m = MetaPool { dex: format!("v3:{}", &pool[..10]), t0: addr(&tp[1]), t1: addr(&tp[2]), fee: w_f64(&tp[3][2..], false), dec0: f64::NAN, dec1: f64::NAN, lahir: blk, ok: false, pabrik: pool.clone() };
                self.meta.insert(p.clone(), m);
                self.lengkapi(&p);
                self.stat.entry(p).or_default().pertama_dilihat = blk;
            }
            V2_CREATED if tp.len() >= 3 => {
                let p = addr(word(data, 0));
                let m = MetaPool { dex: format!("v2:{}", &pool[..10]), t0: addr(&tp[1]), t1: addr(&tp[2]), fee: 3000.0, dec0: f64::NAN, dec1: f64::NAN, lahir: blk, ok: false, pabrik: pool.clone() };
                self.meta.insert(p.clone(), m);
                self.lengkapi(&p);
                self.stat.entry(p).or_default().pertama_dilihat = blk;
            }
            V3_SWAP | PCS3_SWAP => {
                self.terlihat.insert(pool.clone(), blk);
                let siap = self.meta.contains_key(&pool) && self.lengkapi(&pool);
                let m = match self.meta.get(&pool) {
                    Some(m) if siap => m.clone(),
                    _ => {
                        self.minta_meta(&pool);
                        let v = self.tunda_lain.entry(pool.clone()).or_default();
                        if v.len() < 400 {
                            v.push(l.clone());
                        }
                        return;
                    }
                };
                let Some(q1) = self.quote_sisi(&m) else { return };
                // v3: jumlah dari sisi POOL (positif = pool menerima) -> sisi pedagang = negatif
                let (a0, a1) = (-w_f64(word(data, 0), true), -w_f64(word(data, 1), true));
                let s = w_f64(word(data, 2), false) / Q96;
                let liq = w_f64(word(data, 3), false);
                let p1per0 = s * s * 10f64.powf(m.dec0 - m.dec1);
                let (dasar, qabs, harga, lq, qa, dq, bt) = if q1 {
                    (a0, a1.abs(), p1per0, liq * s, m.t1.clone(), m.dec1, m.t0.clone())
                } else {
                    (a1, a0.abs(), 1.0 / p1per0, liq / s, m.t0.clone(), m.dec0, m.t1.clone())
                };
                let pelaku = tp.get(2).map(|t| hash_pelaku(t)).unwrap_or(0); // penerima (agregat, tanpa profil)
                self.catat(&pool, blk, dasar, qabs, harga, lq, &qa, dq, &bt, pelaku);
            }
            V2_SWAP => {
                self.terlihat.insert(pool.clone(), blk);
                let siap = self.meta.contains_key(&pool) && self.lengkapi(&pool);
                let m = match self.meta.get(&pool) {
                    Some(m) if siap => m.clone(),
                    _ => {
                        self.minta_meta(&pool);
                        let v = self.tunda_lain.entry(pool.clone()).or_default();
                        if v.len() < 400 {
                            v.push(l.clone());
                        }
                        return;
                    }
                };
                let Some(q1) = self.quote_sisi(&m) else { return };
                let (i0, i1, o0, o1) = (w_f64(word(data, 0), false), w_f64(word(data, 1), false), w_f64(word(data, 2), false), w_f64(word(data, 3), false));
                let (dasar, qabs, qa, dq, dd, bt) = if q1 { (o0 - i0, (i1 - o1).abs(), m.t1.clone(), m.dec1, m.dec0, m.t0.clone()) } else { (o1 - i1, (i0 - o0).abs(), m.t0.clone(), m.dec0, m.dec1, m.t1.clone()) };
                // harga rata-rata swap ini (quote per dasar), dinormalkan desimal
                let harga = if dasar.abs() > 0.0 { (qabs / 10f64.powf(dq)) / (dasar.abs() / 10f64.powf(dd)) } else { f64::NAN };
                let pelaku = tp.get(2).map(|t| hash_pelaku(t)).unwrap_or(0);
                self.catat(&pool, blk, dasar, qabs, harga, f64::NAN, &qa, dq, &bt, pelaku);
            }
            V2_SYNC => {
                if let Some(m) = self.meta.get(&pool).filter(|m| m.ok).cloned() {
                    if let Some(q1) = self.quote_sisi(&m) {
                        let (r0, r1) = (w_f64(word(data, 0), false), w_f64(word(data, 1), false));
                        let (rq, dq, qa) = if q1 { (r1, m.dec1, m.t1.clone()) } else { (r0, m.dec0, m.t0.clone()) };
                        let px = self.usd_quote(&qa);
                        if px.is_finite() {
                            self.stat.entry(pool).or_default().likuid = 2.0 * rq / 10f64.powf(dq) * px;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    /// Gabung data DEX Screener (satu objek "pair"). Dedupe: token & pool huruf kecil.
    pub fn gabung_ds(&mut self, p: &Value, sekarang_ms: f64) {
        if p["chainId"].as_str() != Some("robinhood") {
            return;
        }
        let pool = p["pairAddress"].as_str().unwrap_or("").to_lowercase();
        let base = p["baseToken"]["address"].as_str().unwrap_or("").to_lowercase();
        let quote = p["quoteToken"]["address"].as_str().unwrap_or("").to_lowercase();
        if base.is_empty() {
            return;
        }
        let t = self.token.entry(base.clone()).or_default();
        t.simbol = p["baseToken"]["symbol"].as_str().unwrap_or("").chars().take(24).collect();
        t.nama = p["baseToken"]["name"].as_str().unwrap_or("").chars().take(40).collect();
        if let Some(mc) = p["marketCap"].as_f64().or(p["fdv"].as_f64()) {
            t.mcap = mc;
        }
        if let Some(c) = p["pairCreatedAt"].as_f64() {
            t.dibuat_ms = if t.dibuat_ms > 0.0 { t.dibuat_ms.min(c) } else { c };
        }
        if let Some(b) = p["boosts"]["active"].as_f64() {
            t.boost = b;
        }
        if p["info"].is_object() {
            t.profil = true;
        }
        t.ds_waktu = sekarang_ms;
        if !pool.is_empty() {
            let st = self.stat.entry(pool.clone()).or_default();
            st.sumber |= 2;
            if !self.meta.contains_key(&pool) {
                // pool hanya dikenal dari DEX Screener: on-chain akan mengisinya begitu ada swap
                let dex = format!("{}{}", p["dexId"].as_str().unwrap_or("?"), p["labels"].as_array().and_then(|a| a.first()).and_then(|x| x.as_str()).map(|x| format!("-{x}")).unwrap_or_default());
                let (t0, t1) = if base < quote { (base.clone(), quote.clone()) } else { (quote.clone(), base.clone()) };
                if pool.len() == 66 {
                    self.minta_meta(&pool); // v4: meta lengkap dari Initialize
                } else {
                    self.meta.insert(pool.clone(), MetaPool { dex, t0, t1, fee: f64::NAN, dec0: f64::NAN, dec1: f64::NAN, lahir: f64::NAN, ok: false, pabrik: String::new() });
                    self.diminta.remove(&pool);
                    self.minta_meta(&pool);
                }
            }
        }
    }
    /// Gagal (mis. RPC 429): boleh diminta ulang saat pool/token itu terlihat lagi.
    pub fn lupakan_meta(&mut self, pool: &str) {
        self.diminta.remove(pool);
    }
    pub fn lupakan_desimal(&mut self, token: &str) {
        self.desimal_diminta.remove(token);
    }
    pub fn lupakan_kelas(&mut self, token: &str) {
        self.kelas_diminta.remove(token);
    }
    pub fn tandai_token_ds(&mut self, alamat: &str, boost: f64, profil: bool) {
        let t = self.token.entry(alamat.to_lowercase()).or_default();
        if boost > 0.0 {
            t.boost = t.boost.max(boost);
        }
        t.profil |= profil;
    }
    fn agregat(s: &StatPool, mnt: i64, dari: i64, sampai: i64) -> (u32, u32, f64, f64, u32) {
        let (mut b, mut j, mut ub, mut uj, mut tx) = (0, 0, 0.0, 0.0, 0);
        for e in s.ember.iter().rev() {
            if e.menit < mnt - sampai {
                break;
            }
            if e.menit <= mnt - dari {
                b += e.beli;
                j += e.jual;
                ub += e.usd_beli;
                uj += e.usd_jual;
                tx += e.tx;
            }
        }
        (b, j, ub, uj, tx)
    }
    fn pelaku_unik(s: &StatPool, mnt: i64, dari: i64, sampai: i64) -> usize {
        let mut v: Vec<u64> = Vec::new();
        for e in s.ember.iter().rev() {
            if e.menit < mnt - sampai {
                break;
            }
            if e.menit <= mnt - dari {
                v.extend(e.pelaku.iter().cloned());
            }
        }
        v.sort_unstable();
        v.dedup();
        v.len()
    }
    fn harga_lalu(s: &StatPool, mnt: i64, mundur: i64) -> f64 {
        s.ember.iter().rev().find(|e| e.menit <= mnt - mundur && e.tutup > 0.0).map_or(f64::NAN, |e| e.tutup)
    }
    fn dasar_token(&self, pool: &str) -> String {
        match self.meta.get(pool).and_then(|m| self.quote_sisi(m).map(|q1| if q1 { m.t0.clone() } else { m.t1.clone() })) {
            Some(t) => t,
            None => String::new(),
        }
    }
    /// Fitur radar satu pool pada blok `blk` (hanya data <= blk).
    fn fitur(&self, pool: &str, s: &StatPool, blk: f64) -> Value {
        let mnt = (blk / MENIT).floor() as i64;
        let (b5, j5, ub5, uj5, tx5) = Self::agregat(s, mnt, 0, 4);
        let (b5p, j5p, _, _, _) = Self::agregat(s, mnt, 5, 9);
        let (b60, j60, ub60, uj60, tx60) = Self::agregat(s, mnt, 0, 59);
        let (b24, j24, ub24, uj24, _) = Self::agregat(s, mnt, 0, 1439);
        let (_, _, _, _, tx30p) = Self::agregat(s, mnt, 5, 34);
        let (p5, p5p, p60) = (Self::pelaku_unik(s, mnt, 0, 4), Self::pelaku_unik(s, mnt, 5, 9), Self::pelaku_unik(s, mnt, 0, 59));
        let m = self.meta.get(pool);
        let lahir = m.map_or(f64::NAN, |m| m.lahir);
        let tok = self.token.get(&self.dasar_token(pool));
        let ubah = |k: i64| {
            let p = Self::harga_lalu(s, mnt, k);
            if p > 0.0 && s.harga > 0.0 {
                s.harga / p - 1.0
            } else {
                f64::NAN
            }
        };
        json!({
            "umur_mnt": if lahir.is_finite() { (blk - lahir) / MENIT } else { f64::NAN },
            "diamati_mnt": (blk - s.pertama_dilihat.max(self.mulai_blk)) / MENIT,
            "dex": m.map_or("?".to_string(), |m| m.dex.clone()),
            "harga_usd": s.harga, "likuid_usd": s.likuid,
            "beli_5m": b5, "jual_5m": j5, "usd_beli_5m": ub5, "usd_jual_5m": uj5, "tx_5m": tx5,
            "beli_5m_sebelum": b5p, "jual_5m_sebelum": j5p,
            "pelaku_5m": p5, "pelaku_5m_sebelum": p5p, "pelaku_1j": p60,
            "tumbuh_tx": (tx5 as f64 + 1.0) / (tx30p as f64 / 6.0 + 1.0),
            "tumbuh_pelaku": (p5 as f64 + 1.0) / (p5p as f64 + 1.0),
            "beli_1j": b60, "jual_1j": j60, "usd_beli_1j": ub60, "usd_jual_1j": uj60, "tx_1j": tx60,
            "beli_24j": b24, "jual_24j": j24, "usd_24j": ub24 + uj24,
            "ubah_5m": ubah(5), "ubah_1j": ubah(60), "ubah_24j": ubah(1440),
            "bangun_dari_tidur": s.bangun > 0.0 && blk - s.bangun < 36000.0,
            "jam_tidur": if s.bangun > 0.0 { (s.bangun - s.sepi_sejak) / 36000.0 } else { 0.0 },
            "mcap_ds": tok.map_or(f64::NAN, |t| t.mcap), "boost_ds": tok.map_or(0.0, |t| t.boost), "profil_ds": tok.map_or(false, |t| t.profil),
            "skor_ml": self.skor_ml.get(pool).cloned().unwrap_or(f64::NAN),
        })
    }
    /// Skor "panas" radar (transparan, BUKAN sinyal beli): volume, jumlah beli, akselerasi pembeli, dominasi jual.
    fn panas(f: &Value) -> f64 {
        let g = |k: &str| f[k].as_f64().unwrap_or(0.0);
        let vol = g("usd_beli_5m") + g("usd_jual_5m");
        let (b5, b5p, j5) = (g("beli_5m"), g("beli_5m_sebelum"), g("jual_5m"));
        let akselerasi = ((b5 + 1.0) / (b5p + 1.0)).log2().clamp(-2.0, 3.0);
        let mut s = (1.0 + vol).log10() + 0.5 * (1.0 + b5).log2() + akselerasi;
        if j5 > 2.0 * b5 + 2.0 {
            s -= 1.0;
        }
        if g("likuid_usd") < 2000.0 {
            s -= 1.0;
        }
        if f["bangun_dari_tidur"].as_bool() == Some(true) {
            s += 0.5;
        }
        s
    }
    /// Tulis radar.json (peringkat) dan kelola snapshot berkala + label masa depan.
    /// KLASIFIKASI DULU: hanya token dasar berkelas meme yang diberi peringkat; saham ter-tokenisasi hanya dicatat.
    pub fn tulis(&mut self, dir: &Path, blk: f64) {
        let mut baris: Vec<(f64, String, Value)> = Vec::new();
        let mut dikecualikan: BTreeMap<String, u64> = BTreeMap::new();
        let mut belum: Vec<String> = Vec::new();
        for (pool, s) in &self.stat {
            if s.terakhir <= 0.0 || blk - s.terakhir > 36000.0 {
                continue; // tidak ada swap 1 jam terakhir
            }
            let tok = self.dasar_token(pool);
            match self.kelas.get(&tok) {
                Some(Kelas::Meme) => {}
                Some(Kelas::Ekuitas) => {
                    *dikecualikan.entry(tok).or_insert(0) += s.ember.iter().map(|e| e.tx as u64).sum::<u64>();
                    continue;
                }
                Some(Kelas::Quote) => continue,
                None => {
                    if !tok.is_empty() {
                        belum.push(tok);
                    }
                    continue; // belum diklasifikasi -> belum boleh diberi peringkat
                }
            }
            let f = self.fitur(pool, s, blk);
            baris.push((Self::panas(&f), pool.clone(), f));
        }
        for t in belum {
            if self.kelas_diminta.insert(t.clone()) {
                self.perlu_kelas.push_back(t);
            }
        }
        let daftar_kecuali: Vec<Value> = dikecualikan
            .iter()
            .map(|(t, n)| json!({"token": t, "simbol": self.token.get(t).map_or("", |x| x.simbol.as_str()), "alasan": "saham ter-tokenisasi (dikecualikan)", "swap_24j": n}))
            .collect();
        baris.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let jadi = |(sk, pool, f): &(f64, String, Value)| {
            let tok = self.dasar_token(pool);
            let info = self.token.get(&tok);
            json!({"pool": pool, "token": tok, "simbol": info.map_or("", |t| t.simbol.as_str()), "panas": (sk * 100.0).round() / 100.0, "fitur": f})
        };
        let n_aktif = baris.len();
        let g = |x: &Value, k: &str| x[k].as_f64().unwrap_or(0.0);
        // SEMUA koin meme aktif 1 jam terakhir (tanpa batas), diurutkan dari yang paling panas
        let semua: Vec<Value> = baris.iter().map(jadi).collect();
        let baru: Vec<Value> = baris.iter().filter(|x| x.2["umur_mnt"].as_f64().map_or(false, |u| u <= 60.0)).map(jadi).collect();
        let bangun: Vec<Value> = baris.iter().filter(|x| x.2["bangun_dari_tidur"].as_bool() == Some(true)).map(jadi).collect();
        // POTENSI: transaksi bertumbuh ATAU pelaku aktif bertambah, beli >= jual, harga tidak sedang ambruk (tanpa batas jumlah)
        let potensi: Vec<Value> = baris
            .iter()
            .filter(|x| {
                let f = &x.2;
                let tumbuh = (g(f, "tumbuh_tx") >= 1.5 && g(f, "tx_5m") >= 5.0) || (g(f, "tumbuh_pelaku") >= 1.5 && g(f, "pelaku_5m") >= 3.0);
                let lahir_diamati = f["umur_mnt"].as_f64().map_or(false, |u| u <= g(f, "diamati_mnt") + 1.0);
                (g(f, "diamati_mnt") >= 10.0 || lahir_diamati) && tumbuh && g(f, "beli_5m") >= g(f, "jual_5m") && f["ubah_5m"].as_f64().map_or(true, |u| u > -0.2)
            })
            .map(jadi)
            .collect();
        let teratas = semua.clone();
        let j = json!({
            "catatan": "RADAR — bukan sinyal beli. Trading hanya lewat strategi v4 teruji. Skor ML hanya untuk koin yang dinilai model teruji.",
            "blok": blk, "pool_aktif_1jam": n_aktif, "pool_dikenal": self.meta.len(), "token_ds": self.token.len(), "swap_tercatat": self.n_swap,
            "label_ditulis": self.n_label, "eth_usd": self.eth,
            "jumlah": {"aktif_1jam": n_aktif, "potensi": potensi.len(), "baru_1jam": baru.len(), "bangun_dari_tidur": bangun.len()},
            "cakupan": {
                "pool_terlihat_5m": self.terlihat.values().filter(|b| blk - **b <= 3000.0).count(),
                "menunggu_meta": self.tunda_v4.len() + self.tunda_lain.len(),
                "antrean_meta": self.perlu_meta.len(), "antrean_kelas": self.perlu_kelas.len(), "antrean_desimal": self.perlu_desimal.len(),
                "pabrik_dikenal": self.pabrik_dikenal.len(),
                "stablecoin_terdeteksi": self.kelas.iter().filter(|(k, v)| **v == Kelas::Quote && !crate::kelas::quote_dikenal(k)).count(),
                "radar_jalan_mnt": (blk - self.mulai_blk) / MENIT,
            },
            "terlihat_5m": self.terlihat.iter().filter(|(_, b)| blk - **b <= 3000.0).map(|(k, _)| k.clone()).collect::<Vec<String>>(),
            "potensi_tumbuh": potensi, "baru_1jam": baru, "bangun_dari_tidur": bangun, "semua_aktif": teratas,
            "terdeteksi_dikecualikan": daftar_kecuali,
        });
        let p = dir.join("radar.json");
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&j).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
        // snapshot tiap 5 menit untuk semua pool yang aktif 5 menit terakhir
        if blk - self.snap_blk >= 3000.0 {
            self.snap_blk = blk;
            for (_, pool, f) in &baris {
                if f["tx_5m"].as_f64().unwrap_or(0.0) > 0.0 {
                    if let Some(h) = self.stat.get(pool).map(|s| s.harga).filter(|h| *h > 0.0) {
                        self.snap.push(Snap { pool: pool.clone(), blk, harga: h, fitur: f.clone(), maks1: 1.0, maks6: 1.0, min6: 1.0, akhir: 1.0 });
                    }
                }
            }
        }
        // label: snapshot yang horizon 6 jamnya sudah lewat -> ditulis (fitur masa lalu + hasil masa depan)
        let (lewat, sisa): (Vec<Snap>, Vec<Snap>) = std::mem::take(&mut self.snap).into_iter().partition(|s| blk > s.blk + HORIZON);
        self.snap = sisa;
        if !lewat.is_empty() {
            if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("radar_labels.jsonl")) {
                for s in lewat {
                    let j = json!({"pool": s.pool, "blk": s.blk, "fitur": s.fitur, "maks_1j": s.maks1, "maks_6j": s.maks6, "min_6j": s.min6, "akhir_6j": s.akhir});
                    let _ = writeln!(fh, "{j}");
                    self.n_label += 1;
                }
            }
        }
        self.terlihat.retain(|_, b| blk - *b <= 36000.0);
        // buang statistik pool yang diam > 24 jam (memori tetap kecil); meta tetap disimpan
        self.stat.retain(|_, s| blk - s.terakhir < 864000.0 || s.terakhir == 0.0 && blk - s.pertama_dilihat < 864000.0);
    }
    /// Token yang perlu diperkaya DEX Screener (teramai yang datanya belum ada / > 10 menit).
    pub fn perlu_ds(&self, sekarang_ms: f64, maks: usize) -> Vec<String> {
        let mut v: Vec<(f64, String)> = Vec::new();
        for (pool, s) in &self.stat {
            if s.ember.is_empty() {
                continue;
            }
            let tok = self.dasar_token(pool);
            if tok.is_empty() || self.kelas.get(&tok) != Some(&Kelas::Meme) {
                continue;
            }
            let lama = self.token.get(&tok).map_or(true, |t| sekarang_ms - t.ds_waktu > 600_000.0);
            if lama {
                let n: u32 = s.ember.iter().rev().take(5).map(|e| e.tx).sum();
                v.push((n as f64, tok));
            }
        }
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let mut out: Vec<String> = Vec::new();
        for (_, t) in v {
            if !out.contains(&t) {
                out.push(t);
            }
            if out.len() >= maks {
                break;
            }
        }
        out
    }
}

/// Pembatas laju global untuk RPC latar radar: maks ~2 permintaan HTTP/detik; kena 429 -> semua pekerja mundur 10 dtk.
/// Polling utama (trading) tidak lewat pembatas ini dan selalu didahulukan.
pub struct Laju {
    berikut: std::sync::Mutex<std::time::Instant>,
}
impl Laju {
    pub fn new() -> Laju {
        Laju { berikut: std::sync::Mutex::new(std::time::Instant::now()) }
    }
    pub fn tunggu(&self) {
        let jadwal = {
            let mut g = self.berikut.lock().unwrap();
            let t = (*g).max(std::time::Instant::now());
            *g = t + std::time::Duration::from_millis(700);
            t
        };
        let now = std::time::Instant::now();
        if jadwal > now {
            std::thread::sleep(jadwal - now);
        }
    }
    pub fn mundur(&self) {
        *self.berikut.lock().unwrap() = std::time::Instant::now() + std::time::Duration::from_secs(10);
    }
}
fn batch_latar(rpc: &crate::net::Rpc, laju: &Laju, calls: &[(&str, Value)]) -> Option<Vec<Option<String>>> {
    laju.tunggu();
    match rpc.batch(calls, std::time::Duration::from_secs(10)) {
        Ok(v) => Some(v.into_iter().map(|r| r.ok().and_then(|x| x.as_str().map(|s| s.to_string()))).collect()),
        Err(e) => {
            if e.contains("429") || e.to_lowercase().contains("too many") {
                laju.mundur();
            }
            None
        }
    }
}
fn kata_f64(h: &Option<String>) -> f64 {
    h.as_ref().map(|h| w_f64(&format!("{:0>64}", h.trim_start_matches("0x")), false)).unwrap_or(f64::NAN)
}
fn desimal_ok(d: f64) -> f64 {
    if d.is_finite() && d <= 36.0 {
        d
    } else {
        f64::NAN
    }
}

/// Ambil meta satu pool (thread pekerja, dibatasi laju). v4: Initialize dari PoolManager; v2/v3: token0/token1/fee/factory
/// dalam SATU permintaan batch. Desimal diambil terpisah secara massal.
pub fn ambil_meta(rpc: &crate::net::Rpc, laju: &Laju, pool: &str, lama: Option<MetaPool>) -> Option<MetaPool> {
    let ec = |to: &str, data: &str| ("eth_call", json!([{"to": to, "data": data}, "latest"]));
    let mut m = lama.unwrap_or_default();
    if pool.len() == 66 {
        laju.tunggu();
        let r = match rpc.call_once("eth_getLogs", json!([{"address": PM, "topics": [V4_INIT, pool], "fromBlock": "0x0", "toBlock": "latest"}]), std::time::Duration::from_secs(10)) {
            Ok(r) => r,
            Err(e) => {
                if e.contains("429") {
                    laju.mundur();
                }
                return None;
            }
        };
        let l = r.as_array()?.first()?.clone();
        let tp: Vec<&str> = l["topics"].as_array()?.iter().filter_map(|t| t.as_str()).collect();
        return Some(MetaPool { dex: "uniswap-v4".into(), t0: addr(tp.get(2)?), t1: addr(tp.get(3)?), fee: w_f64(word(l["data"].as_str()?, 0), false), dec0: f64::NAN, dec1: f64::NAN, lahir: hexnum(&l["blockNumber"]), ok: false, pabrik: String::new() });
    }
    let v = batch_latar(rpc, laju, &[ec(pool, "0x0dfe1681"), ec(pool, "0xd21220a7"), ec(pool, "0xddca3f43"), ec(pool, "0xc45a0155")])?;
    m.t0 = addr(v.first()?.as_ref()?);
    m.t1 = addr(v.get(1)?.as_ref()?);
    let f = kata_f64(&v[2]);
    m.pabrik = v[3].as_ref().map(|h| addr(h)).unwrap_or_default();
    m.dex = format!("{}:{}", if f.is_finite() { "v3" } else { "v2" }, m.pabrik.get(..10).unwrap_or("?"));
    m.fee = if f.is_finite() { f } else { 3000.0 };
    m.dec0 = f64::NAN;
    m.dec1 = f64::NAN;
    m.ok = false;
    Some(m)
}

/// Desimal banyak token sekaligus (maks 50 / permintaan). Token yang decimals()-nya gagal dianggap 18.
pub fn ambil_desimal(rpc: &crate::net::Rpc, laju: &Laju, tok: &[String]) -> Option<Vec<(String, f64)>> {
    let calls: Vec<(&str, Value)> = tok.iter().map(|t| ("eth_call", json!([{"to": t, "data": "0x313ce567"}, "latest"]))).collect();
    let v = batch_latar(rpc, laju, &calls)?;
    Some(tok.iter().zip(v.iter()).map(|(t, h)| (t.clone(), { let d = desimal_ok(kata_f64(h)); if d.is_finite() { d } else { 18.0 } })).collect())
}

/// getLogs massal dengan pemecahan rentang otomatis (batas 10.000 log / kueri).
pub fn log_massal(rpc: &crate::net::Rpc, laju: &Laju, alamat: &str, topik: &str, dari: u64, sampai: u64) -> Vec<Value> {
    let mut out = Vec::new();
    // RPC Robinhood: maks 10 juta blok per kueri ber-alamat -> potong 9 juta; sisanya dipecah otomatis
    let mut tumpuk = Vec::new();
    let mut x = dari;
    while x <= sampai {
        let y = (x + 9_000_000).min(sampai);
        tumpuk.push((x, y));
        x = y + 1;
    }
    tumpuk.reverse();
    let mut gagal = 0;
    while let Some((a, b)) = tumpuk.pop() {
        laju.tunggu();
        match rpc.call_once("eth_getLogs", json!([{"address": alamat, "topics": [topik], "fromBlock": format!("0x{:x}", a), "toBlock": format!("0x{:x}", b)}]), std::time::Duration::from_secs(60)) {
            Ok(Value::Array(v)) => out.extend(v),
            Ok(_) => {}
            Err(e) if !e.contains("429") && (e.contains("exceeds") || e.contains("spans") || e.contains("allowed") || e.contains("limit")) => {
                if b > a {
                    let mid = a + (b - a) / 2;
                    tumpuk.push((mid + 1, b));
                    tumpuk.push((a, mid));
                }
            }
            Err(e) => {
                if e.contains("429") {
                    laju.mundur();
                }
                gagal += 1;
                if gagal < 20 {
                    tumpuk.push((a, b));
                }
            }
        }
    }
    out
}
/// Pemuatan awal: semua pool v4 yang lahir sejak blok `dari` (event Initialize PoolManager).
pub fn boot_v4(rpc: &crate::net::Rpc, laju: &Laju, dari: u64, sampai: u64) -> Vec<(String, MetaPool)> {
    log_massal(rpc, laju, PM, V4_INIT, dari, sampai)
        .iter()
        .filter_map(|l| {
            let tp: Vec<&str> = l["topics"].as_array()?.iter().filter_map(|t| t.as_str()).collect();
            Some((tp.get(1)?.to_lowercase(), MetaPool { dex: "uniswap-v4".into(), t0: addr(tp.get(2)?), t1: addr(tp.get(3)?), fee: w_f64(word(l["data"].as_str()?, 0), false), dec0: f64::NAN, dec1: f64::NAN, lahir: hexnum(&l["blockNumber"]), ok: false, pabrik: String::new() }))
        })
        .collect()
}
/// Semua pool satu pabrik v3 (PoolCreated) / v2 (PairCreated) sejak awal chain.
pub fn boot_pabrik(rpc: &crate::net::Rpc, laju: &Laju, pabrik: &str, v3: bool, sampai: u64) -> Vec<(String, MetaPool)> {
    log_massal(rpc, laju, pabrik, if v3 { V3_CREATED } else { V2_CREATED }, 0, sampai)
        .iter()
        .filter_map(|l| {
            let tp: Vec<&str> = l["topics"].as_array()?.iter().filter_map(|t| t.as_str()).collect();
            let data = l["data"].as_str()?;
            let (p, fee) = if v3 { (addr(word(data, 1)), w_f64(tp.get(3)?.trim_start_matches("0x"), false)) } else { (addr(word(data, 0)), 3000.0) };
            Some((p, MetaPool { dex: format!("{}:{}", if v3 { "v3" } else { "v2" }, pabrik.get(..10).unwrap_or("?")), t0: addr(tp.get(1)?), t1: addr(tp.get(2)?), fee, dec0: f64::NAN, dec1: f64::NAN, lahir: hexnum(&l["blockNumber"]), ok: false, pabrik: pabrik.to_string() }))
        })
        .collect()
}
