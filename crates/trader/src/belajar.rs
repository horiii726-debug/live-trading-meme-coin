//! Adaptive entry filter, used IDENTICALLY by the live bot and the simulator:
//!
//!   score = (ML + DL) / 2
//!   ML = CatBoost (the filter_cat.json format), retrained by the official CatBoost command-line program (C++) when
//!        the bot's own recent picks go bad (win < 65% or rug > 20% over the last 20 picks whose outcome is known) or
//!        after 300 new outcomes; base data (Aug-Sep) weighs 0.3, a coin's outcome weighs 0.5^(age in days / 3)
//!   DL = a 34-128-64-32-1 neural network (ReLU, logistic output, Adam, L2 1e-3) on quantile-normalised features,
//!        trained in Rust on the base data at start and then on every coin the moment its outcome is known (one Adam
//!        step on that coin plus 32 random recent coins)
//!
//! The bot still only buys the top (1 - QTHR) of its recent scores (bot.rs), so only the ranking matters.
//!
//! Overfit guard: every coin is predicted by both halves BEFORE it is learned, so the AUC over the last 500 outcomes
//! is out-of-sample. If the network's falls to chance (< 0.52) the score is ML alone until it recovers (> 0.55); the
//! network keeps learning meanwhile. No look-ahead: a coin is scored at its decision, learned only after its outcome.
//! Research: ml_uji2.py / uji-gabung (18 Sep - 16 Oct, the bot's own simulator, causal): $148k frozen -> $185k.
use crate::catmodel::CatModel;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

/// Robinhood Chain blocks per day (measured 18 Sep - 15 Oct 2026)
pub const BLOK_PER_HARI: f64 = 852_000.0;
const N_KUANTIL: usize = 200;
const LAPIS: [usize; 4] = [128, 64, 32, 1];
const ALPHA: f64 = 1e-3;
const LR: f64 = 1e-3;
const EPOCH_DASAR: usize = 30;
const BATCH_DASAR: usize = 128;
const REPLAY: usize = 32;
const REPLAY_JENDELA: usize = 500;
const PICK_JENDELA: usize = 20;
const LABEL_BARU: usize = 300;
const JEDA_SKOR: u64 = 30;
const BOBOT_DASAR: f64 = 0.3;
const PARUH_HARI: f64 = 3.0;
/// overfit guard: out-of-sample accuracy over the last 500 outcomes (each coin is predicted BEFORE it is learned)
const UJI_JENDELA: usize = 500;
const DL_MATI_AUC: f64 = 0.52;
const DL_HIDUP_AUC: f64 = 0.55;

// ---------------------------------------------------------------- small deterministic RNG (splitmix64)
#[derive(Clone)]
pub struct Acak(u64);
impl Acak {
    pub fn new(seed: u64) -> Acak {
        Acak(seed)
    }
    pub fn u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn f64(&mut self) -> f64 {
        (self.u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn idx(&mut self, n: usize) -> usize {
        (self.f64() * n as f64) as usize % n.max(1)
    }
}

// ---------------------------------------------------------------- quantile normalisation (= sklearn QuantileTransformer, normal output)
fn persentil(sorted: &[f64], r: f64) -> f64 {
    let pos = r * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}
/// np.interp: x outside the table takes the end value
fn interp(x: f64, xs: &[f64], ys: &[f64]) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[xs.len() - 1] {
        return ys[ys.len() - 1];
    }
    let i = xs.partition_point(|v| *v <= x);
    let (x0, x1, y0, y1) = (xs[i - 1], xs[i], ys[i - 1], ys[i]);
    if x1 == x0 {
        y1
    } else {
        y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    }
}
/// inverse standard normal CDF (Acklam, relative error < 1.2e-9)
pub fn norm_ppf(p: f64) -> f64 {
    const A: [f64; 6] = [-3.969683028665376e1, 2.209460984245205e2, -2.759285104469687e2, 1.383577518672690e2, -3.066479806614716e1, 2.506628277459239];
    const B: [f64; 5] = [-5.447609879822406e1, 1.615858368580409e2, -1.556989798598866e2, 6.680131188771972e1, -1.328068155288572e1];
    const C: [f64; 6] = [-7.784894002430293e-3, -3.223964580411365e-1, -2.400758277161838, -2.549732539343734, 4.374664141464968, 2.938163982698783];
    const D: [f64; 4] = [7.784695709041462e-3, 3.224671290700398e-1, 2.445134137142996, 3.754408661907416];
    let pl = 0.02425;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5]) / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -norm_ppf(1.0 - p)
    }
}

