//! alat ukur komponen (riset, di luar repo bot). Hanya std.
//! kebenaran <dir> <out.csv> : kebenaran per kolam dari data rantai (pools.csv, liq.csv, swaps_sorted.csv, dev_creator.json)
//! auc <csv> <kolom_skor> <kolom_label> [filter_kolom=nilai] : AUC + presisi 20% teratas
mod akar;
mod jalur;
mod ml;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const H: f64 = 324000.0;

fn f(x: &str) -> f64 {
    x.trim().parse::<f64>().unwrap_or(f64::NAN)
}

struct Kolam {
    init: f64,
    c1_native: bool,
    c0_native: bool,
    fee: f64,
    hooks: String,
    liq: Vec<(f64, f64)>,
    sw: Vec<(f64, f64, f64, f64)>, // blk, a0, a1, sqrtp
}

fn kebenaran(dir: &str, out: &str) {
    let mut k: HashMap<String, Kolam> = HashMap::new();
    let pools = std::fs::read_to_string(format!("{dir}/pools.csv")).expect("pools.csv");
    for l in pools.lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 7 {
            continue;
        }
        k.insert(
            p[0].to_lowercase(),
            Kolam { init: f(p[6]), c0_native: p[1] == NATIVE, c1_native: p[2] == NATIVE, fee: f(p[3]), hooks: p[5].to_lowercase(), liq: vec![], sw: vec![] },
        );
    }
    let liq = std::fs::read_to_string(format!("{dir}/liq.csv")).expect("liq.csv");
    for l in liq.lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 6 {
            continue;
        }
        if let Some(x) = k.get_mut(&p[0].to_lowercase()) {
            x.liq.push((f(p[1]), f(p[5])));
        }
    }
    let fh = std::fs::File::open(format!("{dir}/swaps_sorted.csv")).expect("swaps_sorted.csv");
    for l in BufReader::new(fh).lines().skip(1) {
        let l = l.unwrap();
        let id = &l[..l.find(',').unwrap_or(0)];
        if let Some(x) = k.get_mut(id) {
            let p: Vec<&str> = l.split(',').collect();
            x.sw.push((f(p[1]), f(p[4]), f(p[5]), f(p[6])));
        }
    }
    let devs = std::fs::read_to_string(format!("{dir}/dev_creator.json")).unwrap_or_default();
    let mut dev: HashMap<String, String> = HashMap::new();
    for part in devs.split("},") {
        if let (Some(a), Some(b)) = (part.find("\"0x"), part.find("\"dev\":\"")) {
            if part.len() < a + 67 || part.len() < b + 49 {
                continue;
            }
            let pool = &part[a + 1..a + 67];
            let d = &part[b + 7..b + 7 + 42];
            if d.starts_with("0x") {
                dev.insert(pool.to_lowercase(), d.to_lowercase());
            }
        }
    }
    let mut o = String::from("pool,init,fee,hook,eth_pair,dev,jual_m1,beli_m1,mati_d,rug,rug_dt,max10m,max1j,max9j,akhir,beli_awal,eth_beli,eth_jual,jual_pertama,maks_jual,beli_0_30,beli_30_60,ret_m1,lp0\n");
    let mut extra: Vec<String> = Vec::new();
    let mut ids: Vec<&String> = k.keys().collect();
    ids.sort_by(|a, b| k[*a].init.partial_cmp(&k[*b].init).unwrap());
    // riwayat dev KAUSAL: kolam sebelumnya dari dev yang sama, dan rug-nya yang SUDAH terjadi sebelum keputusan
    let mut dev_kolam: HashMap<String, Vec<(f64, f64)>> = HashMap::new(); // dev -> (init, rug_blk)
    o = o.replace("ret_m1,lp0\n", "ret_m1,lp0,dev_lama,dev_rug_lama,t15,t2,t3\n");
    let mut dev_extra: Vec<String> = Vec::new();
    for id in &ids {
        let x = &k[*id];
        let d = x.init + 600.0;
        let mut lq = x.liq.clone();
        lq.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (mut add, mut rem, mut rb) = (0.0f64, 0.0f64, f64::INFINITY);
        for &(b, dl) in &lq {
            if dl > 0.0 { add += dl } else { rem -= dl }
            if add > 0.0 && rem > 0.8 * add { rb = b; break; }
        }
        match dev.get(*id) {
            Some(dv) => {
                let h = dev_kolam.entry(dv.clone()).or_default();
                let lama = h.iter().filter(|p| p.0 < x.init).count();
                let rug_lama = h.iter().filter(|p| p.1 < d).count();
                dev_extra.push(format!(",{lama},{rug_lama}"));
                h.push((x.init, rb));
            }
            None => dev_extra.push(",NaN,NaN".to_string()),
        }
    }
    dev_extra.reverse();
    for id in ids {
        let x = &k[id];
        let d = x.init + 600.0;
        let e = d + 3.0;
        let eth_pair = x.c0_native || x.c1_native;
        let mut sw = x.sw.clone();
        sw.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut lq = x.liq.clone();
        lq.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // likuiditas kumulatif
        let (mut add, mut rem, mut mati_d, mut rug_blk) = (0.0f64, 0.0f64, false, f64::NAN);
        for &(b, dl) in &lq {
            if dl > 0.0 { add += dl } else { rem -= dl }
            if b <= d && add > 0.0 && rem > 0.8 * add {
                mati_d = true;
            }
            if b > d && rug_blk.is_nan() && add > 0.0 && rem > 0.8 * add {
                rug_blk = b;
            }
        }
        if add <= 0.0 {
            mati_d = true;
        }
        let rug = !mati_d && rug_blk.is_finite() && rug_blk <= d + H;
        // menit-1: penjual/pembeli; beli di 10 blok pertama
        let (mut jual, mut beli, mut awal) = (0, 0, 0);
        let (mut eth_b, mut eth_s, mut jual_pertama, mut maks_jual) = (0.0f64, 0.0f64, f64::NAN, 0.0f64);
        let (mut beli_30_60, mut beli_0_30) = (0, 0);
        for s in &sw {
            if s.0 > d {
                break;
            }
            let eth_d = if x.c0_native { s.1 } else { s.2 };
            let is_beli = eth_d < 0.0; // pemanggil membayar ETH
            if is_beli { beli += 1 } else { jual += 1 }
            if is_beli { eth_b += -eth_d / 1e18 } else { eth_s += eth_d / 1e18; maks_jual = maks_jual.max(eth_d / 1e18) }
            if !is_beli && jual_pertama.is_nan() {
                jual_pertama = s.0 - x.init;
            }
            if is_beli && s.0 > x.init + 300.0 { beli_30_60 += 1 }
            if is_beli && s.0 <= x.init + 300.0 { beli_0_30 += 1 }
            if is_beli && s.0 <= x.init + 10.0 {
                awal += 1;
            }
        }
        let lp0 = lq.iter().find(|l| l.1 > 0.0).map_or(f64::NAN, |l| l.1);
        let harga = |sp: f64| -> f64 { if x.c0_native { 1.0 / (sp * sp) } else { sp * sp } };
        let p_awal = sw.first().map(|s| harga(s.3));
        let p_d = sw.iter().filter(|s| s.0 <= d).last().map(|s| harga(s.3));
        let ret_m1 = match (p_awal, p_d) { (Some(a), Some(b)) => b / a, _ => f64::NAN };
        extra.push(format!("{},{},{},{},{},{},{},{}", eth_b, eth_s, jual_pertama, maks_jual, beli_0_30, beli_30_60, ret_m1, lp0));
        let p0 = sw.iter().filter(|s| s.0 <= e).last().map(|s| harga(s.3));
        let (mut m10, mut m1j, mut m9j, mut akhir) = (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
        let (mut t15, mut t2, mut t3) = (f64::NAN, f64::NAN, f64::NAN);
        if let (Some(p0), false) = (p0, mati_d) {
            let batas = if rug { rug_blk } else { d + H };
            let (mut a, mut b, mut c) = (1.0f64, 1.0f64, 1.0f64);
            let mut last = 1.0;
            for s in sw.iter().filter(|s| s.0 > e && s.0 < batas) {
                let m = harga(s.3) / p0;
                if m >= 1.5 && t15.is_nan() { t15 = s.0 - d }
                if m >= 2.0 && t2.is_nan() { t2 = s.0 - d }
                if m >= 3.0 && t3.is_nan() { t3 = s.0 - d }
                if s.0 <= e + 6000.0 { a = a.max(m) }
                if s.0 <= e + 36000.0 { b = b.max(m) }
                c = c.max(m);
                last = m;
            }
            m10 = a;
            m1j = b;
            m9j = c;
            akhir = if rug { 0.0 } else { last };
        }
        o.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}{}\n",
            id,
            x.init,
            x.fee,
            (x.hooks != NATIVE) as i32,
            eth_pair as i32,
            dev.get(id).cloned().unwrap_or_default(),
            jual,
            beli,
            mati_d as i32,
            rug as i32,
            if rug { rug_blk - d } else { f64::NAN },
            m10,
            m1j,
            m9j,
            akhir,
            awal,
            extra.pop().unwrap(),
            dev_extra.pop().unwrap() + &format!(",{t15},{t2},{t3}")
        ));
    }
    std::fs::File::create(out).unwrap().write_all(o.as_bytes()).unwrap();
}

