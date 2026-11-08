//! Jalur harga per kolam: pola koin naik + uji aturan entry/TP (riset, hanya std).
//! Harga = harga token dalam ETH dari sqrtPrice swap. Rug = LP ditarik > 80% (kumulatif).
//! Eksekusi: isi di harga swap terakhir <= sinyal + LAT blok; kalau rug terjadi sebelum isi -> nilai 0.
//! Biaya: fee kolam dua kali (beli + jual). Dampak harga ukuran kita TIDAK dihitung (ukuran kecil).
use std::collections::HashMap;
use std::io::{BufRead, BufReader};

const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const LAT: f64 = 3.0;
const HZ: f64 = 324000.0; // 9 jam

fn f(x: &str) -> f64 {
    x.trim().parse::<f64>().unwrap_or(f64::NAN)
}

pub struct K {
    pub init: f64,
    pub fee: f64,
    pub rug: f64, // blok rug (INF kalau tidak)
    pub b: Vec<f64>,
    pub p: Vec<f64>,
    pub beli: Vec<bool>,
    pub eth: Vec<f64>,  // ETH per swap (abs)
    pub res: Vec<f64>,  // perkiraan cadangan ETH kolam (L dan sqrtP)
    pub id: String,
}

impl K {
    fn idx_le(&self, blk: f64) -> Option<usize> {
        let i = self.b.partition_point(|x| *x <= blk);
        if i == 0 { None } else { Some(i - 1) }
    }
    pub fn harga(&self, blk: f64) -> Option<f64> {
        self.idx_le(blk).map(|i| self.p[i])
    }
}