#[derive(Clone)]
pub struct Kuantil {
    q: Vec<Vec<f64>>,
    q_neg: Vec<Vec<f64>>,
    refs: Vec<f64>,
    refs_neg: Vec<f64>,
}
impl Kuantil {
    pub fn fit(x: &[Vec<f64>]) -> Kuantil {
        let d = x[0].len();
        let refs: Vec<f64> = (0..N_KUANTIL).map(|i| i as f64 / (N_KUANTIL - 1) as f64).collect();
        let mut q = Vec::with_capacity(d);
        for j in 0..d {
            let mut c: Vec<f64> = x.iter().map(|r| r[j]).collect();
            c.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut col: Vec<f64> = refs.iter().map(|r| persentil(&c, *r)).collect();
            // sklearn makes the quantiles monotonic
            for i in 1..col.len() {
                if col[i] < col[i - 1] {
                    col[i] = col[i - 1];
                }
            }
            q.push(col);
        }
        let q_neg = q.iter().map(|c| c.iter().rev().map(|v| -v).collect()).collect();
        let refs_neg = refs.iter().rev().map(|v| -v).collect();
        Kuantil { q, q_neg, refs, refs_neg }
    }
    pub fn ubah(&self, x: &[f64]) -> Vec<f64> {
        const BATAS: f64 = 1e-7;
        let lo = norm_ppf(BATAS - f64::EPSILON);
        let hi = norm_ppf(1.0 - (BATAS - f64::EPSILON));
        x.iter()
            .enumerate()
            .map(|(j, v)| {
                let (q, qn) = (&self.q[j], &self.q_neg[j]);
                let mut u = 0.5 * (interp(*v, q, &self.refs) - interp(-*v, qn, &self.refs_neg));
                if *v + BATAS > q[q.len() - 1] {
                    u = 1.0;
                }
                if *v - BATAS < q[0] {
                    u = 0.0;
                }
                norm_ppf(u.clamp(1e-300, 1.0 - 1e-16)).clamp(lo, hi)
            })
            .collect()
    }
}

// ---------------------------------------------------------------- neural network (= sklearn MLPClassifier: relu, logistic out, adam)
#[derive(Clone)]
pub struct Mlp {
    w: Vec<Vec<f64>>, // layer l: n_in * n_out, row-major [i * n_out + o]
    b: Vec<Vec<f64>>,
    mw: Vec<Vec<f64>>,
    vw: Vec<Vec<f64>>,
    mb: Vec<Vec<f64>>,
    vb: Vec<Vec<f64>>,
    t: f64,
    ukuran: Vec<usize>,
}
impl Mlp {
    pub fn new(n_in: usize, rng: &mut Acak) -> Mlp {
        let mut ukuran = vec![n_in];
        ukuran.extend_from_slice(&LAPIS);
        let (mut w, mut b) = (Vec::new(), Vec::new());
        for l in 0..ukuran.len() - 1 {
            let (a, c) = (ukuran[l], ukuran[l + 1]);
            let bound = (6.0 / (a + c) as f64).sqrt();
            w.push((0..a * c).map(|_| (rng.f64() * 2.0 - 1.0) * bound).collect::<Vec<f64>>());
            b.push((0..c).map(|_| (rng.f64() * 2.0 - 1.0) * bound).collect::<Vec<f64>>());
        }
        let nol = |v: &Vec<Vec<f64>>| v.iter().map(|x| vec![0.0; x.len()]).collect::<Vec<_>>();
        Mlp { mw: nol(&w), vw: nol(&w), mb: nol(&b), vb: nol(&b), w, b, t: 0.0, ukuran }
    }
    fn maju(&self, x: &[f64]) -> Vec<Vec<f64>> {
        let mut akt = vec![x.to_vec()];
        for l in 0..self.w.len() {
            let (a, c) = (self.ukuran[l], self.ukuran[l + 1]);
            let inp = &akt[l];
            let mut out = self.b[l].clone();
            for i in 0..a {
                let xi = inp[i];
                if xi != 0.0 {
                    let row = &self.w[l][i * c..(i + 1) * c];
                    for o in 0..c {
                        out[o] += xi * row[o];
                    }
                }
            }
            if l + 1 < self.w.len() {
                for v in out.iter_mut() {
                    *v = v.max(0.0);
                }
            } else {
                for v in out.iter_mut() {
                    *v = 1.0 / (1.0 + (-v.clamp(-500.0, 500.0)).exp());
                }
            }
            akt.push(out);
        }
        akt
    }
    pub fn prob(&self, z: &[f64]) -> f64 {
        self.maju(z).last().unwrap()[0]
    }
    /// one Adam step on a mini-batch (mean log-loss + alpha/2 * |W|^2 / n, exactly sklearn's objective)
    pub fn langkah(&mut self, xs: &[&[f64]], ys: &[f64]) {
        let n = xs.len() as f64;
        let mut gw: Vec<Vec<f64>> = self.w.iter().map(|x| vec![0.0; x.len()]).collect();
        let mut gb: Vec<Vec<f64>> = self.b.iter().map(|x| vec![0.0; x.len()]).collect();
        let nl = self.w.len();
        for (x, y) in xs.iter().zip(ys) {
            let akt = self.maju(x);
            let mut delta: Vec<f64> = vec![akt[nl][0] - y];
            for l in (0..nl).rev() {
                let (a, c) = (self.ukuran[l], self.ukuran[l + 1]);
                let inp = &akt[l];
                for i in 0..a {
                    let xi = inp[i];
                    if xi != 0.0 {
                        let g = &mut gw[l][i * c..(i + 1) * c];
                        for o in 0..c {
                            g[o] += xi * delta[o];
                        }
                    }
                }
                for o in 0..c {
                    gb[l][o] += delta[o];
                }
                if l > 0 {
                    let mut d2 = vec![0.0; a];
                    for i in 0..a {
                        if inp[i] > 0.0 {
                            let row = &self.w[l][i * c..(i + 1) * c];
                            d2[i] = row.iter().zip(&delta).map(|(w, d)| w * d).sum();
                        }
                    }
                    delta = d2;
                }
            }
        }
        self.t += 1.0;
        let (b1, b2, eps) = (0.9f64, 0.999f64, 1e-8f64);
        let lr_t = LR * (1.0 - b2.powf(self.t)).sqrt() / (1.0 - b1.powf(self.t));
        for l in 0..nl {
            for k in 0..self.w[l].len() {
                let g = (gw[l][k] + ALPHA * self.w[l][k]) / n;
                self.mw[l][k] = b1 * self.mw[l][k] + (1.0 - b1) * g;
                self.vw[l][k] = b2 * self.vw[l][k] + (1.0 - b2) * g * g;
                self.w[l][k] -= lr_t * self.mw[l][k] / (self.vw[l][k].sqrt() + eps);
            }
            for k in 0..self.b[l].len() {
                let g = gb[l][k] / n;
                self.mb[l][k] = b1 * self.mb[l][k] + (1.0 - b1) * g;
                self.vb[l][k] = b2 * self.vb[l][k] + (1.0 - b2) * g * g;
                self.b[l][k] -= lr_t * self.mb[l][k] / (self.vb[l][k].sqrt() + eps);
            }
        }
    }
    fn ke_json(&self) -> Value {
        json!({"w": self.w, "b": self.b, "mw": self.mw, "vw": self.vw, "mb": self.mb, "vb": self.vb, "t": self.t, "ukuran": self.ukuran})
    }
    fn dari_json(j: &Value) -> Option<Mlp> {
        let m2 = |k: &str| -> Option<Vec<Vec<f64>>> { j.get(k)?.as_array()?.iter().map(|r| r.as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect())).collect() };
        let ukuran: Vec<usize> = j.get("ukuran")?.as_array()?.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).collect();
        let m = Mlp { w: m2("w")?, b: m2("b")?, mw: m2("mw")?, vw: m2("vw")?, mb: m2("mb")?, vb: m2("vb")?, t: j.get("t")?.as_f64()?, ukuran };
        let ok = m.w.len() == m.ukuran.len() - 1 && (0..m.w.len()).all(|l| m.w[l].len() == m.ukuran[l] * m.ukuran[l + 1] && m.b[l].len() == m.ukuran[l + 1]);
        ok.then_some(m)
    }
}

