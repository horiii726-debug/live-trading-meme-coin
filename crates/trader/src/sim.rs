//! MODE SIMULASI: jalankan bot (kode yang SAMA dengan live) di atas data historis satu periode.
//! Tujuan: membuktikan bot live = backtest. Data sama dengan replay uji maju:
//! pools.csv + swaps_sorted.csv + liq.csv; keputusan menit-1 dari swaps_m1.json + transfers.json
//! (port_features, identik bit dengan JS) dan dev_creator.json. Harga ETH = harga saat pool lahir (sama dengan replay "auto").
use crate::bot::{Akun, Keputusan, Mesin};
use crate::catmodel::CatModel;
use crate::cfg::Cfg;
use num_bigint::BigInt;
use port_engine::engine::{parse_bigint, PoolMeta, Swap};
use port_engine::model::Model;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;

const Q96: f64 = 79228162514264337593543950336.0;
const ETH_POOL: &str = "0x24107d152f14a76d292123265ae3f3c71f863fc2f4ef7ba49d64e78d28ea379e";

fn f(x: &str) -> f64 {
    x.trim().parse::<f64>().unwrap_or(f64::NAN)
}

pub struct SimArg {
    pub dir: String,
    pub from: f64,
    pub to: f64,
    pub bot_dir: String,
    pub dev_pre: Option<String>,
    pub out: String,
    pub skor: Option<String>,
    pub thr0: Option<f64>,
    pub danger: Option<String>,
    pub naik: Option<String>,
    pub akun_in: Option<String>,
    pub akun_out: Option<String>,
}

