//! ML entry kapan saja: potret koin di banyak umur (bukan hanya detik 60). Fitur hanya dari data <= saat potret.
//! Label = dari beli di potret+LAT: TP 1.5x tercapai sebelum stop -30% / rug.
use crate::jalur::{muat, K};

const LAT: f64 = 3.0;
pub const UMUR: [f64; 10] = [60.0, 90.0, 120.0, 180.0, 300.0, 450.0, 600.0, 900.0, 1200.0, 1800.0];
pub const NAMA: &str = "umur,m60,m_awal,r30,r60,r300,maks,dd,sejak_maks,nb60,ns60,nb300,ns300,eb60,es60,eb300,es300,aksel,n_swap,beli_m1,jual_m1,beli_awal,ret_m1,res_eth,fee,jual_pertama,res_relatif";

/// hasil trade TP t / stop s dari blok masuk: (kelipatan bersih fee pool, blok keluar)
pub fn tp_sl(k: &K, masuk: f64, t: f64, s: f64) -> Option<(f64, f64)> {
    let isi = masuk + LAT;
    if k.rug <= isi { return Some((0.0, k.rug)); }
    let i0 = k.b.partition_point(|x| *x <= isi);
    if i0 == 0 { return None; }
    let p0 = k.p[i0 - 1];
    let biaya = (1.0 - k.fee / 1e6).powi(2);
    let akhir = (k.init + 324000.0).min(k.rug);
    for i in i0..k.b.len() {
        let blk = k.b[i];
        if blk >= akhir { break; }
        let m = k.p[i] / p0;
        if m >= t || m <= 1.0 - s {
            if k.rug <= blk + LAT { return Some((0.0, k.rug)); }
            let j = k.b.partition_point(|x| *x <= blk + LAT) - 1;
            return Some(((k.p[j] / p0).min(m) * biaya, blk + LAT));
        }
    }
    if k.rug < k.init + 324000.0 { Some((0.0, k.rug)) } else { Some((k.p[k.b.len() - 1] / p0 * biaya, akhir)) }
}

fn fitur(k: &K, t: f64) -> Option<Vec<f64>> {
    let tb = k.init + t * 10.0;
    let i = k.b.partition_point(|x| *x <= tb);
    if i == 0 { return None; }
    let p = k.p[i - 1];
    let ph = |blk: f64| -> f64 { let j = k.b.partition_point(|x| *x <= blk); if j == 0 { k.p[0] } else { k.p[j - 1] } };
    let p60 = ph(k.init + 600.0);
    let (mut mx, mut tmx) = (0.0f64, k.init);
    for j in 0..i { if k.p[j] > mx { mx = k.p[j]; tmx = k.b[j]; } }
    let hit = |a: f64, b: f64, beli: bool| -> (f64, f64) {
        let (mut n, mut e) = (0.0, 0.0);
        for j in 0..i { if k.b[j] > a && k.b[j] <= b && k.beli[j] == beli { n += 1.0; e += k.eth[j]; } }
        (n, e)
    };
    let (nb60, eb60) = hit(tb - 600.0, tb, true);
    let (ns60, es60) = hit(tb - 600.0, tb, false);
    let (nb300, eb300) = hit(tb - 3000.0, tb, true);
    let (ns300, es300) = hit(tb - 3000.0, tb, false);
    let (nbp, _) = hit(tb - 1200.0, tb - 600.0, true);
    if nb60 + ns60 + nb300 + ns300 == 0.0 { return None; } // tidak ada aktivitas 5 menit: tidak dinilai
    let d = k.init + 600.0;
    let (bm1, _) = { let (mut n, mut e) = (0.0, 0.0); for j in 0..k.b.len() { if k.b[j] > d { break } if k.beli[j] { n += 1.0; e += k.eth[j]; } } (n, e) };
    let jm1 = k.b.iter().zip(&k.beli).filter(|(b, bb)| **b <= d && !**bb).count() as f64;
    let ba = k.b.iter().zip(&k.beli).filter(|(b, bb)| **b <= k.init + 10.0 && **bb).count() as f64;
    let jp = k.b.iter().zip(&k.beli).find(|(_, bb)| !**bb).map_or(-1.0, |(b, _)| if *b <= tb { (b - k.init) / 10.0 } else { -1.0 });
    let res = k.res[i - 1];
    let res0 = k.res[0];
    Some(vec![
        t, p / p60, p / k.p[0], p / ph(tb - 300.0), p / ph(tb - 600.0), p / ph(tb - 3000.0), mx / p60, p / mx, (tb - tmx) / 10.0,
        nb60, ns60, nb300, ns300, eb60, es60, eb300, es300, nb60 / (nbp + 1.0), i as f64, bm1, jm1, ba, ph(d) / k.p[0],
        (res.max(1e-9)).ln(), k.fee, jp, res / res0.max(1e-12),
    ])
}