// ---------------------------------------------------------------- training data
/// Base training set (the system's own Aug-Sep coins): features in `feats` order, missing = -1 (as the bot scores).
pub fn baca_dasar(p: &Path, feats: &[String]) -> Result<(Vec<Vec<f64>>, Vec<bool>), String> {
    let t = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut it = t.lines();
    let h: Vec<&str> = it.next().ok_or("data latih dasar kosong")?.split(',').collect();
    let col = |k: &str| h.iter().position(|x| x.trim() == k).ok_or(format!("data latih dasar: kolom {k} tidak ada"));
    let idx: Vec<usize> = feats.iter().map(|k| col(k)).collect::<Result<_, _>>()?;
    let ir = col("ret")?;
    let (mut x, mut y) = (Vec::new(), Vec::new());
    for l in it {
        let p: Vec<&str> = l.split(',').collect();
        let ret: f64 = match p.get(ir).and_then(|s| s.trim().parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        x.push(idx.iter().map(|i| p.get(*i).and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(-1.0)).collect());
        y.push(ret > 1.0);
    }
    if x.len() < 500 {
        return Err(format!("data latih dasar terlalu sedikit ({} baris)", x.len()));
    }
    Ok((x, y))
}

#[derive(Clone, Debug)]
pub struct Label {
    pub x: Vec<f64>,
    pub ret: f64,
    /// the coin's decision block (its age for the ML weights)
    pub blk: f64,
}

/// AUC (probability a random winner scores above a random loser), ties counted half
pub fn auc(p: &[f64], y: &[bool]) -> f64 {
    let mut o: Vec<usize> = (0..p.len()).collect();
    o.sort_by(|a, b| p[*a].partial_cmp(&p[*b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut rank = vec![0.0; p.len()];
    let mut i = 0;
    while i < o.len() {
        let mut j = i;
        while j + 1 < o.len() && p[o[j + 1]] == p[o[i]] {
            j += 1;
        }
        for k in i..=j {
            rank[o[k]] = (i + j) as f64 / 2.0 + 1.0;
        }
        i = j + 1;
    }
    let n1 = y.iter().filter(|v| **v).count() as f64;
    let n0 = y.len() as f64 - n1;
    if n1 == 0.0 || n0 == 0.0 {
        return 0.5;
    }
    let s: f64 = rank.iter().zip(y).filter(|(_, v)| **v).map(|(r, _)| *r).sum();
    (s - n1 * (n1 + 1.0) / 2.0) / (n1 * n0)
}

/// What a retrain needs: a snapshot, so the live bot can train in a thread while it keeps trading.
pub struct DataLatih {
    pub x: Vec<Vec<f64>>,
    pub y: Vec<bool>,
    pub w: Vec<f64>,
    pub n_label: usize,
    pub alasan: &'static str,
}

// ---------------------------------------------------------------- the learner
pub struct Pembelajar {
    pub feats: Vec<String>,
    pub ml: Arc<CatModel>,
    dl: Mlp,
    qt: Kuantil,
    dasar_x: Vec<Vec<f64>>,
    dasar_y: Vec<bool>,
    pub labels: Vec<Label>,
    recent: VecDeque<(Vec<f64>, f64)>,
    picks: VecDeque<(bool, bool)>,
    pub n_label_fit: usize,
    pub n_skor: u64,
    n_skor_fit: u64,
    pub sedang_latih: bool,
    pub n_latih: u64,
    rng: Acak,
    /// (DL prediction, ML prediction, won) of the last 500 coins, each made before the coin was learned
    uji: VecDeque<(f64, f64, bool)>,
    pub dl_aktif: bool,
    pub n_dl_mati: u64,
    /// new outcomes that trigger an ML retrain (300; BELAJAR_LABEL_BARU lowers it only for a short live test)
    pub label_baru: usize,
}

impl Pembelajar {
    /// Start state: the shipped ML model, the network trained on the base data (deterministic).
    pub fn baru(ml: CatModel, dasar_x: Vec<Vec<f64>>, dasar_y: Vec<bool>) -> Pembelajar {
        let feats = ml.feats.clone();
        let qt = Kuantil::fit(&dasar_x);
        let mut rng = Acak::new(1);
        let mut dl = Mlp::new(feats.len(), &mut rng);
        let z: Vec<Vec<f64>> = dasar_x.iter().map(|x| qt.ubah(x)).collect();
        let y: Vec<f64> = dasar_y.iter().map(|b| if *b { 1.0 } else { 0.0 }).collect();
        let mut ord: Vec<usize> = (0..z.len()).collect();
        for _ in 0..EPOCH_DASAR {
            for i in (1..ord.len()).rev() {
                let j = rng.idx(i + 1);
                ord.swap(i, j);
            }
            for c in ord.chunks(BATCH_DASAR) {
                let xs: Vec<&[f64]> = c.iter().map(|i| z[*i].as_slice()).collect();
                let ys: Vec<f64> = c.iter().map(|i| y[*i]).collect();
                dl.langkah(&xs, &ys);
            }
        }
        Pembelajar {
            feats,
            ml: Arc::new(ml),
            dl,
            qt,
            dasar_x,
            dasar_y,
            labels: Vec::new(),
            recent: VecDeque::new(),
            picks: VecDeque::new(),
            n_label_fit: 0,
            n_skor: 0,
            n_skor_fit: 0,
            sedang_latih: false,
            n_latih: 0,
            rng,
            uji: VecDeque::new(),
            dl_aktif: true,
            n_dl_mati: 0,
            label_baru: LABEL_BARU,
        }
    }
    /// Out-of-sample AUC of (DL, ML) over the last outcomes; None before 100 of them.
    pub fn akurasi(&self) -> Option<(f64, f64)> {
        if self.uji.len() < 100 {
            return None;
        }
        let y: Vec<bool> = self.uji.iter().map(|u| u.2).collect();
        let pd: Vec<f64> = self.uji.iter().map(|u| u.0).collect();
        let pm: Vec<f64> = self.uji.iter().map(|u| u.1).collect();
        Some((auc(&pd, &y), auc(&pm, &y)))
    }
    pub fn skor_ml(&self, x: &[f64]) -> f64 {
        self.ml.predict(x)
    }
    pub fn skor_dl(&self, x: &[f64]) -> f64 {
        self.dl.prob(&self.qt.ubah(x))
    }
    /// The filter score of one coin at its minute-1 decision.
    pub fn skor(&mut self, x: &[f64]) -> f64 {
        self.n_skor += 1;
        if self.dl_aktif {
            0.5 * self.skor_ml(x) + 0.5 * self.skor_dl(x)
        } else {
            self.skor_ml(x)
        }
    }
    /// A coin's outcome is known (its shadow trade closed). `pick` = it passed the filter when it was decided.
    pub fn label(&mut self, x: Vec<f64>, ret: f64, blk: f64, pick: bool) {
        let z = self.qt.ubah(&x);
        let y = if ret > 1.0 { 1.0 } else { 0.0 };
        // out-of-sample check first: both halves predict this coin before anything learns from it
        self.uji.push_back((self.dl.prob(&z), self.ml.predict(&x), ret > 1.0));
        if self.uji.len() > UJI_JENDELA {
            self.uji.pop_front();
        }
        if self.uji.len() == UJI_JENDELA {
            if let Some((a_dl, _)) = self.akurasi() {
                if self.dl_aktif && a_dl < DL_MATI_AUC {
                    self.dl_aktif = false;
                    self.n_dl_mati += 1;
                } else if !self.dl_aktif && a_dl > DL_HIDUP_AUC {
                    self.dl_aktif = true;
                }
            }
        }
        self.recent.push_back((z.clone(), y));
        if self.recent.len() > REPLAY_JENDELA {
            self.recent.pop_front();
        }
        let mut xs: Vec<&[f64]> = vec![z.as_slice()];
        let mut ys: Vec<f64> = vec![y];
        let ambil: Vec<usize> = (0..REPLAY).map(|_| self.rng.idx(self.recent.len())).collect();
        for i in &ambil {
            xs.push(self.recent[*i].0.as_slice());
            ys.push(self.recent[*i].1);
        }
        let xs_own: Vec<Vec<f64>> = xs.iter().map(|v| v.to_vec()).collect();
        let refs: Vec<&[f64]> = xs_own.iter().map(|v| v.as_slice()).collect();
        self.dl.langkah(&refs, &ys);
        if pick {
            self.picks.push_back((ret > 1.0, ret < 0.1));
            if self.picks.len() > PICK_JENDELA {
                self.picks.pop_front();
            }
        }
        self.labels.push(Label { x, ret, blk });
    }
    /// The ML model must be retrained now? Some(reason). Checked before each score, as in the research.
    pub fn perlu_latih(&self) -> Option<&'static str> {
        if self.sedang_latih {
            return None;
        }
        if self.picks.len() == PICK_JENDELA {
            let n = PICK_JENDELA as f64;
            let win = self.picks.iter().filter(|p| p.0).count() as f64 / n;
            let rug = self.picks.iter().filter(|p| p.1).count() as f64 / n;
            if (win < 0.65 || rug > 0.20) && self.n_skor - self.n_skor_fit >= JEDA_SKOR {
                return Some("kinerja pilihan terakhir turun");
            }
        }
        if self.labels.len() - self.n_label_fit >= self.label_baru {
            return Some("300 hasil koin baru");
        }
        None
    }
    /// Snapshot for a retrain at block `now` (base weight 0.3, outcome weight halves every 3 days).
    pub fn data_latih(&mut self, now: f64, alasan: &'static str) -> DataLatih {
        self.sedang_latih = true;
        let mut x = self.dasar_x.clone();
        let mut y = self.dasar_y.clone();
        let mut w = vec![BOBOT_DASAR; x.len()];
        for l in &self.labels {
            // older than 45 days weighs < 0.00003: left out so a retrain stays fast after months of trading
            if (now - l.blk) / BLOK_PER_HARI > 45.0 {
                continue;
            }
            x.push(l.x.clone());
            y.push(l.ret > 1.0);
            w.push(0.5f64.powf(((now - l.blk) / BLOK_PER_HARI).max(0.0) / PARUH_HARI));
        }
        DataLatih { x, y, w, n_label: self.labels.len(), alasan }
    }
    /// A retrain finished. `bad` retrains (picks went bad) start the pick window afresh, as in the research.
    pub fn selesai_latih(&mut self, hasil: Result<CatModel, String>, d_n_label: usize, alasan: &'static str) -> Result<(), String> {
        self.sedang_latih = false;
        self.n_label_fit = d_n_label;
        self.n_skor_fit = self.n_skor;
        if alasan.starts_with("kinerja") {
            self.picks.clear();
        }
        let m = hasil?;
        if m.feats != self.feats {
            return Err("fitur model baru tidak sama".into());
        }
        self.ml = Arc::new(m);
        self.n_latih += 1;
        Ok(())
    }
    // ---------- persistence (the live bot restarts without forgetting) ----------
    pub fn simpan(&self, p: &Path) -> Result<(), String> {
        let j = json!({
            "versi": 1, "dl": self.dl.ke_json(), "n_label": self.labels.len(), "n_label_fit": self.n_label_fit,
            "n_skor": self.n_skor, "n_skor_fit": self.n_skor_fit, "n_latih": self.n_latih,
            "picks": self.picks.iter().map(|(w, r)| json!([w, r])).collect::<Vec<_>>(),
            "recent": self.recent.iter().map(|(z, y)| json!([z, y])).collect::<Vec<_>>(), "rng": self.rng.0,
            "uji": self.uji.iter().map(|(d, m, y)| json!([d, m, y])).collect::<Vec<_>>(), "dl_aktif": self.dl_aktif, "n_dl_mati": self.n_dl_mati,
        });
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, j.to_string()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, p).map_err(|e| e.to_string())
    }
    /// Restore a saved state; `labels` are the bot's labels.jsonl (all of them). Labels the saved state had not seen
    /// yet (bot stopped between saves) are learned now, in order. Returns how many were caught up.
    pub fn pulihkan(&mut self, p: &Path, labels: Vec<(Label, bool)>) -> Result<usize, String> {
        let mut dari = 0usize;
        if let Ok(t) = std::fs::read_to_string(p) {
            let j: Value = serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display()))?;
            if let Some(dl) = j.get("dl").and_then(Mlp::dari_json) {
                if dl.ukuran[0] == self.feats.len() {
                    self.dl = dl;
                    let u = |k: &str| j.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                    dari = (u("n_label") as usize).min(labels.len());
                    self.n_label_fit = (u("n_label_fit") as usize).min(dari);
                    self.n_skor = u("n_skor");
                    self.n_skor_fit = u("n_skor_fit").min(self.n_skor);
                    self.n_latih = u("n_latih");
                    self.rng = Acak::new(u("rng"));
                    self.picks = j.get("picks").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|p| Some((p.get(0)?.as_bool()?, p.get(1)?.as_bool()?))).collect()).unwrap_or_default();
                    self.recent = j
                        .get("recent")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|r| Some((r.get(0)?.as_array()?.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>(), r.get(1)?.as_f64()?))).collect())
                        .unwrap_or_default();
                    self.uji = j.get("uji").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|r| Some((r.get(0)?.as_f64()?, r.get(1)?.as_f64()?, r.get(2)?.as_bool()?))).collect()).unwrap_or_default();
                    self.dl_aktif = j.get("dl_aktif").and_then(|v| v.as_bool()).unwrap_or(true);
                    self.n_dl_mati = u("n_dl_mati");
                }
            }
        }
        let n = labels.len();
        for (i, (l, pick)) in labels.into_iter().enumerate() {
            if i < dari {
                self.labels.push(l);
            } else {
                self.label(l.x, l.ret, l.blk, pick);
            }
        }
        Ok(n - dari)
    }
}

