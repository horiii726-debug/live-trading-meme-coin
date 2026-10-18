//! Port 1:1 dari `rhc_vps/bot/filter_feats.js` fungsi `features(meta, sw, tr, ethUsd)`.
//!
//! Aturan kesamaan dengan V8:
//! * urutan penjumlahan floating point sama persis dengan `reduce`/loop JS (kiri ke kanan,
//!   mulai dari 0), tanpa FMA (Rust tidak pernah menggabung mul+add otomatis);
//! * `Math.log`/`Math.log10` memakai port fdlibm V8 (`crate::fdlibm`);
//! * `Math.max` dengan semantik NaN JS (`crate::jsnum::js_max`);
//! * sort stabil dengan pembanding yang sama (`a.blk-b.blk || a.li-b.li`, `b-a`, `a-b`);
//! * `Map` JS diganti peta berurutan-sisip (Vec + HashMap indeks) karena `totPos`
//!   dijumlah menurut urutan sisip;
//! * HashMap/HashSet Rust hanya dipakai untuk pencarian, tidak pernah diiterasi.

use crate::fdlibm;
use crate::jsnum::{cmp_from_js, js_max, js_max_all, js_str_cmp, js_truthy};
use std::collections::{HashMap, HashSet};

pub const NATIVE: &str = "0x0000000000000000000000000000000000000000";
pub const PM: &str = "0x8366a39cc670b4001a1121b8f6a443a643e40951";

/// Semua kunci objek `f` dalam urutan sisip JS (fee, hook, lalu `keys`).
pub const KEYS: [&str; 35] = [
    "fee",
    "hook",
    "n_swap",
    "n_buy",
    "n_sell",
    "buy_usd",
    "sell_usd",
    "lp_m1",
    "ret_m1",
    "flow_lp",
    "n_holders",
    "top1",
    "top5",
    "hhi",
    "entropy",
    "gini",
    "l2_l1",
    "pool_share",
    "n_buyers",
    "n_sellers",
    "g_nodes",
    "g_edges",
    "g_density",
    "g_lambda1",
    "g_eig_max_nonpool",
    "g_deg_central",
    "g_p2p_frac",
    "kyle_lambda",
    "impact_r2",
    "impact_cos",
    "impact_norm",
    "lam_buy",
    "lam_sell",
    "lw_mom",
    "lw_mom_log",
];

// Indeks ke KEYS.
const FEE: usize = 0;
const HOOK: usize = 1;
const N_SWAP: usize = 2;
const N_BUY: usize = 3;
const N_SELL: usize = 4;
const BUY_USD: usize = 5;
const SELL_USD: usize = 6;
const LP_M1: usize = 7;
const RET_M1: usize = 8;
const FLOW_LP: usize = 9;
const N_HOLDERS: usize = 10;
const TOP1: usize = 11;
const TOP5: usize = 12;
const HHI: usize = 13;
const ENTROPY: usize = 14;
const GINI: usize = 15;
const L2_L1: usize = 16;
const POOL_SHARE: usize = 17;
const N_BUYERS: usize = 18;
const N_SELLERS: usize = 19;
const G_NODES: usize = 20;
const G_EDGES: usize = 21;
const G_DENSITY: usize = 22;
const G_LAMBDA1: usize = 23;
const G_EIG_MAX_NONPOOL: usize = 24;
const G_DEG_CENTRAL: usize = 25;
const G_P2P_FRAC: usize = 26;
const KYLE_LAMBDA: usize = 27;
const IMPACT_R2: usize = 28;
const IMPACT_COS: usize = 29;
const IMPACT_NORM: usize = 30;
const LAM_BUY: usize = 31;
const LAM_SELL: usize = 32;
const LW_MOM: usize = 33;
const LW_MOM_LOG: usize = 34;

/// `meta` di JS: `{c0, c1, fee, hooks, init}` (alamat sudah huruf kecil).
#[derive(Clone, Debug)]
pub struct Meta {
    pub c0: String,
    pub c1: String,
    pub fee: f64,
    pub hooks: String,
    pub init: f64,
}

