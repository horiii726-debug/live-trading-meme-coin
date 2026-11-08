//! Bedah akar: fitur struktural dari data mentah (LP tick range, tx, blok, ukuran beli) pada umur T (kausal),
//! dan kelas hasil dari jalur harga penuh. Semua koin layak (pasangan ETH, fee <= 1%, tanpa hook, hidup di T).
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};

const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const HZ: f64 = 324000.0;

fn f(x: &str) -> f64 {
    x.trim().parse::<f64>().unwrap_or(f64::NAN)
}
fn h(s: &str) -> u64 {
    let mut x = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut x);
    x.finish()
}

struct Sw { b: f64, li: f64, tx: u64, eth: f64, beli: bool, p: f64 }
struct P { init: f64, c0n: bool, ok: bool, fee: f64, ts: f64, liq: Vec<(f64, f64, f64, f64)>, sw: Vec<Sw> }

pub fn akar(dir: &str, ds: &str, umur: f64, out: &str) {
    let mut k: HashMap<String, P> = HashMap::new();
    for l in std::fs::read_to_string(format!("{dir}/pools.csv")).unwrap().lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 7 { continue; }
        let c0n = p[1] == NATIVE;
        let ok = (c0n || p[2] == NATIVE) && f(p[3]) <= 10000.0 && p[5].to_lowercase() == NATIVE;
        k.insert(p[0].to_lowercase(), P { init: f(p[6]), c0n, ok, fee: f(p[3]), ts: f(p[4]), liq: vec![], sw: vec![] });
    }
    for l in std::fs::read_to_string(format!("{dir}/liq.csv")).unwrap().lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 6 { continue; }
        if let Some(x) = k.get_mut(&p[0].to_lowercase()) { if x.ok { x.liq.push((f(p[1]), f(p[3]), f(p[4]), f(p[5]))); } }
    }
    let fh = std::fs::File::open(format!("{dir}/swaps_sorted.csv")).unwrap();
    for l in BufReader::new(fh).lines().skip(1) {
        let l = l.unwrap();
        let id = &l[..l.find(',').unwrap_or(0)];
        if let Some(x) = k.get_mut(id) {
            if !x.ok { continue; }
            let p: Vec<&str> = l.split(',').collect();
            let (a0, a1, sp) = (f(p[4]), f(p[5]), f(p[6]));
            let eth_d = if x.c0n { a0 } else { a1 };
            let pr = if x.c0n { 1.0 / (sp * sp) } else { sp * sp };
            if !pr.is_finite() || pr <= 0.0 { continue; }
            x.sw.push(Sw { b: f(p[1]), li: f(p[2]), tx: h(p[3]), eth: eth_d.abs() / 1e18, beli: eth_d < 0.0, p: pr });
        }
    }
    let kolom = "ds,pool,init,kelas_roket,kelas_pumprug,kelas_mati,puncak,rug,lp_fullrange,lp_lebar_log,lp_n_tambah,lp_n_kurang,lp_eth_log,tick_spacing,fee,swap_per_tx,maks_swap_per_blok,share_swap_blok_ramai,beli_kembar,beli_bulat,beli_blok_lahir,tunda_beli_pertama,whale_share,jual_share_eth,n_beli,n_tx_beli,beli_median_eth,beli_cv,interval_cv,beli_naik_ukuran,harga_naik,tx_multi_kolam";
    let mut o = String::from(kolom);
    o.push('\n');
    // tx yang muncul di lebih dari satu kolam (router/bot multi-kolam) dalam jendela umur
    let mut tx_kolam: HashMap<u64, HashSet<u64>> = HashMap::new();
    for (id, x) in &k {
        if !x.ok { continue; }
        let t = x.init + umur * 10.0;
        let hid = h(id);
        for s in &x.sw { if s.b <= t { tx_kolam.entry(s.tx).or_default().insert(hid); } }
    }
    let mut ids: Vec<&String> = k.keys().collect();
    ids.sort();
    for id in ids {
        let x = k.get(id).unwrap();
        if !x.ok || x.sw.is_empty() { continue; }
        let t = x.init + umur * 10.0;
        let mut lq = x.liq.clone();
        lq.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (mut add, mut rem, mut rug) = (0.0f64, 0.0f64, f64::INFINITY);
        for &(b, _, _, dl) in &lq {
            if dl > 0.0 { add += dl } else { rem -= dl }
            if add > 0.0 && rem > 0.8 * add { rug = b; break; }
        }
        if rug <= t || add <= 0.0 { continue; }
        let mut sw: Vec<&Sw> = x.sw.iter().collect();
        sw.sort_by(|a, b| a.b.partial_cmp(&b.b).unwrap().then(a.li.partial_cmp(&b.li).unwrap()));
        let pre: Vec<&&Sw> = sw.iter().filter(|s| s.b <= t).collect();
        if pre.is_empty() { continue; }
        // ---- kelas dari jalur sesudah T (masuk T+3)
        // harga dihaluskan: median 3 swap (lonjakan satu swap diabaikan)
        let n = sw.len();
        let ps: Vec<f64> = (0..n).map(|i| { let (a, b, c) = (sw[i.saturating_sub(1)].p, sw[i].p, sw[(i + 1).min(n - 1)].p); a.max(b.min(c)).min(b.max(c)) }).collect();
        let ip = sw.partition_point(|s| s.b <= t + 3.0);
        let p0 = ps[ip.max(1) - 1];
        let akhir = (x.init + HZ).min(rug);
        let (mut pk, mut tpk) = (1.0f64, t);
        for i in ip..n { if sw[i].b >= akhir { break; } let m = ps[i] / p0; if m > pk { pk = m; tpk = sw[i].b; } }
        let cukup_waktu = !rug.is_finite() || rug - tpk >= 600.0; // puncak >= 60 dtk sebelum rug
        let roket = pk >= 6.0 && cukup_waktu;
        let rug9 = rug < x.init + HZ;
        let pumprug = rug9 && !roket && pk >= 1.5;
        let mati = pk < 1.5;
        // ---- fitur LP pada T
        let lp_t: Vec<&(f64, f64, f64, f64)> = lq.iter().filter(|l| l.0 <= t).collect();
        let first = lp_t.iter().find(|l| l.3 > 0.0);
        let full = first.map_or(f64::NAN, |l| if l.1 <= -880000.0 && l.2 >= 880000.0 { 1.0 } else { 0.0 });
        let lebar = first.map_or(f64::NAN, |l| (l.2 - l.1).max(1.0).ln());
        let n_add = lp_t.iter().filter(|l| l.3 > 0.0).count() as f64;
        let n_rem = lp_t.iter().filter(|l| l.3 < 0.0).count() as f64;
        // cadangan ETH tidak tersedia langsung di liq: pakai ETH yang masuk lewat beli - jual + perkiraan dari swap pertama tidak andal -> gunakan L swap pertama
        let lp_eth = {
            let s0 = pre[0];
            // perkiraan kasar dari volume: tidak dipakai bila NaN
            let _ = s0;
            f64::NAN
        };
        // ---- fitur transaksi pada T
        let beli: Vec<&&&Sw> = pre.iter().filter(|s| s.beli).collect();
        let n_beli = beli.len() as f64;
        let mut txs = HashSet::new();
        let mut per_blok: HashMap<i64, usize> = HashMap::new();
        for s in &pre { txs.insert(s.tx); *per_blok.entry(s.b as i64).or_default() += 1; }
        let swap_per_tx = pre.len() as f64 / txs.len().max(1) as f64;
        let maks_blok = *per_blok.values().max().unwrap_or(&0) as f64;
        let ramai = per_blok.values().filter(|v| **v >= 3).map(|v| *v as f64).sum::<f64>() / pre.len() as f64;
        let mut cnt: HashMap<u64, usize> = HashMap::new();
        for s in &beli { *cnt.entry((s.eth * 1e9).round() as u64).or_default() += 1; }
        let kembar = if n_beli > 0.0 { beli.iter().filter(|s| cnt[&((s.eth * 1e9).round() as u64)] >= 2).count() as f64 / n_beli } else { f64::NAN };
        let bulat = if n_beli > 0.0 { beli.iter().filter(|s| { let v = s.eth * 1000.0; (v - v.round()).abs() < 1e-6 && v.round() > 0.0 }).count() as f64 / n_beli } else { f64::NAN };
        let lahir = beli.iter().filter(|s| s.b <= x.init + 1.0).count() as f64;
        let tunda = beli.first().map_or(f64::NAN, |s| s.b - x.init);
        let vol: f64 = beli.iter().map(|s| s.eth).sum();
        let whale = if vol > 0.0 { beli.iter().map(|s| s.eth).fold(0.0, f64::max) / vol } else { f64::NAN };
        let jual_eth: f64 = pre.iter().filter(|s| !s.beli).map(|s| s.eth).sum();
        let jual_share = if vol + jual_eth > 0.0 { jual_eth / (vol + jual_eth) } else { f64::NAN };
        let tx_beli: HashSet<u64> = beli.iter().map(|s| s.tx).collect();
        let mut ukuran: Vec<f64> = beli.iter().map(|s| s.eth).collect();
        ukuran.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = if ukuran.is_empty() { f64::NAN } else { ukuran[ukuran.len() / 2] };
        let rata = if n_beli > 0.0 { vol / n_beli } else { f64::NAN };
        let cv = if n_beli > 1.0 { (beli.iter().map(|s| (s.eth - rata).powi(2)).sum::<f64>() / n_beli).sqrt() / rata } else { f64::NAN };
        let iv: Vec<f64> = beli.windows(2).map(|w| w[1].b - w[0].b).collect();
        let iv_cv = if iv.len() > 1 { let m = iv.iter().sum::<f64>() / iv.len() as f64; if m > 0.0 { (iv.iter().map(|v| (v - m).powi(2)).sum::<f64>() / iv.len() as f64).sqrt() / m } else { f64::NAN } } else { f64::NAN };
        // ukuran beli paruh kedua vs pertama (minat bertumbuh?)
        let naik_uk = if beli.len() >= 4 { let hlf = beli.len() / 2; let a: f64 = beli[..hlf].iter().map(|s| s.eth).sum::<f64>() / hlf as f64; let b: f64 = beli[hlf..].iter().map(|s| s.eth).sum::<f64>() / (beli.len() - hlf) as f64; b / a.max(1e-12) } else { f64::NAN };
        let harga_naik = p0 / pre[0].p;
        let multi = if !pre.is_empty() { pre.iter().filter(|s| tx_kolam.get(&s.tx).map_or(0, |v| v.len()) >= 2).count() as f64 / pre.len() as f64 } else { f64::NAN };
        let v = [roket as i32 as f64, pumprug as i32 as f64, mati as i32 as f64, pk, rug9 as i32 as f64, full, lebar, n_add, n_rem, lp_eth, x.ts, x.fee, swap_per_tx, maks_blok, ramai, kembar, bulat, lahir, tunda, whale, jual_share, n_beli, tx_beli.len() as f64, med, cv, iv_cv, naik_uk, harga_naik, multi];
        o.push_str(&format!("{ds},{id},{}", x.init));
        for z in v { o.push_str(&format!(",{}", if z.is_finite() { format!("{z}") } else { "NaN".into() })); }
        o.push('\n');
    }
    std::fs::File::create(out).unwrap().write_all(o.as_bytes()).unwrap();
}
