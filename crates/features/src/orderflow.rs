//! Fitur orderflow menit-1 berdasarkan referensi "Robinhood Chain Meme-Coin Quant Spec".
//!
//! Semua fitur hanya memakai data sampai blok keputusan (init+600). Tidak ada profil dompet:
//! dompet hanya dipakai untuk HITUNGAN AGREGAT (berapa pembeli unik, seberapa terkonsentrasi),
//! tidak pernah dilabeli sniper/bot/paus/retail.
//!
//! Jendela waktu: menit pertama dibagi 3 jendela x 200 blok (±20 detik).
//!   w0 = [init, init+200), w1 = [init+200, init+400), w2 = [init+400, init+600]

use crate::features::{Meta, Swap, Transfer, NATIVE, PM};
use std::collections::{HashMap, HashSet};

pub const KEYS: &[&str] = &[
    // 1. pertumbuhan pembeli
    "of_buyers_w0", "of_buyers_w1", "of_buyers_w2", "of_new_buyers_w2",
    "of_buyer_growth", "of_buyer_accel", "of_sellers_w2", "of_seller_growth",
    // 2. aliran beli/jual
    "of_bs_ratio", "of_ofi", "of_ofi_w2", "of_ofi_trend",
    // 3. CVD
    "of_cvd_slope", "of_cvd_accel", "of_cvd_end_lp",
    // 4. distribusi ukuran trade (USD)
    "of_bsz_mean", "of_bsz_med", "of_bsz_p90", "of_bsz_skew", "of_bsz_kurt", "of_bsz_gini", "of_bsz_entropy",
    "of_ssz_med", "of_ssz_p90", "of_big_sell_share",
    // 5. intensitas transaksi
    "of_tps", "of_btps", "of_stps", "of_iat_cv", "of_burst", "of_tx_accel",
    // 6. likuiditas
    "of_liq_change", "of_liq_min_ratio",
    // 8. proksi permintaan organik (agregat)
    "of_swaps_per_buyer", "of_top_buyer_share", "of_repeat_buyer_share", "of_organic_ratio",
    // 4. distribusi ukuran (lanjutan)
    "of_bsz_p25", "of_bsz_p75", "of_bsz_p99", "of_bsz_cv", "of_ssz_mean", "of_ssz_skew", "of_ssz_gini", "of_sell_buy_med",
    // 7/8. klaster agregat dari graf transfer token (tanpa label per dompet)
    "of_clu_buy_share", "of_rt_share", "of_indep_ratio", "of_indep_buyers", "of_cvd_indep",
    // 9. volatilitas
    "of_rv", "of_ewma_vol", "of_updown_vol", "of_mdd_m1", "of_ret_w2",
    // 10. deteksi perubahan (CUSUM k=0 pada aliran bertanda / LP)
    "of_cusum_neg_last", "of_cusum_neg_max",
];

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { return f64::NAN; }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn moments(x: &[f64]) -> (f64, f64, f64) {
    // mean, skew, excess kurtosis
    let n = x.len() as f64;
    if x.len() < 3 { return (if x.is_empty() { f64::NAN } else { x.iter().sum::<f64>() / n }, f64::NAN, f64::NAN); }
    let m = x.iter().sum::<f64>() / n;
    let v = x.iter().map(|a| (a - m).powi(2)).sum::<f64>() / n;
    if v <= 0.0 { return (m, 0.0, 0.0); }
    let sd = v.sqrt();
    let sk = x.iter().map(|a| ((a - m) / sd).powi(3)).sum::<f64>() / n;
    let ku = x.iter().map(|a| ((a - m) / sd).powi(4)).sum::<f64>() / n - 3.0;
    (m, sk, ku)
}

fn gini(x: &[f64]) -> f64 {
    let mut s: Vec<f64> = x.iter().cloned().filter(|v| *v >= 0.0).collect();
    if s.len() < 2 { return f64::NAN; }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len() as f64;
    let tot: f64 = s.iter().sum();
    if tot <= 0.0 { return f64::NAN; }
    let mut cum = 0.0;
    for (i, v) in s.iter().enumerate() { cum += (i as f64 + 1.0) * v; }
    (2.0 * cum) / (n * tot) - (n + 1.0) / n
}