/// Satu swap menit-1: `{blk, li, a0, a1, sqrtP, L}` (sqrtP sudah dibagi Q96).
#[derive(Clone, Copy, Debug)]
pub struct Swap {
    pub blk: f64,
    pub li: f64,
    pub a0: f64,
    pub a1: f64,
    pub sqrt_p: f64,
    pub l: f64,
}

/// Satu transfer token: `[blk, li, from, to, amount]`. `amt` = `Number(amount)` JS
/// (lihat `jsnum::js_string_to_number`); konversinya murni sehingga boleh dilakukan di awal.
#[derive(Clone, Debug)]
pub struct Transfer {
    pub blk: f64,
    pub li: f64,
    pub from: String,
    pub to: String,
    pub amt: f64,
}

/// Hasil fitur, urutan sesuai `KEYS`. Nilai yang di JS `NaN` tetap `NaN` di sini.
#[derive(Clone, Debug)]
pub struct Features {
    pub vals: [f64; 35],
}

impl Features {
    pub fn get(&self, key: &str) -> Option<f64> {
        KEYS.iter().position(|k| *k == key).map(|i| self.vals[i])
    }

    /// `vector(f, feats)` di filter_feats.js: nilai tak-hingga/NaN -> NaN.
    pub fn vector(&self, feats: &[&str]) -> Vec<f64> {
        feats
            .iter()
            .map(|k| match self.get(k) {
                Some(v) if v.is_finite() => v,
                _ => f64::NAN,
            })
            .collect()
    }
}

/// `const num = v => (Number.isFinite(v) ? v : NaN)`
#[inline]
fn num(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        f64::NAN
    }
}

/// `a.reduce((s_, x, i) => s_ + x * b[i], 0)`
#[inline]
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len() {
        s += a[i] * b[i];
    }
    s
}

/// Pembanding `(a, b) => a.blk - b.blk || a.li - b.li`.
#[inline]
fn blk_li_cmp(ab: f64, al: f64, bb: f64, bl: f64) -> std::cmp::Ordering {
    let d = ab - bb;
    let r = if js_truthy(d) { d } else { al - bl };
    cmp_from_js(r)
}

/// Peta berurutan-sisip seperti `Map` JS.
struct OrderedMap<'a> {
    idx: HashMap<&'a str, usize>,
    keys: Vec<&'a str>,
    vals: Vec<f64>,
}

impl<'a> OrderedMap<'a> {
    fn new() -> Self {
        OrderedMap { idx: HashMap::new(), keys: Vec::new(), vals: Vec::new() }
    }
    fn get(&self, k: &str) -> Option<f64> {
        self.idx.get(k).map(|&i| self.vals[i])
    }
    fn set(&mut self, k: &'a str, v: f64) {
        match self.idx.get(k) {
            Some(&i) => self.vals[i] = v,
            None => {
                self.idx.insert(k, self.keys.len());
                self.keys.push(k);
                self.vals.push(v);
            }
        }
    }
}