/// kolam layak bot: pasangan ETH, fee <= 1%, tanpa hook, masih hidup di detik 60, ada swap di menit 1
pub fn muat(dir: &str) -> Vec<K> {
    struct M { init: f64, c0n: bool, ok: bool, fee: f64, liq: Vec<(f64, f64)>, sw: Vec<(f64, f64, f64, f64, f64)> }
    let mut k: HashMap<String, M> = HashMap::new();
    for l in std::fs::read_to_string(format!("{dir}/pools.csv")).unwrap().lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 7 { continue; }
        let c0n = p[1] == NATIVE;
        let ok = (c0n || p[2] == NATIVE) && f(p[3]) <= 10000.0 && p[5].to_lowercase() == NATIVE;
        k.insert(p[0].to_lowercase(), M { init: f(p[6]), c0n, ok, fee: f(p[3]), liq: vec![], sw: vec![] });
    }
    for l in std::fs::read_to_string(format!("{dir}/liq.csv")).unwrap().lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 6 { continue; }
        if let Some(x) = k.get_mut(&p[0].to_lowercase()) { if x.ok { x.liq.push((f(p[1]), f(p[5]))); } }
    }
    let fh = std::fs::File::open(format!("{dir}/swaps_sorted.csv")).unwrap();
    for l in BufReader::new(fh).lines().skip(1) {
        let l = l.unwrap();
        let id = &l[..l.find(',').unwrap_or(0)];
        if let Some(x) = k.get_mut(id) {
            if !x.ok { continue; }
            let p: Vec<&str> = l.split(',').collect();
            x.sw.push((f(p[1]), f(p[4]), f(p[5]), f(p[6]), f(p[7])));
        }
    }
    let mut out = vec![];
    let mut ids: Vec<&String> = k.keys().collect();
    ids.sort();
    for id in ids {
        let x = &k[id];
        if !x.ok || x.sw.is_empty() { continue; }
        let d = x.init + 600.0;
        let mut lq = x.liq.clone();
        lq.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (mut add, mut rem, mut rug, mut mati) = (0.0, 0.0, f64::INFINITY, false);
        for &(b, dl) in &lq {
            if dl > 0.0 { add += dl } else { rem -= dl }
            if add > 0.0 && rem > 0.8 * add {
                if b <= d { mati = true }
                rug = b;
                break;
            }
        }
        if mati || add <= 0.0 { continue; }
        let mut sw = x.sw.clone();
        sw.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if !sw.iter().any(|s| s.0 <= d) { continue; }
        let mut kk = K { init: x.init, fee: x.fee, rug, b: vec![], p: vec![], beli: vec![], eth: vec![], res: vec![], id: id.clone() };
        for s in &sw {
            if s.0 > x.init + HZ { break; }
            let pr = if x.c0n { 1.0 / (s.3 * s.3) } else { s.3 * s.3 };
            if !pr.is_finite() || pr <= 0.0 { continue; }
            kk.b.push(s.0);
            kk.p.push(pr);
            kk.beli.push(if x.c0n { s.1 < 0.0 } else { s.2 < 0.0 });
            kk.eth.push((if x.c0n { s.1 } else { s.2 }).abs() / 1e18);
            let sp = s.3 / 79228162514264337593543950336.0;
            kk.res.push(if x.c0n { s.4 / sp } else { s.4 * sp } / 1e18);
        }
        // haluskan: median 3 swap berturut-turut (lonjakan satu swap di kolam kosong diabaikan)
        let n = kk.p.len();
        if n >= 3 {
            let asli = kk.p.clone();
            for i in 0..n {
                let (a, b, c) = (asli[i.saturating_sub(1)], asli[i], asli[(i + 1).min(n - 1)]);
                kk.p[i] = a.max(b.min(c)).min(b.max(c)); // median dari a, b, c
            }
        }
        out.push(kk);
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub enum Keluar {
    Tp(f64),             // jual semua di k x
    Trail(f64),          // jual semua saat turun x dari puncak
    TpTrail(f64, f64),   // jual 50% di k x, sisanya trailing x
    TpStop(f64, f64),    // TP k x atau stop-loss -s
    /// bertingkat: stop -30% sebelum TP pertama; jual f1 di t1, f2 di t2; sisa trailing x dari puncak, lantai harga beli
    Tangga(f64, f64, f64, f64, f64),
    Tahan,               // tahan sampai 9 jam / rug
}

/// hasil satu trade (kelipatan modal, sudah termasuk fee), None kalau tidak bisa masuk
pub fn trade(k: &K, masuk: f64, ex: Keluar) -> Option<f64> {
    let isi = masuk + LAT;
    if k.rug <= isi { return Some(0.0); }
    let p0 = k.harga(isi)?;
    let biaya = (1.0 - k.fee / 1e6).powi(2);
    let akhir = (k.init + HZ).min(k.rug);
    let jual = |sig: f64| -> f64 {
        // isi jual: rug sebelum isi -> 0; tidak pernah lebih baik dari harga saat sinyal
        if k.rug <= sig + LAT { return 0.0; }
        let m_sig = k.harga(sig).map_or(0.0, |p| p / p0);
        k.harga(sig + LAT).map_or(0.0, |p| (p / p0).min(m_sig))
    };
    let i0 = k.b.partition_point(|x| *x <= isi);
    let (mut puncak, mut sisa, mut hasil) = (1.0f64, 1.0f64, 0.0f64);
    let mut tp_done = false;
    for i in i0..k.b.len() {
        let blk = k.b[i];
        if blk >= akhir { break; }
        let m = k.p[i] / p0;
        puncak = puncak.max(m);
        match ex {
            Keluar::Tp(t) => if m >= t { return Some(jual(blk) * biaya); },
            Keluar::Trail(x) => if m <= puncak * (1.0 - x) { return Some(jual(blk) * biaya); },
            Keluar::TpStop(t, s) => if m >= t || m <= 1.0 - s { return Some(jual(blk) * biaya); },
            Keluar::Tangga(f1, t1, f2, t2, x) => {
                if !tp_done && m <= 0.7 { return Some((hasil + sisa * jual(blk)) * biaya); }
                if !tp_done && m >= t1 { hasil += f1 * jual(blk); sisa -= f1; tp_done = true; }
                if tp_done && sisa > 1.0 - f1 - f2 + 1e-9 && m >= t2 { hasil += f2 * jual(blk); sisa -= f2; }
                if tp_done && (m <= puncak * (1.0 - x) || m <= 1.0) { return Some((hasil + sisa * jual(blk)) * biaya); }
            }
            Keluar::TpTrail(t, x) => {
                if !tp_done && m >= t { hasil += 0.5 * jual(blk); sisa = 0.5; tp_done = true; }
                else if tp_done && m <= puncak * (1.0 - x) { return Some((hasil + sisa * jual(blk)) * biaya); }
            }
            Keluar::Tahan => {}
        }
    }
    // tidak ada sinyal: rug -> sisa 0; kalau tidak rug -> jual di akhir horizon
    let v = if k.rug < k.init + HZ { 0.0 } else { k.harga(akhir).map_or(0.0, |p| p / p0) };
    Some((hasil + sisa * v) * biaya)
}

pub fn pola(dir: &str) {
    let ks = muat(dir);
    let mut grup: Vec<(&str, f64)> = vec![("semua", 0.0), ("puncak>=3x", 3.0), ("puncak>=6x", 6.0), ("puncak>=11x", 11.0), ("puncak>=21x", 21.0)];
    grup.reverse();
    let q = |v: &mut Vec<f64>, p: f64| -> f64 { if v.is_empty() { return f64::NAN } v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[((v.len() - 1) as f64 * p) as usize] };
    for (nama, batas) in grup.iter().rev() {
        let (mut n, mut nrug) = (0, 0);
        let (mut t_puncak, mut t2, mut dd, mut t_rug_sesudah, mut beli_m1, mut naik_m1, mut puncakv) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
        for k in &ks {
            let d = k.init + 600.0;
            let Some(p0) = k.harga(d + LAT) else { continue };
            let akhir = (k.init + HZ).min(k.rug);
            let (mut pk, mut tpk, mut low, mut t2x) = (1.0f64, f64::NAN, 1.0f64, f64::NAN);
            let mut low_sebelum = 1.0f64;
            for i in 0..k.b.len() {
                if k.b[i] <= d + LAT || k.b[i] >= akhir { continue; }
                let m = k.p[i] / p0;
                if m > pk { pk = m; tpk = k.b[i]; low_sebelum = low; }
                low = low.min(m);
                if m >= 2.0 && t2x.is_nan() { t2x = k.b[i]; }
            }
            if pk < *batas { continue; }
            n += 1;
            puncakv.push(pk);
            if k.rug.is_finite() { nrug += 1; if tpk.is_finite() { t_rug_sesudah.push((k.rug - tpk) / 10.0); } }
            if tpk.is_finite() { t_puncak.push((tpk - k.init) / 10.0); }
            if t2x.is_finite() { t2.push((t2x - k.init) / 10.0); }
            dd.push(1.0 - low_sebelum);
            beli_m1.push(k.beli.iter().zip(&k.b).filter(|(bb, b)| **bb && **b <= d).count() as f64);
            let pa = k.p[0];
            naik_m1.push(k.harga(d).unwrap_or(pa) / pa);
        }
        if n == 0 { continue; }
        println!(
            "{nama:12} n={n:5} rug={:3.0}% | puncak median {:.1}x | waktu ke 2x (median) {:.0} dtk | waktu ke puncak p25/median/p75 {:.0}/{:.0}/{:.0} dtk | turun dulu sebelum puncak median {:.0}% p75 {:.0}% | rug sesudah puncak median {:.0} dtk | beli menit-1 median {:.0} | naik menit-1 median {:.2}x",
            100.0 * nrug as f64 / n as f64, q(&mut puncakv, 0.5), q(&mut t2, 0.5), q(&mut t_puncak.clone(), 0.25), q(&mut t_puncak.clone(), 0.5), q(&mut t_puncak, 0.75),
            100.0 * q(&mut dd.clone(), 0.5), 100.0 * q(&mut dd, 0.75), q(&mut t_rug_sesudah, 0.5), q(&mut beli_m1, 0.5), q(&mut naik_m1, 0.5)
        );
    }
}

/// grid aturan entry x keluar; cetak csv: entry,keluar,n,win,rata,median,pf
pub fn strategi(dir: &str, ds: &str) {
    let ks = muat(dir);
    let keluar: Vec<(String, Keluar)> = vec![
        ("tahan".into(), Keluar::Tahan),
        ("tp1.5".into(), Keluar::Tp(1.5)), ("tp2".into(), Keluar::Tp(2.0)), ("tp3".into(), Keluar::Tp(3.0)), ("tp5".into(), Keluar::Tp(5.0)), ("tp10".into(), Keluar::Tp(10.0)),
        ("trail20".into(), Keluar::Trail(0.2)), ("trail30".into(), Keluar::Trail(0.3)), ("trail50".into(), Keluar::Trail(0.5)),
        ("tp2+trail30".into(), Keluar::TpTrail(2.0, 0.3)), ("tp2+trail50".into(), Keluar::TpTrail(2.0, 0.5)), ("tp3+trail50".into(), Keluar::TpTrail(3.0, 0.5)),
        ("tp2/sl30".into(), Keluar::TpStop(2.0, 0.3)), ("tp3/sl50".into(), Keluar::TpStop(3.0, 0.5)),
    ];
    // entry: (nama, detik sesudah lahir, syarat naik dari harga detik-60, syarat beli dalam 30 dtk terakhir)
    let entri: Vec<(&str, f64, f64, usize)> = vec![
        ("d60", 60.0, 0.0, 0), ("d60_aktif3", 60.0, 0.0, 3),
        ("m2", 120.0, 0.0, 0), ("m2_naik1.5", 120.0, 1.5, 0), ("m2_aktif3", 120.0, 0.0, 3),
        ("m5", 300.0, 0.0, 0), ("m5_naik2", 300.0, 2.0, 0), ("m5_aktif5", 300.0, 0.0, 5),
        ("m10", 600.0, 0.0, 0), ("m10_naik2", 600.0, 2.0, 0), ("m10_aktif5", 600.0, 0.0, 5),
    ];
    for (en, det, naik, aktif) in &entri {
        let mut kandidat = vec![];
        for k in &ks {
            let t = k.init + det * 10.0;
            if k.rug <= t { continue; } // sudah rug sebelum entry: tidak bisa dibeli
            let (Some(p60), Some(pt)) = (k.harga(k.init + 600.0), k.harga(t)) else { continue };
            if *naik > 0.0 && pt / p60 < *naik { continue; }
            if *aktif > 0 {
                let n = k.b.iter().zip(&k.beli).filter(|(b, bb)| **bb && **b > t - 300.0 && **b <= t).count();
                if n < *aktif { continue; }
            }
            kandidat.push((k, t));
        }
        for (xn, ex) in &keluar {
            let mut r: Vec<f64> = kandidat.iter().filter_map(|(k, t)| trade(k, *t, *ex)).map(|x| x.min(200.0)).collect();
            if r.is_empty() { continue; }
            let n = r.len();
            let win = r.iter().filter(|x| **x > 1.0).count() as f64 / n as f64;
            let rata = r.iter().sum::<f64>() / n as f64;
            let (gp, gl) = r.iter().fold((0.0, 0.0), |(a, b), x| if *x > 1.0 { (a + x - 1.0, b) } else { (a, b + 1.0 - x) });
            r.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = r[n / 2];
            // rata tanpa 5 trade terbaik (cek: untung dari segelintir koin?)
            let tanpa5 = if n > 5 { r[..n - 5].iter().sum::<f64>() / (n - 5) as f64 } else { f64::NAN };
            println!("{ds},{en},{xn},{n},{:.3},{:.3},{:.3},{:.2},{:.3}", win, rata, med, if gl > 0.0 { gp / gl } else { f64::INFINITY }, tanpa5);
        }
    }
}

/// pilihan <dir> <pools.txt> <umur_detik> <ds>: hasil aturan keluar untuk kolam terpilih (beli di umur itu)
pub fn pilihan(dir: &str, daftar: &str, umur: f64, ds: &str) {
    let pilih: std::collections::HashSet<String> = std::fs::read_to_string(daftar).unwrap().lines().map(|s| s.trim().to_lowercase()).collect();
    let ks = muat(dir);
    let keluar: Vec<(&str, Keluar)> = vec![
        ("tahan9j", Keluar::Tahan), ("tp1.5", Keluar::Tp(1.5)), ("tp2", Keluar::Tp(2.0)), ("tp3", Keluar::Tp(3.0)), ("tp5", Keluar::Tp(5.0)), ("tp10", Keluar::Tp(10.0)),
        ("trail30", Keluar::Trail(0.3)), ("trail50", Keluar::Trail(0.5)), ("tp2+trail50", Keluar::TpTrail(2.0, 0.5)), ("tp3+trail50", Keluar::TpTrail(3.0, 0.5)), ("tp2/sl30", Keluar::TpStop(2.0, 0.3)),
        ("50%@2x+sisa_trail40", Keluar::Tangga(0.5, 2.0, 0.0, 99.0, 0.4)), ("50%@2x+sisa_trail60", Keluar::Tangga(0.5, 2.0, 0.0, 99.0, 0.6)),
        ("33%@2x+33%@5x+trail40", Keluar::Tangga(0.34, 2.0, 0.33, 5.0, 0.4)), ("33%@2x+33%@5x+trail60", Keluar::Tangga(0.34, 2.0, 0.33, 5.0, 0.6)),
        ("25%@2x+25%@5x+trail50", Keluar::Tangga(0.25, 2.0, 0.25, 5.0, 0.5)), ("50%@3x+sisa_trail50", Keluar::Tangga(0.5, 3.0, 0.0, 99.0, 0.5)),
    ];
    let kand: Vec<&K> = ks.iter().filter(|k| pilih.contains(&k.id) && k.rug > k.init + umur * 10.0).collect();
    for (xn, ex) in &keluar {
        let mut r: Vec<f64> = kand.iter().filter_map(|k| trade(k, k.init + umur * 10.0, *ex)).map(|x| x.min(200.0)).collect();
        if r.is_empty() { continue; }
        let n = r.len();
        let win = r.iter().filter(|x| **x > 1.0).count() as f64 / n as f64;
        let rata = r.iter().sum::<f64>() / n as f64;
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let t5 = if n > 5 { r[..n - 5].iter().sum::<f64>() / (n - 5) as f64 } else { f64::NAN };
        let rug0 = r.iter().filter(|x| **x < 0.05).count() as f64 / n as f64;
        let x5 = r.iter().filter(|x| **x >= 5.0).count();
        let mx = r[n - 1];
        println!("{ds},{xn},n={n},win={:.0}%,rata={:.3},tanpa_top5={:.3},hilang_total={:.0}%,trade>=5x={x5},terbesar={mx:.1}x", 100.0 * win, rata, t5, 100.0 * rug0);
    }
}

/// status <dir> <beli.txt> <seed_rets> : posisi kertas (beli di detik 60+LAT, TP 2x / stop -30% / rug), akun $40 1/4 Kelly + gas.
/// beli.txt: pool per baris. Cetak tiap posisi + ringkasan akun. Tanpa transaksi apa pun.
pub fn status(dir: &str, daftar: &str, seed: &str) {
    let pilih: Vec<String> = std::fs::read_to_string(daftar).unwrap_or_default().lines().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let ks = muat_semua(dir);
    let tip = ks.iter().filter_map(|k| k.b.last().copied()).fold(0.0, f64::max);
    let mut rets: Vec<f64> = std::fs::read_to_string(seed).unwrap_or_default().split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let n_seed = rets.len();
    let mut baris: Vec<(f64, String, String, f64, f64)> = vec![]; // masuk, pool, status, ret/m, keluar
    for id in &pilih {
        let Some(k) = ks.iter().find(|k| &k.id == id) else { baris.push((0.0, id.clone(), "data belum ada".into(), f64::NAN, f64::NAN)); continue };
        let isi = k.init + 603.0;
        let i0 = k.b.partition_point(|x| *x <= isi);
        if i0 == 0 { baris.push((isi, id.clone(), "tanpa harga".into(), f64::NAN, f64::NAN)); continue; }
        let p0 = k.p[i0 - 1];
        let biaya = (1.0 - k.fee / 1e6).powi(2);
        if k.rug <= isi { baris.push((isi, id.clone(), "TUTUP rug sebelum isi".into(), 0.0, k.rug)); continue; }
        let mut st = None;
        for i in i0..k.b.len() {
            let blk = k.b[i];
            if blk >= k.rug { break; }
            let m = k.p[i] / p0;
            if m >= 2.0 || m <= 0.7 {
                let j = k.b.partition_point(|x| *x <= blk + 3.0) - 1;
                let r = if k.rug <= blk + 3.0 { 0.0 } else { (k.p[j] / p0).min(m) * biaya };
                st = Some((if m >= 2.0 { "TUTUP TP 2x" } else { "TUTUP stop -30%" }, r, blk + 3.0));
                break;
            }
        }
        match st {
            Some((s, r, b)) => baris.push((isi, id.clone(), s.into(), r, b)),
            None if k.rug.is_finite() => baris.push((isi, id.clone(), "TUTUP rug".into(), 0.0, k.rug)),
            None => { let m = k.p[k.b.len() - 1] / p0; baris.push((isi, id.clone(), "TERBUKA".into(), m, f64::NAN)); }
        }
    }
    baris.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let kelly = |r: &Vec<f64>| -> f64 {
        if r.is_empty() { return 0.02; }
        let (mut best, mut bv) = (0.02f64, -1e9f64);
        let mut fr = 0.01;
        while fr <= 0.6 { let mut s = 0.0; let mut ok = true; for x in r { let v = 1.0 + fr * (x - 1.0); if v <= 0.0 { ok = false; break } s += v.ln(); } if ok && s / r.len() as f64 > bv { bv = s / r.len() as f64; best = fr; } fr += 0.005; }
        (best / 4.0).min(0.15).max(0.02)
    };
    let mut kas = 40.0f64;
    let (mut tutup, mut menang) = (0, 0);
    for (_, id, s, r, _) in &baris {
        let f = kelly(&rets);
        let uk = f * kas;
        if s.starts_with("TUTUP") { kas += uk * (r - 1.0) - 0.21; rets.push(*r); tutup += 1; if *r > 1.0 { menang += 1; } }
        println!("{} {:<26} {}", &id[..12.min(id.len())], s, if r.is_finite() { format!("{:.3}x  ukuran ${:.2} (1/4 Kelly {:.1}%)", r, uk, 100.0 * f) } else { "-".into() });
    }
    println!("RINGKAS tip_blok={tip} posisi={} tutup={tutup} menang={menang} akun_kertas $40 -> ${kas:.2} (seed Kelly {n_seed} trade validasi m1)", baris.len());
}

/// muat tanpa syarat hidup di detik 60 (untuk status posisi)
pub fn muat_semua(dir: &str) -> Vec<K> { muat(dir) }

/// arus <dir1,dir2,...>: arus trader per menit sejak lahir (menit 1..30) untuk kelas roket (>=6x, puncak >=60 dtk sebelum rug),
/// pump-rug (rug 9 jam & puncak 1.5-6x) dan mati (<1.5x). Median per menit: beli, jual, ETH bersih masuk, harga vs detik 60.
/// Kelas dihitung dari jalur penuh (deskriptif, BUKAN fitur prediksi).
pub fn arus(dirs: &str) {
    let mut data: [Vec<Vec<(f64, f64, f64, f64)>>; 3] = [vec![], vec![], vec![]];
    for dir in dirs.split(',') {
        for k in muat(dir) {
            let d = k.init + 600.0;
            let Some(p60) = k.harga(d) else { continue };
            let akhir = (k.init + HZ).min(k.rug);
            let (mut pk, mut tpk) = (1.0f64, d);
            for i in 0..k.b.len() { if k.b[i] > d && k.b[i] < akhir { let m = k.p[i] / p60; if m > pk { pk = m; tpk = k.b[i]; } } }
            let roket = pk >= 6.0 && (!k.rug.is_finite() || k.rug - tpk >= 600.0);
            let kelas = if roket { 0 } else if k.rug < k.init + HZ && pk >= 1.5 { 1 } else if pk < 1.5 { 2 } else { continue };
            let mut baris = vec![];
            for m in 1..=30 {
                let (a, b) = (k.init + (m - 1) as f64 * 600.0, k.init + m as f64 * 600.0);
                if k.rug <= a { break; }
                let (mut nb, mut ns, mut net) = (0.0, 0.0, 0.0);
                for i in 0..k.b.len() { if k.b[i] > a && k.b[i] <= b { if k.beli[i] { nb += 1.0; net += k.eth[i]; } else { ns += 1.0; net -= k.eth[i]; } } }
                let pm = k.harga(b).map_or(f64::NAN, |p| p / p60);
                baris.push((nb, ns, net, pm));
            }
            data[kelas].push(baris);
        }
    }
    let nama = ["ROKET >=6x", "PUMP-RUG", "MATI <1.5x"];
    for c in 0..3 {
        println!("== {} (n={})", nama[c], data[c].len());
        println!("menit | median beli | median jual | median ETH bersih | median harga vs detik60 | masih hidup");
        for m in [1usize, 2, 3, 5, 7, 10, 15, 20, 30] {
            let mut v: Vec<&(f64, f64, f64, f64)> = data[c].iter().filter_map(|r| r.get(m - 1)).collect();
            let n = v.len();
            if n == 0 { continue; }
            let med = |f: &dyn Fn(&(f64, f64, f64, f64)) -> f64, v: &mut Vec<&(f64, f64, f64, f64)>| -> f64 { let mut x: Vec<f64> = v.iter().map(|r| f(r)).filter(|x| x.is_finite()).collect(); if x.is_empty() { return f64::NAN } x.sort_by(|a, b| a.partial_cmp(b).unwrap()); x[x.len() / 2] };
            println!("{m:5} | {:11.0} | {:11.0} | {:17.3} | {:23.2} | {:.0}%", med(&|r| r.0, &mut v), med(&|r| r.1, &mut v), med(&|r| r.2, &mut v), med(&|r| r.3, &mut v), 100.0 * n as f64 / data[c].len() as f64);
        }
    }
}

/// sinyal_m2 <dir> <ds>: aturan "sepi di menit 1, lanjut dibeli di menit 2" diuji di umur 120 dtk (hanya data <= 120 dtk).
/// cocok = beli_m1 <= 10 & beli_menit2 >= 3 & jual_menit2 <= 1 & ETH bersih menit2 > 0.
/// Cetak: tingkat roket (>=6x dari harga 120 dtk, puncak >= 60 dtk sebelum rug), rug, dan hasil trade beli 120 dtk.
pub fn sinyal_m2(dir: &str, ds: &str) {
    let ks = muat(dir);
    let mut g: [(usize, usize, usize, Vec<f64>, Vec<f64>); 2] = [(0, 0, 0, vec![], vec![]), (0, 0, 0, vec![], vec![])];
    for k in &ks {
        let t = k.init + 1200.0;
        if k.rug <= t { continue; }
        let (mut b1, mut b2, mut s2, mut net2) = (0, 0, 0, 0.0);
        for i in 0..k.b.len() {
            let x = k.b[i];
            if x <= k.init + 600.0 { if k.beli[i] { b1 += 1 } }
            else if x <= t { if k.beli[i] { b2 += 1; net2 += k.eth[i] } else { s2 += 1; net2 -= k.eth[i] } }
        }
        let cocok = b1 <= 10 && b2 >= 3 && s2 <= 1 && net2 > 0.0;
        let Some(p0) = k.harga(t + LAT) else { continue };
        let akhir = (k.init + HZ).min(k.rug);
        let (mut pk, mut tpk) = (1.0f64, t);
        for i in 0..k.b.len() { if k.b[i] > t + LAT && k.b[i] < akhir { let m = k.p[i] / p0; if m > pk { pk = m; tpk = k.b[i]; } } }
        let roket = pk >= 6.0 && (!k.rug.is_finite() || k.rug - tpk >= 600.0);
        let e = &mut g[cocok as usize];
        e.0 += 1;
        if roket { e.1 += 1 }
        if k.rug < k.init + HZ { e.2 += 1 }
        if let Some(r) = trade(k, t, Keluar::TpStop(2.0, 0.3)) { e.3.push(r.min(200.0)) }
        if let Some(r) = trade(k, t, Keluar::Tangga(0.34, 2.0, 0.33, 5.0, 0.4)) { e.4.push(r.min(200.0)) }
    }
    for (c, nama) in [(1, "COCOK"), (0, "lainnya")] {
        let e = &g[c];
        if e.0 == 0 { continue; }
        let avg = |v: &Vec<f64>| if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 };
        let win = e.3.iter().filter(|x| **x > 1.0).count() as f64 / e.3.len().max(1) as f64;
        println!("{ds},{nama},n={},roket={:.1}%,rug={:.0}%,TP2/SL30 rata={:+.0}% win={:.0}%,tangga rata={:+.0}%", e.0, 100.0 * e.1 as f64 / e.0 as f64, 100.0 * e.2 as f64 / e.0 as f64, 100.0 * (avg(&e.3) - 1.0), 100.0 * win, 100.0 * (avg(&e.4) - 1.0));
    }
}