fn entropy(x: &[f64]) -> f64 {
    let tot: f64 = x.iter().filter(|v| **v > 0.0).sum();
    if tot <= 0.0 { return f64::NAN; }
    -x.iter().filter(|v| **v > 0.0).map(|v| { let p = v / tot; p * p.ln() }).sum::<f64>()
}

fn slope(y: &[f64]) -> f64 {
    // kemiringan kuadrat terkecil terhadap indeks 0..n-1
    let n = y.len();
    if n < 2 { return f64::NAN; }
    let nf = n as f64;
    let mx = (nf - 1.0) / 2.0;
    let my = y.iter().sum::<f64>() / nf;
    let mut sxy = 0.0; let mut sxx = 0.0;
    for (i, v) in y.iter().enumerate() { let dx = i as f64 - mx; sxy += dx * (v - my); sxx += dx * dx; }
    if sxx > 0.0 { sxy / sxx } else { f64::NAN }
}

pub fn orderflow(meta: &Meta, sw: &[Swap], tr: &[Transfer], eth_usd: f64) -> Vec<f64> {
    let mut f = vec![f64::NAN; KEYS.len()];
    let idx = |k: &str| KEYS.iter().position(|x| *x == k).unwrap();
    let q1 = meta.c1 == NATIVE;
    let init = meta.init;
    let win = |blk: f64| -> usize { let d = blk - init; if d < 200.0 { 0 } else if d < 400.0 { 1 } else { 2 } };

    // ---------- swap: aliran, CVD, ukuran trade, intensitas, likuiditas ----------
    let mut s: Vec<&Swap> = sw.iter().filter(|x| x.blk <= init + 600.0).collect();
    s.sort_by(|a, b| a.blk.partial_cmp(&b.blk).unwrap().then(a.li.partial_cmp(&b.li).unwrap()));
    let n = s.len();
    if n > 0 {
        let mut bsz = Vec::new(); let mut ssz = Vec::new();
        let mut bu_w = [0.0f64; 3]; let mut su_w = [0.0f64; 3]; let mut cnt_w = [0usize; 3];
        let mut cvd = Vec::with_capacity(n); let mut c = 0.0;
        let mut nb = 0usize; let mut ns = 0usize;
        for x in &s {
            let base = if q1 { x.a0 } else { x.a1 };
            let quote = if q1 { x.a1 } else { x.a0 };
            let usd = (quote.abs() / 1e18) * eth_usd;
            let w = win(x.blk);
            cnt_w[w] += 1;
            if base > 0.0 { bsz.push(usd); bu_w[w] += usd; c += usd; nb += 1; } else { ssz.push(usd); su_w[w] += usd; c -= usd; ns += 1; }
            cvd.push(c);
        }
        let bu: f64 = bu_w.iter().sum(); let su: f64 = su_w.iter().sum();
        f[idx("of_bs_ratio")] = if su > 0.0 { bu / su } else if bu > 0.0 { 50.0 } else { f64::NAN };
        f[idx("of_ofi")] = if bu + su > 0.0 { (bu - su) / (bu + su) } else { f64::NAN };
        let ofi = |i: usize| if bu_w[i] + su_w[i] > 0.0 { (bu_w[i] - su_w[i]) / (bu_w[i] + su_w[i]) } else { f64::NAN };
        f[idx("of_ofi_w2")] = ofi(2);
        f[idx("of_ofi_trend")] = ofi(2) - ofi(0);
        // CVD: kemiringan seluruh menit, dan akselerasi = kemiringan paruh kedua - paruh pertama
        f[idx("of_cvd_slope")] = slope(&cvd);
        if n >= 6 { f[idx("of_cvd_accel")] = slope(&cvd[n / 2..]) - slope(&cvd[..n / 2]); }
        // likuiditas dalam USD (sama dengan lp_m1 di filter_feats)
        let lpv: Vec<f64> = s.iter().map(|x| 2.0 * ((if q1 { x.l * x.sqrt_p } else { x.l / x.sqrt_p }) / 1e18) * eth_usd).collect();
        let lp_last = *lpv.last().unwrap();
        f[idx("of_cvd_end_lp")] = if lp_last > 0.0 { c / lp_last } else { f64::NAN };
        let lp0 = lpv[0];
        f[idx("of_liq_change")] = if lp0 > 0.0 { (lp_last - lp0) / lp0 } else { f64::NAN };
        let lpmin = lpv.iter().cloned().fold(f64::INFINITY, f64::min);
        f[idx("of_liq_min_ratio")] = if lp0 > 0.0 { lpmin / lp0 } else { f64::NAN };
        // distribusi ukuran
        let mut bs = bsz.clone(); bs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut ss = ssz.clone(); ss.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (bm, bsk, bku) = moments(&bsz);
        f[idx("of_bsz_mean")] = bm; f[idx("of_bsz_skew")] = bsk; f[idx("of_bsz_kurt")] = bku;
        f[idx("of_bsz_med")] = pct(&bs, 0.5); f[idx("of_bsz_p90")] = pct(&bs, 0.9);
        f[idx("of_bsz_gini")] = gini(&bsz); f[idx("of_bsz_entropy")] = entropy(&bsz);
        f[idx("of_ssz_med")] = pct(&ss, 0.5); f[idx("of_ssz_p90")] = pct(&ss, 0.9);
        // porsi jual "besar" (> p90 ukuran beli) dari total jual
        let p90b = pct(&bs, 0.9);
        if su > 0.0 && p90b.is_finite() { f[idx("of_big_sell_share")] = ssz.iter().filter(|v| **v > p90b).sum::<f64>() / su; }
        f[idx("of_bsz_p25")] = pct(&bs, 0.25); f[idx("of_bsz_p75")] = pct(&bs, 0.75); f[idx("of_bsz_p99")] = pct(&bs, 0.99);
        if bm.is_finite() && bm > 0.0 && bsz.len() >= 2 {
            let v = bsz.iter().map(|a| (a - bm).powi(2)).sum::<f64>() / bsz.len() as f64;
            f[idx("of_bsz_cv")] = v.sqrt() / bm;
        }
        let (sm, ssk, _) = moments(&ssz);
        f[idx("of_ssz_mean")] = sm; f[idx("of_ssz_skew")] = ssk; f[idx("of_ssz_gini")] = gini(&ssz);
        let (mb, ms) = (pct(&bs, 0.5), pct(&ss, 0.5));
        if mb.is_finite() && ms.is_finite() && mb > 0.0 { f[idx("of_sell_buy_med")] = ms / mb; }
        // volatilitas dari harga tiap swap
        let price: Vec<f64> = s.iter().map(|x| if q1 { x.sqrt_p * x.sqrt_p } else { 1.0 / (x.sqrt_p * x.sqrt_p) }).collect();
        let dl: Vec<f64> = (1..n).map(|i| (price[i] / price[i - 1]).ln()).filter(|d| d.is_finite()).collect();
        if !dl.is_empty() {
            f[idx("of_rv")] = dl.iter().map(|d| d * d).sum::<f64>().sqrt();
            let mut e = dl[0] * dl[0];
            for d in &dl[1..] { e = 0.94 * e + 0.06 * d * d; }
            f[idx("of_ewma_vol")] = e.sqrt();
            let up: f64 = dl.iter().filter(|d| **d > 0.0).map(|d| d * d).sum();
            let dn: f64 = dl.iter().filter(|d| **d < 0.0).map(|d| d * d).sum();
            f[idx("of_updown_vol")] = if dn > 0.0 { (up / dn).sqrt() } else if up > 0.0 { 50.0 } else { f64::NAN };
        }
        let mut pk = 0.0f64; let mut mdd = 0.0f64;
        for p in &price { if p.is_finite() { pk = pk.max(*p); if pk > 0.0 { mdd = mdd.max(1.0 - p / pk); } } }
        f[idx("of_mdd_m1")] = mdd;
        if let Some(i2) = s.iter().position(|x| win(x.blk) == 2) {
            let p0 = if i2 > 0 { price[i2 - 1] } else { price[0] };
            if p0 > 0.0 { f[idx("of_ret_w2")] = (price[n - 1] / p0).ln(); }
        }
        // CUSUM bawah (k=0) atas CVD / LP: jarak CVD dari puncaknya (sekarang & terbesar)
        if lp_last > 0.0 {
            let mut top = 0.0f64; let mut gmax = 0.0f64;
            for v in &cvd { top = top.max(*v); gmax = gmax.max(top - v); }
            f[idx("of_cusum_neg_last")] = (top - c) / lp_last;
            f[idx("of_cusum_neg_max")] = gmax / lp_last;
        }
        // intensitas: rentang waktu nyata dari swap pertama s/d akhir menit (0,1 dtk per blok)
        let t0 = s[0].blk; let dur = ((init + 600.0) - t0).max(1.0) * 0.1;
        f[idx("of_tps")] = n as f64 / dur; f[idx("of_btps")] = nb as f64 / dur; f[idx("of_stps")] = ns as f64 / dur;
        if n >= 3 {
            let iat: Vec<f64> = (1..n).map(|i| (s[i].blk - s[i - 1].blk) * 0.1).collect();
            let m = iat.iter().sum::<f64>() / iat.len() as f64;
            let sd = (iat.iter().map(|a| (a - m).powi(2)).sum::<f64>() / iat.len() as f64).sqrt();
            f[idx("of_iat_cv")] = if m > 0.0 { sd / m } else { f64::NAN };
        }
        // ledakan: swap terbanyak dalam jendela geser 50 blok (5 dtk)
        let mut best = 0usize; let mut j = 0usize;
        for i in 0..n { while s[i].blk - s[j].blk > 50.0 { j += 1; } best = best.max(i - j + 1); }
        f[idx("of_burst")] = best as f64;
        f[idx("of_tx_accel")] = cnt_w[2] as f64 - cnt_w[0] as f64;
    }

    // ---------- transfer: pertumbuhan pembeli & proksi organik (agregat) ----------
    let mut t: Vec<&Transfer> = tr.iter().filter(|x| x.blk <= init + 600.0).collect();
    t.sort_by(|a, b| a.blk.partial_cmp(&b.blk).unwrap().then(a.li.partial_cmp(&b.li).unwrap()));
    let mut seen: HashSet<&str> = HashSet::new();
    let mut buyers_w = [0usize; 3]; let mut sellers_w = [0usize; 3];
    let mut seen_w: [HashSet<&str>; 3] = [HashSet::new(), HashSet::new(), HashSet::new()];
    let mut sellers_seen: HashSet<&str> = HashSet::new();
    let mut new_w2 = 0usize;
    let mut buy_tok: HashMap<&str, f64> = HashMap::new(); let mut buy_cnt: HashMap<&str, usize> = HashMap::new();
    for x in &t {
        let w = win(x.blk);
        if x.from == PM && x.to != PM && x.to != NATIVE {
            if seen_w[w].insert(x.to.as_str()) { buyers_w[w] += 1; }
            if seen.insert(x.to.as_str()) && w == 2 { new_w2 += 1; }
            *buy_tok.entry(x.to.as_str()).or_insert(0.0) += x.amt;
            *buy_cnt.entry(x.to.as_str()).or_insert(0) += 1;
        }
        if x.to == PM && x.from != PM && x.from != NATIVE {
            if sellers_seen.insert(x.from.as_str()) { sellers_w[w] += 1; }
        }
    }
    let b = [buyers_w[0] as f64, buyers_w[1] as f64, buyers_w[2] as f64];
    f[idx("of_buyers_w0")] = b[0]; f[idx("of_buyers_w1")] = b[1]; f[idx("of_buyers_w2")] = b[2];
    f[idx("of_new_buyers_w2")] = new_w2 as f64;
    // pertumbuhan = perubahan pembeli unik per jendela; akselerasi = perubahan pertumbuhan
    f[idx("of_buyer_growth")] = (b[2] - b[0]) / 2.0;
    f[idx("of_buyer_accel")] = (b[2] - b[1]) - (b[1] - b[0]);
    f[idx("of_sellers_w2")] = sellers_w[2] as f64;
    f[idx("of_seller_growth")] = (sellers_w[2] as f64 - sellers_w[0] as f64) / 2.0;
    let ub = seen.len();
    if ub > 0 {
        let nbuy_swaps = f64::from(t.iter().filter(|x| x.from == PM && x.to != PM && x.to != NATIVE).count() as u32);
        f[idx("of_swaps_per_buyer")] = nbuy_swaps / ub as f64;
        let tot: f64 = buy_tok.values().sum();
        if tot > 0.0 {
            let top = buy_tok.values().cloned().fold(0.0, f64::max);
            f[idx("of_top_buyer_share")] = top / tot;
            let rep: f64 = buy_tok.iter().filter(|(k, _)| buy_cnt[*k] > 1).map(|(_, v)| *v).sum();
            f[idx("of_repeat_buyer_share")] = rep / tot;
            // proksi organik: porsi volume beli dari pembeli yang BUKAN 3 terbesar
            let mut v: Vec<f64> = buy_tok.values().cloned().collect();
            v.sort_by(|a, b| b.partial_cmp(a).unwrap());
            let top3: f64 = v.iter().take(3).sum();
            f[idx("of_organic_ratio")] = (tot - top3) / tot;
        }
    }

    // klaster agregat: dompet yang saling kirim token (p2p) digabung (union-find). Simpul ber-derajat > 10
    // (router/kontrak distribusi) tidak dipakai menggabung supaya tidak menyatukan semua pembeli.
    let mut deg: HashMap<&str, usize> = HashMap::new();
    let p2p: Vec<(&str, &str)> = t.iter()
        .filter(|x| x.from != PM && x.to != PM && x.from != NATIVE && x.to != NATIVE && x.from != x.to)
        .map(|x| (x.from.as_str(), x.to.as_str())).collect();
    for (a, b) in &p2p { *deg.entry(a).or_insert(0) += 1; *deg.entry(b).or_insert(0) += 1; }
    let mut par: HashMap<&str, &str> = HashMap::new();
    fn root<'a>(par: &mut HashMap<&'a str, &'a str>, x: &'a str) -> &'a str {
        let mut r = x;
        while let Some(&p) = par.get(r) { if p == r { break; } r = p; }
        let mut y = x;
        while let Some(&p) = par.get(y) { if p == r { break; } par.insert(y, r); y = p; }
        r
    }
    for (a, b) in &p2p {
        if deg[a] > 10 || deg[b] > 10 { continue; }
        par.entry(a).or_insert(a); par.entry(b).or_insert(b);
        let (ra, rb) = (root(&mut par, a), root(&mut par, b));
        if ra != rb { par.insert(ra, rb); }
    }
    let sold: HashSet<&str> = t.iter().filter(|x| x.to == PM && x.from != PM && x.from != NATIVE).map(|x| x.from.as_str()).collect();
    let mut sell_tok: HashMap<&str, f64> = HashMap::new();
    for x in t.iter().filter(|x| x.to == PM && x.from != PM && x.from != NATIVE) { *sell_tok.entry(x.from.as_str()).or_insert(0.0) += x.amt; }
    let tot: f64 = buy_tok.values().sum();
    if tot > 0.0 {
        let (mut clu, mut rt, mut ind, mut nind, mut net_ind) = (0.0, 0.0, 0.0, 0usize, 0.0);
        for (w, v) in &buy_tok {
            let in_clu = par.contains_key(w);
            let is_rt = sold.contains(w);
            if in_clu { clu += v; }
            if is_rt { rt += v; }
            if !in_clu && !is_rt { ind += v; nind += 1; }
            if !in_clu { net_ind += v - sell_tok.get(w).cloned().unwrap_or(0.0); }
        }
        f[idx("of_clu_buy_share")] = clu / tot;
        f[idx("of_rt_share")] = rt / tot;
        f[idx("of_indep_ratio")] = ind / tot;
        f[idx("of_indep_buyers")] = nind as f64;
        f[idx("of_cvd_indep")] = net_ind / tot;
    }
    f
}