/// AUC (Mann-Whitney, seri = 0.5) + presisi kuantil teratas
fn auc(path: &str, ks: &str, kl: &str, filt: Option<&str>) {
    let t = std::fs::read_to_string(path).expect("csv");
    let mut it = t.lines();
    let h: Vec<&str> = it.next().unwrap().split(',').collect();
    let pos = |n: &str| h.iter().position(|x| *x == n).unwrap_or_else(|| panic!("kolom {n}"));
    let (is, il) = (pos(ks), pos(kl));
    let fl = filt.map(|s| {
        let (a, b) = s.split_once('=').unwrap();
        (pos(a), b.to_string())
    });
    let mut v: Vec<(f64, bool)> = vec![];
    for l in it {
        let p: Vec<&str> = l.split(',').collect();
        if let Some((i, val)) = &fl {
            if p.get(*i).map(|s| s.to_string()) != Some(val.clone()) {
                continue;
            }
        }
        let s = f(p[is]);
        let y = f(p[il]);
        if s.is_finite() && y.is_finite() {
            v.push((s, y > 0.5));
        }
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let n = v.len();
    let np = v.iter().filter(|x| x.1).count();
    let nn = n - np;
    // rank dengan seri
    let mut rsum = 0.0;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && v[j + 1].0 == v[i].0 {
            j += 1;
        }
        let r = (i + j) as f64 / 2.0 + 1.0;
        for x in &v[i..=j] {
            if x.1 {
                rsum += r;
            }
        }
        i = j + 1;
    }
    let a = if np > 0 && nn > 0 { (rsum - (np * (np + 1)) as f64 / 2.0) / (np as f64 * nn as f64) } else { f64::NAN };
    let top = |q: f64| -> f64 {
        let k = ((n as f64) * q).ceil() as usize;
        let s = &v[n - k.max(1)..];
        s.iter().filter(|x| x.1).count() as f64 / s.len() as f64
    };
    println!("n={n} positif={np} ({:.1}%) AUC={a:.3} presisi_top20%={:.1}% presisi_top10%={:.1}%", 100.0 * np as f64 / n.max(1) as f64, 100.0 * top(0.2), 100.0 * top(0.1));
}