pub fn features(meta: &Meta, sw: &[Swap], tr: &[Transfer], eth_usd: f64) -> Features {
    let q1 = meta.c1 == NATIVE;
    let mut f = [f64::NAN; 35];
    f[FEE] = meta.fee;
    f[HOOK] = if meta.hooks != NATIVE { 1.0 } else { 0.0 };

    if !sw.is_empty() {
        // const s = [...sw].sort((a, b) => a.blk - b.blk || a.li - b.li);  (stabil)
        let mut s: Vec<&Swap> = sw.iter().collect();
        s.sort_by(|a, b| blk_li_cmp(a.blk, a.li, b.blk, b.li));
        let n = s.len();
        let sp: Vec<f64> = s.iter().map(|x| x.sqrt_p).collect();
        let l: Vec<f64> = s.iter().map(|x| x.l).collect();
        let base: Vec<f64> = s.iter().map(|x| if q1 { x.a0 } else { x.a1 }).collect();
        let quote: Vec<f64> = s.iter().map(|x| if q1 { x.a1 } else { x.a0 }).collect();
        let buy: Vec<bool> = base.iter().map(|&v| v > 0.0).collect();
        let usd: Vec<f64> = quote.iter().map(|&v| (v.abs() / 1e18) * eth_usd).collect();
        let price: Vec<f64> = sp.iter().map(|&v| if q1 { v * v } else { 1.0 / (v * v) }).collect();
        let lp: Vec<f64> = (0..n)
            .map(|i| 2.0 * ((if q1 { l[i] * sp[i] } else { l[i] / sp[i] }) / 1e18) * eth_usd)
            .collect();
        let mut bu = 0.0;
        let mut su = 0.0;
        for i in 0..n {
            if buy[i] {
                bu += usd[i];
            }
        }
        for i in 0..n {
            if !buy[i] {
                su += usd[i];
            }
        }
        let lp_last = lp[n - 1];
        let n_buy = buy.iter().filter(|&&b| b).count();
        f[N_SWAP] = n as f64;
        f[N_BUY] = n_buy as f64;
        f[N_SELL] = (n - n_buy) as f64;
        f[BUY_USD] = bu;
        f[SELL_USD] = su;
        f[LP_M1] = num(lp_last);
        f[RET_M1] = if price[0] > 0.0 { price[n - 1] / price[0] } else { f64::NAN };
        f[FLOW_LP] = (bu - su) / js_max(lp_last, 1.0);

        // price impact: dlogP = lambda * volume bertanda (kuadrat terkecil)
        let mut dlp: Vec<f64> = Vec::new();
        let mut v: Vec<f64> = Vec::new();
        for i in 1..n {
            let d = fdlibm::log(price[i]) - fdlibm::log(price[i - 1]);
            let vv = if buy[i] { usd[i] } else { -usd[i] };
            if d.is_finite() && vv.is_finite() {
                dlp.push(d);
                v.push(vv);
            }
        }
        if v.len() >= 3 && dot(&v, &v) > 0.0 {
            let lam = dot(&v, &dlp) / dot(&v, &v);
            let res: Vec<f64> = dlp.iter().enumerate().map(|(i, &d)| d - lam * v[i]).collect();
            f[KYLE_LAMBDA] = lam * 100.0;
            f[IMPACT_R2] = 1.0 - dot(&res, &res) / js_max(dot(&dlp, &dlp), 1e-18);
            f[IMPACT_COS] = dot(&v, &dlp) / (dot(&v, &v).sqrt() * dot(&dlp, &dlp).sqrt() + 1e-18);
            f[IMPACT_NORM] = dot(&dlp, &dlp).sqrt();
            let bi: Vec<usize> = (0..v.len()).filter(|&i| v[i] > 0.0).collect();
            let si: Vec<usize> = (0..v.len()).filter(|&i| v[i] < 0.0).collect();
            if bi.len() >= 2 {
                let mut num_ = 0.0;
                for &i in &bi {
                    num_ += v[i] * dlp[i];
                }
                let mut den = 0.0;
                for &i in &bi {
                    den += v[i] * v[i];
                }
                f[LAM_BUY] = (num_ / den) * 100.0;
            }
            if si.len() >= 2 {
                let mut num_ = 0.0;
                for &i in &si {
                    num_ += v[i] * dlp[i];
                }
                let mut den = 0.0;
                for &i in &si {
                    den += v[i] * v[i];
                }
                f[LAM_SELL] = (num_ / den) * 100.0;
            }
        }
        if !dlp.is_empty() {
            // Catatan: JS memakai lp[i + 1] dengan i = indeks di dlp (bukan indeks swap) — ditiru apa adanya.
            let w: Vec<f64> = (0..dlp.len())
                .map(|i| match lp.get(i + 1) {
                    Some(&x) if x.is_finite() => x,
                    _ => 0.0,
                })
                .collect();
            let mut ws = 0.0;
            for &x in &w {
                ws += x;
            }
            f[LW_MOM] = if ws > 0.0 {
                let mut acc = 0.0;
                for (i, &d) in dlp.iter().enumerate() {
                    acc += d * w[i];
                }
                acc / ws
            } else {
                f64::NAN
            };
            f[LW_MOM_LOG] = fdlibm::log(js_max(f[RET_M1], 1e-9)) * fdlibm::log10(js_max(f[LP_M1], 10.0));
        }
    }

    // holder + graf dari transfer token
    // const sorted = [...tr].sort((a, b) => a[0] - b[0] || a[1] - b[1]);  (stabil)
    let mut sorted: Vec<&Transfer> = tr.iter().collect();
    sorted.sort_by(|a, b| blk_li_cmp(a.blk, a.li, b.blk, b.li));

    let mut bal = OrderedMap::new();
    // edges: Map "u|w" -> jumlah. Disimpan sebagai pasangan (u, w) dengan u < w (string JS).
    let mut edge_idx: HashMap<(&str, &str), usize> = HashMap::new();
    let mut edges: Vec<(&str, &str, f64)> = Vec::new();
    for t in &sorted {
        let fr = t.from.as_str();
        let to = t.to.as_str();
        let a = t.amt;
        if fr != NATIVE {
            let cur = bal.get(fr).filter(|&x| js_truthy(x)).unwrap_or(0.0);
            bal.set(fr, cur - a);
        }
        if to != NATIVE {
            let cur = bal.get(to).filter(|&x| js_truthy(x)).unwrap_or(0.0);
            bal.set(to, cur + a);
        }
        if fr != NATIVE && to != NATIVE && fr != to {
            let key = if js_str_cmp(fr, to) == std::cmp::Ordering::Less { (fr, to) } else { (to, fr) };
            match edge_idx.get(&key) {
                Some(&i) => edges[i].2 += 1.0,
                None => {
                    edge_idx.insert(key, edges.len());
                    edges.push((key.0, key.1, 1.0));
                }
            }
        }
    }

    // holders = nilai > 0 (bukan PM/NATIVE), urut turun (b - a)
    let mut holders: Vec<f64> = bal
        .keys
        .iter()
        .zip(bal.vals.iter())
        .filter(|(k, v)| **v > 0.0 && **k != PM && **k != NATIVE)
        .map(|(_, v)| *v)
        .collect();
    holders.sort_by(|a, b| cmp_from_js(b - a));
    if !holders.is_empty() {
        let mut tot = 0.0;
        for &h in &holders {
            tot += h;
        }
        let sh: Vec<f64> = holders.iter().map(|&h| h / tot).collect();
        f[N_HOLDERS] = holders.len() as f64;
        f[TOP1] = sh[0];
        let mut top5 = 0.0;
        for &x in sh.iter().take(5) {
            top5 += x;
        }
        f[TOP5] = top5;
        let mut hhi = 0.0;
        for &b in &sh {
            hhi += b * b;
        }
        f[HHI] = hhi;
        let mut ent = 0.0;
        for &b in &sh {
            ent += b * fdlibm::log(b);
        }
        f[ENTROPY] = -ent;
        let mut asc = sh.clone();
        asc.sort_by(|a, b| cmp_from_js(a - b));
        let mut cum = 0.0;
        let mut cs = 0.0;
        for &x in &asc {
            cum += x;
            cs += cum;
        }
        let len = asc.len() as f64;
        f[GINI] = 1.0 - (2.0 * cs) / len + 1.0 / len;
        let mut sq = 0.0;
        for &h in &holders {
            sq += h * h;
        }
        f[L2_L1] = sq.sqrt() / tot;
        // totPos: urutan sisip Map `bal`
        let mut tot_pos = 0.0;
        for &x in &bal.vals {
            if x > 0.0 {
                tot_pos += x;
            }
        }
        f[POOL_SHARE] = if tot_pos > 0.0 {
            let pm = bal.get(PM).filter(|&x| js_truthy(x)).unwrap_or(0.0);
            js_max(pm, 0.0) / tot_pos
        } else {
            f64::NAN
        };
    }

    let mut buyers: HashSet<&str> = HashSet::new();
    let mut sellers: HashSet<&str> = HashSet::new();
    for t in &sorted {
        let fr = t.from.as_str();
        let to = t.to.as_str();
        if fr == PM && to != PM && to != NATIVE {
            buyers.insert(to);
        }
        if to == PM && fr != PM && fr != NATIVE {
            sellers.insert(fr);
        }
    }
    f[N_BUYERS] = buyers.len() as f64;
    f[N_SELLERS] = sellers.len() as f64;

    // nodes = [...new Set(semua ujung edge)].sort()  (sort string JS)
    let mut node_set: HashSet<&str> = HashSet::new();
    let mut nodes: Vec<&str> = Vec::new();
    for &(u, w, _) in &edges {
        if node_set.insert(u) {
            nodes.push(u);
        }
        if node_set.insert(w) {
            nodes.push(w);
        }
    }
    nodes.sort_by(|a, b| js_str_cmp(a, b));
    if nodes.len() >= 3 {
        graph_features(&nodes, &edges, &mut f);
    }

    Features { vals: f }
}