// ---------------------------------------------------------------- CatBoost command-line program (C++)
fn tsv(path: &Path, x: &[Vec<f64>], y: &[bool], w: &[f64]) -> Result<(), String> {
    let mut s = String::with_capacity(x.len() * x.first().map_or(0, |r| r.len()) * 12);
    for i in 0..x.len() {
        s.push_str(if y[i] { "1" } else { "0" });
        s.push('\t');
        s.push_str(&format!("{:?}", w[i]));
        for v in &x[i] {
            s.push('\t');
            s.push_str(&format!("{:?}", v));
        }
        s.push('\n');
    }
    std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

fn jalankan(bin: &str, args: &[&str]) -> Result<(), String> {
    // low CPU priority where `nice` exists (the VPS): trading keeps its CPU while the model trains
    let mut c = if Path::new("/usr/bin/nice").exists() {
        let mut c = Command::new("/usr/bin/nice");
        c.args(["-n", "15", bin]);
        c
    } else {
        Command::new(bin)
    };
    let o = c.args(args).output().map_err(|e| format!("{bin}: {e}"))?;
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        let out = String::from_utf8_lossy(&o.stdout);
        return Err(format!("{bin} {}: {} {}", args.first().unwrap_or(&""), err.trim().chars().take(300).collect::<String>(), out.trim().chars().take(200).collect::<String>()));
    }
    Ok(())
}