/// trades.jsonl tanpa ethUsd (data 23 jam): tambahkan harga ETH/USDG saat kolam lahir (sama dengan sim.rs eth_at)
fn ethusd(dir: &str, out: &str) {
    const ETH_POOL: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e";
    const Q96: f64 = 79228162514264337593543950336.0;
    let fh = std::fs::File::open(format!("{dir}/swaps_sorted.csv")).expect("swaps");
    let mut eth: Vec<(f64, f64)> = vec![];
    for l in BufReader::new(fh).lines().skip(1) {
        let l = l.unwrap();
        if l.starts_with(ETH_POOL) {
            let p: Vec<&str> = l.split(',').collect();
            let sp = f(p[6]) / Q96;
            eth.push((f(p[1]), sp * sp * 1e12));
        }
    }
    eth.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let at = |b: f64| -> f64 {
        let i = eth.partition_point(|x| x.0 <= b);
        if i == 0 { eth.first().map_or(f64::NAN, |x| x.1) } else { eth[i - 1].1 }
    };
    let t = std::fs::read_to_string(format!("{dir}/trades.jsonl")).expect("trades");
    let mut o = String::new();
    for l in t.lines() {
        let l = l.trim_end();
        if l.is_empty() {
            continue;
        }
        let i = l.find("\"init\":").unwrap() + 7;
        let j = l[i..].find(|c: char| c == ',' || c == '}').unwrap() + i;
        let init = f(&l[i..j]);
        o.push_str(&l[..l.len() - 1]);
        o.push_str(&format!(", \"ethUsd\": {}}}\n", at(init)));
    }
    std::fs::write(out, o).unwrap();
}