/// potret <dir> <ds> <out.tsv>: label \t fitur... \t ds \t pool \t umur \t ret \t blok_masuk \t blok_keluar \t cadangan_eth
pub fn potret(dir: &str, ds: &str, out: &str) {
    let ks = muat(dir);
    let mut o = String::new();
    let mut n = 0;
    for k in &ks {
        for t in UMUR {
            let tb = k.init + t * 10.0;
            if k.rug <= tb { break; } // sudah rug: tidak bisa dibeli lagi
            let Some(x) = fitur(k, t) else { continue };
            let Some((ret, keluar)) = tp_sl(k, tb, 1.5, 0.3) else { continue };
            let y = if ret >= 1.3 { 1 } else { 0 };
            o.push_str(&format!("{y}"));
            for v in &x { o.push_str(&format!("\t{}", if v.is_finite() { format!("{v}") } else { "nan".into() })); }
            o.push_str(&format!("\t{ds}\t{}\t{t}\t{ret}\t{tb}\t{keluar}\t{}\n", k.id, x[23].exp()));
            n += 1;
        }
    }
    std::fs::write(out, o).unwrap();
    eprintln!("{ds}: {} koin, {n} potret", ks.len());
}

/// dagang <skor.tsv> <ambang> <kelly_seed_rata> : akun $40, 1/4 Kelly, maks 6 posisi, eksposur 50%, LP cap 1% cadangan,
/// dampak harga, gas $0.21/trade, ETH $2400. Satu beli per koin: potret PERTAMA yang skornya >= ambang.
/// skor.tsv: ds|pool|umur \t ret \t blok_masuk \t blok_keluar \t cadangan_eth \t skor
pub fn dagang(path: &str, thr: f64, seed: &str) {
    let eth_usd = 2400.0;
    let mut rows: Vec<(String, f64, f64, f64, f64, f64)> = vec![]; // pool, masuk, keluar, ret, res, skor
    for l in std::fs::read_to_string(path).unwrap().lines() {
        let p: Vec<&str> = l.split('\t').collect();
        if p.len() < 6 { continue; }
        let s: f64 = p[5].parse().unwrap_or(0.0);
        if s < thr { continue; }
        let pool = p[0].rsplitn(2, '|').nth(1).unwrap_or("").to_string();
        rows.push((pool, p[2].parse().unwrap(), p[3].parse().unwrap(), p[1].parse().unwrap(), p[4].parse().unwrap(), s));
    }
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let mut sudah = std::collections::HashSet::new();
    let seed_txt = std::fs::read_to_string(seed).unwrap_or_default();
    let mut rets: Vec<f64> = seed_txt.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let awal: f64 = std::env::var("AWAL").ok().and_then(|v| v.parse().ok()).unwrap_or(40.0);
    let (mut kas, mut puncak, mut ddmax) = (awal, awal, 0.0f64);
    let mut buka: Vec<(f64, f64, f64)> = vec![]; // keluar, ukuran, ret_bersih
    let (mut n, mut menang, mut lewat) = (0, 0, 0);
    let kelly = |r: &Vec<f64>| -> f64 {
        let (mut best, mut bv) = (0.02f64, -1e9f64);
        let mut fr = 0.01;
        while fr <= 0.6 {
            let mut s = 0.0; let mut ok = true;
            for x in r { let v = 1.0 + fr * (x - 1.0); if v <= 0.0 { ok = false; break } s += v.ln(); }
            if ok && s / r.len() as f64 > bv { bv = s / r.len() as f64; best = fr; }
            fr += 0.005;
        }
        (best / 4.0).min(0.15).max(0.02)
    };
    for (pool, masuk, keluar, ret, res, _) in rows {
        if !sudah.insert(pool) { continue; }
        // tutup posisi yang sudah keluar sebelum masuk ini
        buka.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        while let Some(&(k, uk, r)) = buka.first() {
            if k > masuk { break; }
            kas += uk * r - 0.21;
            if uk * r - 0.21 > uk { menang += 1; }
            rets.push(r);
            buka.remove(0);
            let eq = kas + buka.iter().map(|x| x.1).sum::<f64>();
            puncak = puncak.max(eq);
            ddmax = ddmax.max(1.0 - eq / puncak);
        }
        let eq = kas + buka.iter().map(|x| x.1).sum::<f64>();
        if buka.len() >= 6 { lewat += 1; continue; }
        let f = kelly(&rets);
        let mut uk = (f * eq).min(kas - 0.21);
        if eq - kas + uk > 0.5 * eq { lewat += 1; continue; }
        uk = uk.min(0.01 * res * eth_usd); // batas 1% cadangan ETH kolam
        if uk < 0.25 { lewat += 1; continue; }
        // dampak harga beli + jual (constant product): kira-kira (1 - x/R)^2 per sisi
        let x = uk / (res * eth_usd).max(1e-9);
        let rb = ret * (1.0 - x).max(0.0).powi(4);
        kas -= uk;
        buka.push((keluar, uk, rb));
        n += 1;
    }
    for (_, uk, r) in &buka { kas += uk * r - 0.21; if uk * r - 0.21 > *uk { menang += 1; } }
    println!("beli={n} menang={:.0}% akun ${:.0} -> ${:.2} DD={:.0}% dilewati={lewat}", 100.0 * menang as f64 / n.max(1) as f64, awal, kas, 100.0 * ddmax);
}