/// CatBoost's own JSON model -> the bot's filter_cat.json format (the same conversion latih_ulang.py did).
pub fn dari_json_catboost(j: &Value, feats: &[String]) -> Result<Value, String> {
    let trees = j["oblivious_trees"].as_array().ok_or("model: oblivious_trees tidak ada")?;
    let mut out = Vec::with_capacity(trees.len());
    for t in trees {
        let splits: Vec<Value> = t["splits"]
            .as_array()
            .ok_or("model: splits")?
            .iter()
            .map(|s| Ok(json!({"feat": s["float_feature_index"].as_u64().ok_or("model: float_feature_index")?, "border": s["border"].as_f64().ok_or("model: border")?})))
            .collect::<Result<_, String>>()?;
        let leaf: Vec<f64> = t["leaf_values"].as_array().ok_or("model: leaf_values")?.iter().filter_map(|v| v.as_f64()).collect();
        if leaf.len() != 1usize << splits.len() {
            return Err(format!("model: {} daun untuk {} split", leaf.len(), splits.len()));
        }
        out.push(json!({"splits": splits, "leaf": leaf}));
    }
    let sb = &j["scale_and_bias"];
    let scale = sb[0].as_f64().unwrap_or(1.0);
    let bias = if sb[1].is_array() { sb[1][0].as_f64().unwrap_or(0.0) } else { sb[1].as_f64().unwrap_or(0.0) };
    let nf = j["features_info"]["float_features"].as_array().map_or(feats.len(), |a| a.len());
    if nf != feats.len() {
        return Err(format!("model: {nf} fitur, harus {}", feats.len()));
    }
    Ok(json!({"type": "catboost", "feats": feats, "trees": out, "scale": scale, "bias": bias}))
}