/// AUC tiap kolom fitur terhadap satu label; baris disaring k=v (boleh beberapa, dipisah ;)
fn semua_auc(path: &str, kl: &str, filt: &str, mulai: usize) {
    let t = std::fs::read_to_string(path).expect("csv");
    let mut it = t.lines();
    let h: Vec<&str> = it.next().unwrap().split(',').collect();
    let il = h.iter().position(|x| *x == kl).expect("label");
    let fl: Vec<(usize, String)> = filt.split(';').filter(|s| !s.is_empty()).map(|s| {
        let (a, b) = s.split_once('=').unwrap();
        (h.iter().position(|x| *x == a).expect("filter"), b.to_string())
    }).collect();
    let rows: Vec<Vec<&str>> = it.map(|l| l.split(',').collect::<Vec<&str>>()).filter(|p| fl.iter().all(|(i, v)| p.get(*i) == Some(&v.as_str()))).collect();
    for c in mulai..h.len() {
        if c == il {
            continue;
        }
        let mut v: Vec<(f64, bool)> = rows.iter().filter_map(|p| {
            let s = f(p.get(c)?);
            let y = f(p.get(il)?);
            if s.is_finite() && y.is_finite() { Some((s, y > 0.5)) } else { None }
        }).collect();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let n = v.len();
        let np = v.iter().filter(|x| x.1).count();
        let nn = n - np;
        let mut rsum = 0.0;
        let mut i = 0;
        while i < n {
            let mut j = i;
            while j + 1 < n && v[j + 1].0 == v[i].0 {
                j += 1;
            }
            let r = (i + j) as f64 / 2.0 + 1.0;
            for x in &v[i..=j] {
                if x.1 {
                    rsum += r;
                }
            }
            i = j + 1;
        }
        let a = if np > 0 && nn > 0 { (rsum - (np * (np + 1)) as f64 / 2.0) / (np as f64 * nn as f64) } else { f64::NAN };
        println!("{},{:.3},{},{}", h[c], a, n, np);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("kebenaran") => kebenaran(&a[2], &a[3]),
        Some("ethusd") => ethusd(&a[2], &a[3]),
        Some("pola") => jalur::pola(&a[2]),
        Some("sinyal_m2") => jalur::sinyal_m2(&a[2], &a[3]),
        Some("arus") => jalur::arus(&a[2]),
        Some("status") => jalur::status(&a[2], &a[3], &a[4]),
        Some("pilihan") => jalur::pilihan(&a[2], &a[3], a[4].parse().unwrap(), &a[5]),
        Some("akar") => akar::akar(&a[2], &a[3], a[4].parse().unwrap(), &a[5]),
        Some("potret") => ml::potret(&a[2], &a[3], &a[4]),
        Some("dagang") => ml::dagang(&a[2], a[3].parse().unwrap(), a.get(4).map(|s| s.as_str()).unwrap_or("")),
        Some("strategi") => jalur::strategi(&a[2], &a[3]),
        Some("semua_auc") => semua_auc(&a[2], &a[3], &a[4], a[5].parse().unwrap()),
        Some("auc") => auc(&a[2], &a[3], &a[4], a.get(5).map(|s| s.as_str())),
        _ => eprintln!("pakai: kebenaran <dir> <out> | auc <csv> <skor> <label> [k=v]"),
    }
}