/// Bagian graf. JS memakai matriks padat `A` (n x n) dan iterasi pangkat 200 kali.
/// Di sini matriks disimpan jarang (baris terurut menurut kolom). Hasilnya identik bit:
/// semua x[j] >= 0 dan hingga, sehingga suku `0 * x[j]` = +0 dan `s + (+0) = s` (s tidak
/// pernah -0 karena dimulai dari +0 dan semua suku >= 0). Kalau syarat itu suatu saat
/// tidak terpenuhi (x memuat NaN/Inf/negatif), perhitungan jatuh ke jalur padat yang
/// meniru JS apa adanya.
fn graph_features(nodes: &[&str], edges: &[(&str, &str, f64)], f: &mut [f64; 35]) {
    let n = nodes.len();
    let idx: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, &nd)| (nd, i)).collect();
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(u, w, c) in edges {
        let iu = idx[u];
        let iw = idx[w];
        adj[iu].push((iw, c));
        adj[iw].push((iu, c));
    }
    for row in adj.iter_mut() {
        row.sort_by_key(|&(j, _)| j);
    }

    // eigenvector utama lewat iterasi pangkat
    let mut x: Vec<f64> = vec![1.0 / (n as f64).sqrt(); n];
    let mut lambda = 0.0;
    let mut y = vec![0.0; n];
    for _it in 0..200 {
        let sparse_ok = x.iter().all(|&v| v.is_finite() && (v > 0.0 || (v == 0.0 && v.is_sign_positive())));
        for i in 0..n {
            let mut s = 0.0;
            if sparse_ok {
                for &(j, a) in &adj[i] {
                    s += a * x[j];
                }
            } else {
                // jalur padat (identik JS kata per kata)
                let row = &adj[i];
                let mut p = 0;
                for (j, &xj) in x.iter().enumerate() {
                    let a = if p < row.len() && row[p].0 == j {
                        p += 1;
                        row[p - 1].1
                    } else {
                        0.0
                    };
                    s += a * xj;
                }
            }
            y[i] = s;
        }
        let mut ss = 0.0;
        for &b in &y {
            ss += b * b;
        }
        let nrm = ss.sqrt();
        if !js_truthy(nrm) {
            break;
        }
        for v in y.iter_mut() {
            *v /= nrm;
        }
        lambda = nrm;
        std::mem::swap(&mut x, &mut y);
    }
    // deg = A.map(r => r.reduce((a, b) => a + b, 0))  — penjumlahan baris (bilangan bulat, eksak)
    let deg: Vec<f64> = adj
        .iter()
        .map(|row| {
            let mut s = 0.0;
            for &(_, a) in row {
                s += a;
            }
            s
        })
        .collect();
    let mx0 = js_max_all(x.iter().map(|v| v.abs()));
    let mx = if js_truthy(mx0) { mx0 } else { 1e-12 };
    let mut ev_max_non_pm = 0.0;
    for (i, &nd) in nodes.iter().enumerate() {
        if nd != PM {
            ev_max_non_pm = js_max(ev_max_non_pm, x[i].abs() / mx);
        }
    }
    let dmax = js_max_all(deg.iter().copied());
    let nf = n as f64;
    let n_edges = edges.len() as f64;
    f[G_NODES] = nf;
    f[G_EDGES] = n_edges;
    f[G_DENSITY] = n_edges / ((nf * (nf - 1.0)) / 2.0);
    f[G_LAMBDA1] = lambda;
    f[G_EIG_MAX_NONPOOL] = ev_max_non_pm;
    let mut dc = 0.0;
    for &d in &deg {
        dc += dmax - d;
    }
    f[G_DEG_CENTRAL] = dc / js_max((nf - 1.0) * (nf - 2.0), 1.0);
    let mut p2p = 0.0;
    let mut tot = 0.0;
    for &(u, w, c) in edges {
        tot += c;
        if u != PM && w != PM {
            p2p += c;
        }
    }
    f[G_P2P_FRAC] = if js_truthy(tot) { p2p / tot } else { 0.0 };
}