/// Train with the official CatBoost program and prove the Rust predictor gives the same probabilities as CatBoost's
/// own `calc` on 500 training rows. Returns the model (filter_cat.json format) and its parity check.
pub fn latih_catboost(bin: &str, kerja: &Path, feats: &[String], d: &DataLatih) -> Result<(Value, Value), String> {
    std::fs::create_dir_all(kerja).map_err(|e| format!("{}: {e}", kerja.display()))?;
    let (learn, cd, model, cek, pred) = (kerja.join("latih.tsv"), kerja.join("kolom.cd"), kerja.join("model.json"), kerja.join("cek.tsv"), kerja.join("cek_pred.tsv"));
    tsv(&learn, &d.x, &d.y, &d.w)?;
    std::fs::write(&cd, "0\tLabel\n1\tWeight\n").map_err(|e| e.to_string())?;
    let s = |p: &Path| p.to_string_lossy().to_string();
    let (ls, cs, ms, ks, ps, ts) = (s(&learn), s(&cd), s(&model), s(&cek), s(&pred), s(&kerja.join("train")));
    jalankan(
        bin,
        &[
            "fit", "--learn-set", &ls, "--column-description", &cs, "--loss-function", "Logloss", "--iterations", "200", "--depth", "6",
            "--learning-rate", "0.08", "--random-seed", "1", "--boosting-type", "Plain", "--border-count", "32", "--thread-count", "4",
            "--model-file", &ms, "--model-format", "Json", "--train-dir", &ts, "--logging-level", "Silent",
        ],
    )?;
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&model).map_err(|e| e.to_string())?).map_err(|e| format!("model.json: {e}"))?;
    let kand = dari_json_catboost(&raw, feats)?;
    let cm = CatModel::from_value(&kand)?;
    // parity: CatBoost's own predictions vs the Rust predictor, 500 rows spread over the training set
    let n = d.x.len();
    let idx: Vec<usize> = (0..500.min(n)).map(|i| i * n / 500.min(n).max(1)).collect();
    let xs: Vec<Vec<f64>> = idx.iter().map(|i| d.x[*i].clone()).collect();
    tsv(&cek, &xs, &vec![false; xs.len()], &vec![1.0; xs.len()])?;
    jalankan(bin, &["calc", "--input-path", &ks, "--column-description", &cs, "--model-file", &ms, "--model-format", "Json", "--prediction-type", "Probability", "--output-path", &ps])?;
    let t = std::fs::read_to_string(&pred).map_err(|e| e.to_string())?;
    let mut it = t.lines();
    let h: Vec<&str> = it.next().ok_or("cek_pred kosong")?.split('\t').collect();
    let kol = h.iter().rposition(|c| c.contains("Probability")).ok_or("cek_pred: kolom Probability tidak ada")?;
    let mut mx = 0.0f64;
    let mut ps_rust = Vec::with_capacity(xs.len());
    for (x, l) in xs.iter().zip(it) {
        let p_cb: f64 = l.split('\t').nth(kol).and_then(|v| v.parse().ok()).ok_or("cek_pred: angka rusak")?;
        let p_rs = cm.predict(x);
        mx = mx.max((p_cb - p_rs).abs());
        ps_rust.push(p_rs);
    }
    if ps_rust.len() != xs.len() || !(mx <= 1e-5) {
        return Err(format!("Rust != CatBoost ({mx:e}, {} dari {} baris)", ps_rust.len(), xs.len()));
    }
    let _ = std::fs::remove_dir_all(kerja.join("train"));
    Ok((kand, json!({"X": xs, "p": ps_rust, "selisih_catboost": mx})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantile_normalisation_matches_sklearn_shape() {
        let x: Vec<Vec<f64>> = (0..1000).map(|i| vec![i as f64, (i % 7) as f64]).collect();
        let q = Kuantil::fit(&x);
        let m = q.ubah(&[499.5, 3.0]);
        assert!(m[0].abs() < 0.01, "median maps to ~0, got {}", m[0]);
        assert!(q.ubah(&[-10.0, 0.0])[0] < -5.0 && q.ubah(&[5000.0, 0.0])[0] > 5.0, "outside the data: clipped tails");
        assert!((norm_ppf(0.975) - 1.959963984540054).abs() < 1e-8);
    }

    #[test]
    fn network_learns_a_simple_rule() {
        let mut rng = Acak::new(5);
        let mut m = Mlp::new(2, &mut rng);
        let data: Vec<(Vec<f64>, f64)> = (0..400).map(|_| {
            let (a, b) = (rng.f64() * 4.0 - 2.0, rng.f64() * 4.0 - 2.0);
            (vec![a, b], if a + b > 0.0 { 1.0 } else { 0.0 })
        }).collect();
        for _ in 0..60 {
            for c in data.chunks(32) {
                let xs: Vec<&[f64]> = c.iter().map(|d| d.0.as_slice()).collect();
                let ys: Vec<f64> = c.iter().map(|d| d.1).collect();
                m.langkah(&xs, &ys);
            }
        }
        let acc = data.iter().filter(|(x, y)| (m.prob(x) > 0.5) == (*y > 0.5)).count() as f64 / data.len() as f64;
        assert!(acc > 0.95, "accuracy {acc}");
        let back = Mlp::dari_json(&m.ke_json()).unwrap();
        assert_eq!(back.prob(&[0.3, -0.1]), m.prob(&[0.3, -0.1]), "saved network predicts identically");
    }

    fn model_satu_pohon(feats: &[String]) -> CatModel {
        CatModel::from_value(&json!({"feats": feats, "trees": [{"splits": [{"feat": 0, "border": 0.5}], "leaf": [-1.0, 1.0]}], "scale": 1.0, "bias": 0.0})).unwrap()
    }

    #[test]
    fn retrain_triggers_on_bad_picks_and_after_300_outcomes() {
        let feats: Vec<String> = vec!["a".into(), "b".into()];
        let dx: Vec<Vec<f64>> = (0..600).map(|i| vec![(i % 10) as f64, (i % 3) as f64]).collect();
        let dy: Vec<bool> = (0..600).map(|i| i % 10 > 4).collect();
        let mut p = Pembelajar::baru(model_satu_pohon(&feats), dx, dy);
        for _ in 0..40 {
            p.skor(&[1.0, 1.0]);
        }
        for _ in 0..19 {
            p.label(vec![1.0, 1.0], 0.0, 1.0, true);
        }
        assert_eq!(p.perlu_latih(), None, "19 known picks: not judged yet");
        p.label(vec![1.0, 1.0], 0.0, 1.0, true);
        assert_eq!(p.perlu_latih(), Some("kinerja pilihan terakhir turun"));
        let d = p.data_latih(BLOK_PER_HARI * 3.0 + 1.0, "kinerja pilihan terakhir turun");
        assert_eq!(d.x.len(), 620);
        assert!((d.w[600] - 0.5).abs() < 1e-6, "an outcome 3 days old weighs 0.5, got {}", d.w[600]);
        assert_eq!(p.perlu_latih(), None, "no second retrain while one runs");
        p.selesai_latih(Err("x".into()), d.n_label, d.alasan).unwrap_err();
        assert_eq!(p.perlu_latih(), None, "window restarted after a bad-picks retrain");
        for _ in 0..300 {
            p.label(vec![0.0, 0.0], 2.0, 2.0, false);
        }
        assert_eq!(p.perlu_latih(), Some("300 hasil koin baru"));
    }

    #[test]
    fn overfit_guard_drops_a_network_that_predicts_no_better_than_chance() {
        assert!((auc(&[0.1, 0.4, 0.35, 0.8], &[false, false, true, true]) - 0.75).abs() < 1e-12);
        let feats: Vec<String> = vec!["a".into(), "b".into()];
        let dx: Vec<Vec<f64>> = (0..600).map(|i| vec![(i % 10) as f64, (i % 3) as f64]).collect();
        let dy: Vec<bool> = (0..600).map(|i| i % 10 > 4).collect();
        let mut p = Pembelajar::baru(model_satu_pohon(&feats), dx, dy);
        assert!(p.dl_aktif);
        // outcomes unrelated to the features: the network cannot be better than chance out of sample
        let mut r = Acak::new(9);
        for i in 0..1500 {
            let x = vec![(i % 10) as f64, (i % 3) as f64];
            p.label(x, if r.f64() < 0.5 { 2.0 } else { 0.0 }, i as f64, false);
        }
        let (a_dl, _) = p.akurasi().unwrap();
        assert!(a_dl < 0.6, "chance-level network, AUC {a_dl}");
        assert!(p.n_dl_mati >= 1 || !p.dl_aktif, "the guard switched the network off at least once (AUC {a_dl})");
        let x = [3.0, 1.0];
        if !p.dl_aktif {
            assert_eq!(p.skor(&x), p.skor_ml(&x), "network off: the score is ML alone");
        }
    }

    #[test]
    fn catboost_json_converts_to_the_bot_format() {
        let feats: Vec<String> = vec!["a".into(), "b".into()];
        let j = json!({"oblivious_trees": [{"splits": [{"float_feature_index": 1, "border": 0.25, "split_type": "FloatFeature"}], "leaf_values": [0.1, -0.2]}],
                       "scale_and_bias": [1.0, [0.05]], "features_info": {"float_features": [{}, {}]}});
        let v = dari_json_catboost(&j, &feats).unwrap();
        let m = CatModel::from_value(&v).unwrap();
        let z = |s: f64| 1.0 / (1.0 + (-s).exp());
        assert!((m.predict(&[0.0, 0.0]) - z(0.15)).abs() < 1e-12);
        assert!((m.predict(&[0.0, 1.0]) - z(-0.15)).abs() < 1e-12);
    }

    #[test]
    fn learner_state_survives_a_restart() {
        let feats: Vec<String> = vec!["a".into(), "b".into()];
        let dx: Vec<Vec<f64>> = (0..600).map(|i| vec![(i % 10) as f64, (i % 3) as f64]).collect();
        let dy: Vec<bool> = (0..600).map(|i| i % 10 > 4).collect();
        let mut a = Pembelajar::baru(model_satu_pohon(&feats), dx.clone(), dy.clone());
        let lab: Vec<(Label, bool)> = (0..50).map(|i| (Label { x: vec![(i % 10) as f64, 1.0], ret: if i % 2 == 0 { 2.0 } else { 0.0 }, blk: i as f64 }, i % 5 == 0)).collect();
        for (l, pk) in lab.iter().take(30) {
            a.label(l.x.clone(), l.ret, l.blk, *pk);
        }
        let p = std::env::temp_dir().join(format!("belajar_test_{}.json", std::process::id()));
        a.simpan(&p).unwrap();
        for (l, pk) in lab.iter().skip(30) {
            a.label(l.x.clone(), l.ret, l.blk, *pk);
        }
        let mut b = Pembelajar::baru(model_satu_pohon(&feats), dx, dy);
        let caught = b.pulihkan(&p, lab.clone()).unwrap();
        assert_eq!(caught, 20, "the 20 outcomes after the last save are learned on restart");
        assert_eq!(b.labels.len(), 50);
        assert!((a.skor_dl(&[3.0, 1.0]) - b.skor_dl(&[3.0, 1.0])).abs() < 1e-12, "same network after restart");
        let _ = std::fs::remove_file(&p);
    }
}