pub fn jalan(cfg: Cfg, a: SimArg) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let rd = |n: &str| std::fs::read_to_string(format!("{}/{}", a.dir, n)).map_err(|e| format!("{n}: {e}"));
    // pool
    let mut metas: HashMap<String, PoolMeta> = HashMap::new();
    for l in rd("pools.csv")?.lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 7 {
            continue;
        }
        let init = f(p[6]);
        if init < a.from || init > a.to {
            continue;
        }
        let id = p[0].to_lowercase();
        metas.insert(id.clone(), PoolMeta { id, c0: p[1].to_lowercase(), c1: p[2].to_lowercase(), fee: f(p[3]), hooks: p[5].to_lowercase(), init });
    }
    // swap: semua pool terpilih + pool ETH (harga ETH historis)
    let txt = rd("swaps_sorted.csv")?;
    let mut eth: Vec<(f64, f64)> = Vec::new();
    let mut ev: Vec<(f64, f64, String, Swap)> = Vec::new();
    for l in txt.lines() {
        let id = &l[..l.find(',').unwrap_or(0)];
        if id == ETH_POOL {
            let p: Vec<&str> = l.split(',').collect();
            let sp = f(p[6]) / Q96;
            eth.push((f(p[1]), sp * sp * 1e12));
            continue;
        }
        if !metas.contains_key(id) {
            continue;
        }
        let p: Vec<&str> = l.split(',').collect();
        let blk = f(p[1]);
        if blk > a.to + 324000.0 {
            continue;
        }
        let s = Swap { blk, li: f(p[2]), tx: p[3].to_string(), a0: f(p[4]), a1: f(p[5]), sqrt_p: f(p[6]) / Q96, l: f(p[7]), fee: f(p[9]) };
        ev.push((blk, s.li, id.to_string(), s));
    }
    drop(txt);
    eth.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let eth_at = |b: f64| -> f64 {
        let i = eth.partition_point(|x| x.0 <= b);
        if i == 0 { eth.first().map_or(f64::NAN, |x| x.1) } else { eth[i - 1].1 }
    };
    let mut liq: Vec<(f64, f64, String, BigInt)> = Vec::new();
    for l in rd("liq.csv")?.lines().skip(1) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 6 {
            continue;
        }
        let id = p[0].to_lowercase();
        if !metas.contains_key(&id) {
            continue;
        }
        liq.push((f(p[1]), f(p[2]), id, parse_bigint(p[5].trim())?));
    }
    ev.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap().then(x.1.partial_cmp(&y.1).unwrap()));
    liq.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap().then(x.1.partial_cmp(&y.1).unwrap()));
    eprintln!("sim: {} pool, {} swap, {} likuiditas, {} titik harga ETH ({:.1} dtk)", metas.len(), ev.len(), liq.len(), eth.len(), t0.elapsed().as_secs_f64());

    // keputusan menit-1 (fitur port_features + filter CatBoost + dev + honeypot)
    let cat = CatModel::load(&format!("{}/filter_cat.json", a.bot_dir)).map_err(|e| e.to_string())?;
    let jobs = port_features::dataset::load_dataset(std::path::Path::new(&a.dir))?;
    let devs: serde_json::Value = serde_json::from_str(&rd("dev_creator.json").unwrap_or("{}".into())).unwrap_or_default();
    // --skor: skor dari luar (mis. skor ML KAUSAL uji maju), menggantikan model filter beku
    let skor_luar: Option<HashMap<String, f64>> = match &a.skor {
        Some(p) => {
            let t = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
            let mut it = t.lines();
            let h: Vec<&str> = it.next().unwrap_or("").split(',').collect();
            let (ip_, is_) = (h.iter().position(|x| *x == "pool").ok_or("kolom pool")?, h.iter().position(|x| *x == "score").ok_or("kolom score")?);
            let dsn = std::path::Path::new(&a.dir).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
            let ids = h.iter().position(|x| *x == "ds");
            let mut m = HashMap::new();
            for l in it {
                let p: Vec<&str> = l.split(',').collect();
                if let Some(i) = ids { if p.get(i) != Some(&dsn.as_str()) { continue; } }
                if let (Some(pl), Some(sc)) = (p.get(ip_), p.get(is_)) { m.insert(pl.to_string(), f(sc)); }
            }
            Some(m)
        }
        None => None,
    };
    let mut kep: Vec<(f64, String, Keputusan)> = Vec::new();
    let mut skor_out = String::new();
    for j in &jobs {
        if !metas.contains_key(&j.pool) {
            continue;
        }
        let fe = port_features::features(&j.meta, &j.swaps, &j.transfers, j.eth_usd);
        let x: Vec<f64> = cat.feats.iter().map(|k| fe.get(k).filter(|v| v.is_finite()).unwrap_or(-1.0)).collect();
        let score = match &skor_luar { Some(m) => m.get(&j.pool).cloned().unwrap_or(f64::NAN), None => cat.predict(&x) };
        let q1 = j.meta.c1 == port_features::NATIVE;
        let jual = j.swaps.iter().filter(|s| (if q1 { s.a0 } else { s.a1 }) < 0.0).count() as i64;
        let dev = devs.get(&j.pool).and_then(|d| d.get("dev")).and_then(|d| d.as_str()).map(|s| s.to_string());
        skor_out.push_str(&format!("{},{}\n", j.pool, score));
        kep.push((j.meta.init + 600.0, j.pool.clone(), Keputusan { score, ml_ok: false, dev, jual_m1: jual, lp_m1: fe.get("lp_m1").unwrap_or(f64::NAN), feats: x.clone(), dikecualikan: None }));
    }
    kep.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    std::fs::write(format!("{}.skor.csv", a.out), format!("pool,score\n{skor_out}")).ok();

    let thr0 = {
        let t = std::fs::read_to_string(format!("{}/filter_cat.json", a.bot_dir)).map_err(|e| e.to_string())?;
        serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string())?["thr_start"].as_f64().unwrap_or(0.195)
    };
    let thr0 = a.thr0.unwrap_or(thr0);
    let mut akun = Akun::new(cfg.clone(), thr0);
    if let Some(p) = &a.akun_in {
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?).map_err(|e| e.to_string())?;
        akun.dari_json(&j);
    }
    if let Some(p) = &a.dev_pre {
        // riwayat dev dari periode SEBELUMNYA (kolom dev & ret), sama dengan DEVPRE arena
        let t = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
        let mut it = t.lines();
        let h: Vec<&str> = it.next().unwrap_or("").split(',').collect();
        let (idv, ir) = (h.iter().position(|x| *x == "dev"), h.iter().position(|x| *x == "ret"));
        if let (Some(idv), Some(ir)) = (idv, ir) {
            for l in it {
                let p: Vec<&str> = l.split(',').collect();
                if p.len() > idv.max(ir) && !p[idv].is_empty() {
                    akun.dev_hist.entry(p[idv].to_string()).or_default().push(f(p[ir]));
                }
            }
        }
        eprintln!("riwayat dev dimuat: {} dev", akun.dev_hist.len());
    }
    let model = Model::load(&a.danger.clone().unwrap_or(format!("{}/danger_model.json", a.bot_dir)))?;
    let model_up = match &a.naik { Some(p) if p == "-" => None, Some(p) => Some(Model::load(p)?), None => Model::load(&format!("{}/pnaik_model.json", a.bot_dir)).ok() };
    let mut m = Mesin::new(akun, model, model_up);

    // putar blok demi blok
    let mut lahir: Vec<&PoolMeta> = metas.values().collect();
    lahir.sort_by(|x, y| x.init.partial_cmp(&y.init).unwrap().then(x.id.cmp(&y.id)));
    let (mut ie, mut il, mut ip, mut ik) = (0usize, 0usize, 0usize, 0usize);
    loop {
        let nb = [ev.get(ie).map(|x| x.0), liq.get(il).map(|x| x.0), lahir.get(ip).map(|x| x.init)]
            .iter()
            .flatten()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        if !nb.is_finite() {
            break;
        }
        while ip < lahir.len() && lahir[ip].init <= nb {
            let pm = lahir[ip].clone();
            let e = eth_at(pm.init);
            m.pool_baru(pm, e);
            ip += 1;
        }
        {
            let mut ak = m.akun.lock().unwrap();
            ak.tip = nb;
            while ik < kep.len() && kep[ik].0 <= nb {
                let (_, pool, k) = kep[ik].clone();
                ak.terima_skor(&pool, k);
                ik += 1;
            }
        }
        let mut per: BTreeMap<String, (Vec<BigInt>, Vec<Swap>)> = BTreeMap::new();
        while il < liq.len() && liq[il].0 == nb {
            per.entry(liq[il].2.clone()).or_default().0.push(liq[il].3.clone());
            il += 1;
        }
        while ie < ev.len() && ev[ie].0 == nb {
            per.entry(ev[ie].2.clone()).or_default().1.push(ev[ie].3.clone());
            ie += 1;
        }
        m.blok(nb, &per);
    }
    m.akhiri();

    // laporan
    let ak = m.akun.lock().unwrap();
    let mut o = String::new();
    for (t, r) in &m.selesai {
        if let Some((pos, back)) = r {
            o.push_str(&format!("{{\"pool\":\"{}\",\"pos\":{:.4},\"kembali\":{:.4},\"ret\":{},\"alasan\":\"{}\",\"masuk\":{},\"keluar\":{}}}\n", t.pool, pos, back, t.ret, t.reason, t.entry_blk, t.exit_blk));
        }
    }
    std::fs::File::create(&a.out).and_then(|mut fh| fh.write_all(o.as_bytes())).map_err(|e| e.to_string())?;
    let pf = {
        let (mut gp, mut gl) = (0.0, 0.0);
        for (_, r) in &m.selesai {
            if let Some((p, b)) = r {
                if b > p { gp += b - p } else { gl += p - b }
            }
        }
        if gl > 0.0 { gp / gl } else { f64::INFINITY }
    };
    println!(
        "SIM {} | beli {} | tutup {} | win {:.1}% | PF {:.2} | akun ${:.2} -> ${:.2} | DD maks {:.1}% | stop darurat {} | {:.1} dtk",
        a.dir,
        ak.n_beli,
        ak.n_tutup,
        100.0 * ak.n_menang as f64 / (ak.n_tutup.max(1)) as f64,
        pf,
        cfg.start_usd,
        ak.ekuitas_beli(),
        100.0 * ak.dd_max,
        if ak.killed { "YA" } else { "tidak" },
        t0.elapsed().as_secs_f64()
    );
    println!("tolak: {:?}", ak.tolak);
    if let Some(p) = &a.akun_out {
        std::fs::write(p, ak.ke_json().to_string()).map_err(|e| e.to_string())?;
    }
    Ok(())
}
