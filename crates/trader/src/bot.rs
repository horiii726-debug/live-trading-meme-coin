//! Inti bot (dipakai SAMA PERSIS oleh mode live dan mode simulasi):
//! - akun kertas dengan aturan uji maju resmi (data_b/arena.js): 1/4 Kelly, pemanasan 2%/30 trade,
//!   9,4% s/d 100 trade, batas 10%, rem drawdown 10/20/30%, stop darurat 60%, eksposur 50%, posisi maks,
//!   batas 1% likuiditas pool, cadangan gas;
//! - gerbang beli (filter ML 10% teratas dari 100 skor terakhir, aturan dev, aturan honeypot);
//! - posisi dikelola mesin keluar v4 (port_engine::PoolSim, identik dengan backtest).
use crate::cfg::Cfg;
use port_engine::engine::{self as eng, Event, PoolMeta, PoolSim, SimOpts, Swap, Thresholds, Trade};
use port_engine::model::Model;
use num_bigint::BigInt;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Hasil pengecekan menit-1 untuk satu koin.
#[derive(Clone, Debug)]
pub struct Keputusan {
    pub score: f64,
    pub ml_ok: bool,
    pub dev: Option<String>,
    pub jual_m1: i64,
    pub lp_m1: f64,
    /// 34 fitur menit-1 (urutan = feats model filter) — disimpan ke label untuk latih ulang
    pub feats: Vec<f64>,
    /// Some(alasan) = aset bukan meme coin (mis. saham ter-tokenisasi) atau gagal diklasifikasi -> TIDAK BOLEH dibeli
    pub dikecualikan: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Posisi {
    pub pos: f64,
    pub lp: f64,
    pub dev: Option<String>,
    pub blk: f64,
}

pub struct Akun {
    pub cfg: Cfg,
    pub cash: f64,
    pub peak: f64,
    pub dd_max: f64,
    pub killed: bool,
    pub open: BTreeMap<String, Posisi>,
    pub rets: Vec<f64>,
    pub recent: VecDeque<f64>,
    pub thr0: f64,
    pub dev_hist: HashMap<String, Vec<f64>>,
    pub kep: HashMap<String, Keputusan>,
    pub pending: HashMap<String, f64>,
    pub mtm: HashMap<String, f64>,
    pub tip: f64,
    pub live: bool,
    /// the live bot replaying its own history after a restart (the backtest has neither flag set)
    pub replay: bool,
    pub tolak: BTreeMap<&'static str, u64>,
    pub n_beli: u64,
    pub n_tutup: u64,
    pub n_menang: u64,
    pub log: Vec<String>,
    pub hari: Option<(i64, f64)>,
    pub eq_curve: Vec<(f64, f64)>,
}

impl Akun {
    pub fn new(cfg: Cfg, thr0: f64) -> Akun {
        Akun {
            cash: cfg.start_usd,
            peak: cfg.start_usd,
            cfg,
            dd_max: 0.0,
            killed: false,
            open: BTreeMap::new(),
            rets: Vec::new(),
            recent: VecDeque::new(),
            thr0,
            dev_hist: HashMap::new(),
            kep: HashMap::new(),
            pending: HashMap::new(),
            mtm: HashMap::new(),
            tip: 0.0,
            live: false,
            replay: false,
            tolak: BTreeMap::new(),
            n_beli: 0,
            n_tutup: 0,
            n_menang: 0,
            log: Vec::new(),
            hari: None,
            eq_curve: Vec::new(),
        }
    }
    /// = q() arena.js: s[min(len-1, floor(p*len))] dari salinan terurut
    fn kuantil(a: &VecDeque<f64>, p: f64) -> f64 {
        let mut s: Vec<f64> = a.iter().cloned().collect();
        s.sort_by(|x, y| x.partial_cmp(y).unwrap());
        s[(s.len() - 1).min((p * s.len() as f64).floor() as usize)]
    }
    /// = kelly() arena.js
    pub fn kelly(&self) -> f64 {
        let r = &self.rets;
        if r.len() < 30 {
            return 0.02;
        }
        if r.len() < 100 {
            return 0.094;
        }
        let (mut best, mut bv) = (0.02f64, -1e9f64);
        let mut f = 0.01f64;
        while f <= 0.6 {
            let (mut s, mut ok) = (0.0f64, true);
            for x in r {
                let v = 1.0 + f * (x - 1.0);
                if v <= 0.0 {
                    ok = false;
                    break;
                }
                s += v.ln();
            }
            if ok && s / r.len() as f64 > bv {
                bv = s / r.len() as f64;
                best = f;
            }
            f += 0.005;
        }
        (best / 4.0).min(self.cfg.kelly_maks).max(self.cfg.kelly_min)
    }
    /// Simpan keadaan akun (stop darurat, puncak, riwayat Kelly, 100 skor terakhir, riwayat dev). Posisi terbuka TIDAK
    /// disimpan di sini: di live posisi dibangun ulang dari putar ulang event tersimpan (deterministik).
    pub fn ke_json(&self) -> serde_json::Value {
        serde_json::json!({
            "cash": self.cash, "peak": self.peak, "dd_max": self.dd_max, "killed": self.killed,
            "rets": self.rets, "recent": self.recent.iter().cloned().collect::<Vec<f64>>(),
            "dev_hist": self.dev_hist, "n_beli": self.n_beli, "n_tutup": self.n_tutup, "n_menang": self.n_menang,
            "open": self.open.len(),
        })
    }
    pub fn dari_json(&mut self, j: &serde_json::Value) {
        let fl = |k: &str, d: f64| j.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
        self.cash = fl("cash", self.cash);
        self.peak = fl("peak", self.peak);
        self.dd_max = fl("dd_max", self.dd_max);
        self.killed = j.get("killed").and_then(|v| v.as_bool()).unwrap_or(false);
        let arr = |k: &str| -> Vec<f64> { j.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default() };
        self.rets = arr("rets");
        self.recent = arr("recent").into_iter().collect();
        if let Some(o) = j.get("dev_hist").and_then(|v| v.as_object()) {
            for (k, v) in o {
                self.dev_hist.insert(k.clone(), v.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default());
            }
        }
        self.n_beli = fl("n_beli", 0.0) as u64;
        self.n_tutup = fl("n_tutup", 0.0) as u64;
        self.n_menang = fl("n_menang", 0.0) as u64;
    }
    pub fn ekuitas_beli(&self) -> f64 {
        self.cash + self.open.values().map(|p| p.pos).sum::<f64>()
    }
    pub fn ekuitas_pasar(&self) -> f64 {
        self.cash + self.open.iter().map(|(k, p)| p.pos * self.mtm.get(k).cloned().unwrap_or(1.0)).sum::<f64>()
    }
    fn tolak(&mut self, why: &'static str) -> Option<bool> {
        *self.tolak.entry(why).or_insert(0) += 1;
        Some(false)
    }
    /// Skor baru masuk (urutan sama dengan arena: skor didorong dulu, baru ambang dihitung).
    pub fn terima_skor(&mut self, pool: &str, mut k: Keputusan) {
        if k.score.is_finite() {
            self.recent.push_back(k.score);
            if self.recent.len() > 100 {
                self.recent.pop_front();
            }
            let thr = if self.recent.len() < 30 { self.thr0 } else { Self::kuantil(&self.recent, self.cfg.qthr) };
            k.ml_ok = k.score >= thr;
        } else {
            k.ml_ok = false;
        }
        self.kep.insert(pool.to_string(), k);
    }
    /// GERBANG BELI. None = tunggu hasil cek, Some(false) = tolak, Some(true) = beli (ukuran disimpan di `pending`).
    pub fn gerbang(&mut self, sim: &PoolSim) -> Option<bool> {
        let id = sim.m.id.clone();
        let telat = self.tip - (sim.m.init + 630.0);
        // The live bot and its replay after a restart follow the same rules: the replay applies every stored minute-1
        // result at the block it arrived at live, so it must also WAIT for it. (Rejecting at once, as the backtest
        // does where every decision is known up front, made a restart forget most open positions.)
        let as_live = self.live || self.replay;
        if as_live && telat > self.cfg.fresh_blocks {
            return self.tolak("data basi");
        }
        let k = match self.kep.get(&id) {
            Some(k) => k.clone(),
            None => {
                if as_live && telat > 100.0 {
                    return self.tolak("cek telat >10 dtk");
                }
                if !as_live {
                    return self.tolak("tanpa keputusan");
                }
                return None;
            }
        };
        if k.dikecualikan.is_some() {
            return self.tolak("bukan meme coin / belum terklasifikasi");
        }
        if self.cfg.filter {
            if !k.score.is_finite() {
                return self.tolak("model belum siap");
            }
            if !k.ml_ok {
                return self.tolak("ml");
            }
        }
        if self.cfg.dev_rule {
            if let Some(d) = &k.dev {
                if self.dev_hist.get(d).map_or(false, |h| h.iter().any(|r| *r < 0.3)) {
                    return self.tolak("dev pernah rug");
                }
            }
        }
        if self.cfg.honeypot && k.jual_m1 == 0 {
            return self.tolak("honeypot (belum ada penjual)");
        }
        if self.killed {
            return self.tolak("stop darurat");
        }
        if self.open.len() >= self.cfg.max_pos {
            return self.tolak("slot penuh");
        }
        if self.cfg.larang_pemanasan && self.recent.len() < 30 {
            return self.tolak("pemanasan model");
        }
        if self.cfg.rem_harian > 0.0 {
            let hari = ((sim.m.init * 0.1013 / 86400.0) as i64, ());
            let eq = self.ekuitas_pasar();
            match self.hari {
                Some((h, _)) if h == hari.0 => {}
                _ => self.hari = Some((hari.0, eq)),
            }
            if let Some((_, awal)) = self.hari {
                if awal > 0.0 && eq < awal * (1.0 - self.cfg.rem_harian) {
                    return self.tolak("rem harian");
                }
            }
        }
        let frac = self.kelly();
        let eq = self.ekuitas_beli();
        let terpakai = eq - self.cash;
        if terpakai + (frac * eq).min(self.cash) > self.cfg.exp_max * eq {
            return self.tolak("eksposur penuh");
        }
        let g = self.cfg.gas_usd * 3.0 + self.cfg.approve_usd;
        let mut mult = 1.0;
        if self.cfg.rem {
            let dd = 1.0 - eq / self.peak;
            mult = if dd > 0.30 { 0.25 } else if dd > 0.20 { 0.5 } else if dd > 0.10 { 0.75 } else { 1.0 };
        }
        let mut pos = (frac * eq * mult).min(self.cash - g);
        if self.cfg.lp_cap > 0.0 && k.lp_m1 > 0.0 {
            pos = pos.min(self.cfg.lp_cap * k.lp_m1);
        }
        if pos < self.cfg.min_pos {
            return self.tolak("kas kurang");
        }
        self.pending.insert(id, pos);
        Some(true)
    }
    pub fn ukuran(&self, pool: &str) -> f64 {
        self.pending.get(pool).cloned().unwrap_or(0.0)
    }
    pub fn masuk(&mut self, pool: &str, size: f64, lp: f64, blk: f64) {
        self.pending.remove(pool);
        let dev = self.kep.get(pool).and_then(|k| k.dev.clone());
        self.cash -= size;
        self.open.insert(pool.to_string(), Posisi { pos: size, lp, dev, blk });
        self.n_beli += 1;
    }
    /// Trade v4 selesai. Hanya mengubah akun kalau akun memegang posisi (atau beli gagal dari keputusan akun).
    pub fn tutup(&mut self, t: &Trade) -> Option<(f64, f64)> {
        let (pos, dev) = if let Some(p) = self.open.remove(&t.pool) {
            (p.pos, p.dev)
        } else if t.reason == "beli gagal" && self.pending.contains_key(&t.pool) {
            self.pending.remove(&t.pool);
            (t.size, self.kep.get(&t.pool).and_then(|k| k.dev.clone()))
        } else {
            return None;
        };
        let g = if t.gas_tot.is_finite() { t.gas_tot } else { 0.2 };
        let back = if t.reason == "beli gagal" { pos - g } else { (pos * (t.ret + g / t.size) - g).max(0.0) };
        if t.reason == "beli gagal" {
            self.cash -= g;
            self.n_beli += 1;
        } else {
            self.cash += back;
        }
        self.mtm.remove(&t.pool);
        self.n_tutup += 1;
        if back > pos {
            self.n_menang += 1;
        }
        self.rets.push(t.ret);
        if let Some(d) = dev {
            self.dev_hist.entry(d).or_default().push(t.ret);
        }
        let eq = self.ekuitas_beli();
        self.peak = self.peak.max(eq);
        self.dd_max = self.dd_max.max(1.0 - eq / self.peak);
        if !self.killed && 1.0 - eq / self.peak > self.cfg.kill_dd {
            self.killed = true;
        }
        self.eq_curve.push((t.exit_blk, eq));
        Some((pos, back))
    }
}

/// Last swap state of a pool when an order was emitted (for the executor's local quote).
#[derive(Clone, Copy, Debug)]
pub struct SwapState {
    pub sqrt_p: f64,
    pub l: f64,
    pub fee: f64,
    pub blk: f64,
}

/// What the account just did, for the live executor. Built from the engine's own events; no decision logic here,
/// and nothing is produced unless `Mesin::emit_orders` is set (simulation never sets it).
#[derive(Clone, Debug)]
pub enum ModelOrder {
    Buy { pool: String, size_usd: f64, lp_cap_usd: f64, model_equity_usd: f64, state: Option<SwapState> },
    TakeProfit { pool: String, frac: f64, state: Option<SwapState> },
    Exit { pool: String, why: String, state: Option<SwapState> },
    Closed { pool: String, reason: String, ret: f64, state: Option<SwapState> },
}

/// Mesin bot: semua PoolSim + akun bersama. Tidak ada jaringan di sini.
pub struct Mesin {
    pub sims: BTreeMap<String, PoolSim>,
    pub akun: Arc<Mutex<Akun>>,
    model: Arc<Model>,
    model_up: Option<Arc<Model>>,
    pub selesai: Vec<(Trade, Option<(f64, f64)>)>,
    pub event_log: Vec<String>,
    /// simulasi bayangan (tanpa gerbang, ukuran tetap seperti replay backtest) -> label semua koin
    pub bayangan: bool,
    pub label_selesai: Vec<Trade>,
    hz_c: f64,
    /// live execution only: collect `orders` for the executor
    pub emit_orders: bool,
    pub orders: Vec<ModelOrder>,
}

impl Mesin {
    pub fn new(akun: Akun, model: Model, model_up: Option<Model>) -> Mesin {
        let hz_c = akun.cfg.hz;
        Mesin {
            bayangan: false,
            label_selesai: Vec::new(),
            hz_c,
            sims: BTreeMap::new(),
            akun: Arc::new(Mutex::new(akun)),
            model: Arc::new(model),
            model_up: model_up.map(Arc::new),
            selesai: Vec::new(),
            event_log: Vec::new(),
            emit_orders: false,
            orders: Vec::new(),
        }
    }
    /// Opsi mesin = AUDIT realistis uji maju (latb 30, entryLat 30, worstOrder, gas nyata, transaksi gagal, ...).
    fn opsi(&self, eth: f64) -> SimOpts {
        let c = self.akun.lock().unwrap().cfg.clone();
        let a1 = self.akun.clone();
        let a2 = self.akun.clone();
        SimOpts {
            eth_usd: eth,
            model: Some(self.model.clone()),
            model_up: self.model_up.clone(),
            th: Thresholds { danger_th: c.danger_th, up_th: c.up_th, tp_up: c.tp_up },
            latb: Some(c.lat_blocks),
            entry_lat: c.lat_blocks,
            keep_no_after: true,
            strict_snap: true,
            lp_adjust: true,
            fill_on_time: true,
            worst_order: true,
            gas_usd: Some(c.gas_usd),
            approve_usd: Some(c.approve_usd),
            fail_buy: Some(c.fail_buy),
            fail_sell: Some(c.fail_sell),
            hz: Some(c.hz),
            rug_exit: c.rug_exit,
            trail: c.trail,
            dump_frac: c.dump_frac,
            sell_impact: match c.sell_impact.as_str() {
                "1" => port_engine::engine::SellImpact::On,
                "full" => port_engine::engine::SellImpact::Full,
                _ => port_engine::engine::SellImpact::Off,
            },
            min_usd: Some(c.min_pos),
            max_usd: Some(f64::INFINITY),
            lp_frac: Some(f64::INFINITY),
            plans: Some(vec!["v4".to_string()]),
            record_events: true,
            gate: Some(Arc::new(move |s: &PoolSim| a1.lock().unwrap().gerbang(s))),
            size_fn: Some(Arc::new(move |_lp: f64, m: &PoolMeta| a2.lock().unwrap().ukuran(&m.id))),
            ..SimOpts::default()
        }
    }
    /// Opsi simulasi bayangan: sama dengan replay uji maju (tanpa gerbang akun, ukuran bawaan mesin).
    fn opsi_bayangan(&self, eth: f64) -> SimOpts {
        let c = self.akun.lock().unwrap().cfg.clone();
        SimOpts {
            eth_usd: eth,
            model: Some(self.model.clone()),
            model_up: self.model_up.clone(),
            th: Thresholds { danger_th: c.danger_th, up_th: c.up_th, tp_up: c.tp_up },
            latb: Some(30.0),
            entry_lat: 30.0,
            keep_no_after: true,
            strict_snap: true,
            lp_adjust: true,
            fill_on_time: true,
            worst_order: true,
            gas_usd: Some(c.gas_usd),
            approve_usd: Some(c.approve_usd),
            fail_buy: Some(c.fail_buy),
            fail_sell: Some(c.fail_sell),
            hz: Some(c.hz),
            rug_exit: c.rug_exit,
            trail: c.trail,
            dump_frac: c.dump_frac,
            plans: Some(vec!["v4".to_string()]),
            ..SimOpts::default()
        }
    }
    /// Pool baru lahir. false kalau tidak aman (bukan pasangan ETH, fee > 0,5%, hook beforeSwap).
    pub fn pool_baru(&mut self, m: PoolMeta, eth: f64) -> bool {
        if !eng::safe(&m).unwrap_or(false) || !eth.is_finite() {
            return false;
        }
        let o = self.opsi(eth);
        match PoolSim::new(m.clone(), o) {
            Ok(s) => {
                if self.bayangan {
                    if let Ok(b) = PoolSim::new(m.clone(), self.opsi_bayangan(eth)) {
                        self.sims.insert(format!("b:{}", m.id), b);
                    }
                }
                self.sims.insert(m.id, s);
                true
            }
            Err(_) => false,
        }
    }
    fn kuras(&mut self, id: &str) {
        if id.starts_with("b:") {
            if let Some(s) = self.sims.get_mut(id) {
                s.events.clear();
                let t: Vec<Trade> = s.trades.drain(..).filter(|t| t.plan == "v4").collect();
                self.label_selesai.extend(t);
            }
            return;
        }
        let (evs, trs, mtm) = match self.sims.get_mut(id) {
            Some(s) => {
                let e: Vec<Event> = s.events.drain(..).collect();
                let t: Vec<Trade> = s.trades.drain(..).collect();
                let m = s.nilai_sekarang("v4");
                (e, t, m)
            }
            None => return,
        };
        let st = if self.emit_orders {
            self.sims.get(id).and_then(|s| s.sw.last()).map(|x| SwapState { sqrt_p: x.sqrt_p, l: x.l, fee: x.fee, blk: x.blk })
        } else {
            None
        };
        let mut a = self.akun.lock().unwrap();
        for e in &evs {
            if let Event::Entry { pool, blk, size, lp, .. } = e {
                a.masuk(pool, *size, *lp, *blk);
                if self.emit_orders {
                    let lp_cap_usd = if a.cfg.lp_cap > 0.0 { a.cfg.lp_cap * lp } else { f64::INFINITY };
                    self.orders.push(ModelOrder::Buy { pool: pool.clone(), size_usd: *size, lp_cap_usd, model_equity_usd: a.ekuitas_beli(), state: st });
                }
            }
            if self.emit_orders {
                match e {
                    Event::Tp1 { plan, pool, .. } if plan == "v4" => {
                        let frac = eng::config(plan, &Thresholds::default()).map(|c| c.frac).unwrap_or(0.0);
                        if frac > 0.0 {
                            self.orders.push(ModelOrder::TakeProfit { pool: pool.clone(), frac, state: st });
                        }
                    }
                    Event::ExitSignal { plan, pool, why, .. } if plan == "v4" => self.orders.push(ModelOrder::Exit { pool: pool.clone(), why: why.clone(), state: st }),
                    _ => {}
                }
            }
            self.event_log.push(e.to_json());
        }
        if let Some(v) = mtm {
            if a.open.contains_key(id) && v.is_finite() {
                a.mtm.insert(id.to_string(), v);
            }
        }
        drop(a);
        for t in trs {
            if t.plan != "v4" {
                continue;
            }
            let r = self.akun.lock().unwrap().tutup(&t);
            if self.emit_orders && r.is_some() {
                self.orders.push(ModelOrder::Closed { pool: t.pool.clone(), reason: t.reason.clone(), ret: t.ret, state: st });
            }
            self.selesai.push((t, r));
        }
    }
    /// Satu blok: (1) pesanan yang jatuh tempo diisi (tick), (2) event likuiditas+swap per pool, (3) pool selesai dibuang.
    pub fn blok(&mut self, blk: f64, per_pool: &BTreeMap<String, (Vec<BigInt>, Vec<Swap>)>) {
        let ids: Vec<String> = self.sims.keys().cloned().collect();
        for id in &ids {
            let mut g = 0;
            loop {
                let d = match self.sims.get(id) {
                    Some(s) => s.next_deadline(),
                    None => break,
                };
                if !(d < blk) || g >= 50 {
                    break;
                }
                self.sims.get_mut(id).unwrap().tick(d);
                self.kuras(id);
                g += 1;
            }
        }
        for (id, (l, s)) in per_pool {
            if let Some(sim) = self.sims.get_mut(id) {
                sim.process_block(blk, l, s);
                self.kuras(id);
            }
            if self.bayangan {
                let bid = format!("b:{id}");
                if let Some(sim) = self.sims.get_mut(&bid) {
                    sim.process_block(blk, l, s);
                    self.kuras(&bid);
                }
            }
        }
        let hz = self.hz_c;
        let habis: Vec<String> = self.sims.iter().filter(|(_, s)| blk >= s.m.init + hz || s.all_done()).map(|(k, _)| k.clone()).collect();
        for id in habis {
            if let Some(s) = self.sims.get_mut(&id) {
                s.finalize();
            }
            self.kuras(&id);
            self.sims.remove(&id);
        }
    }
    fn hz(&self) -> f64 {
        self.hz_c
    }
    /// Akhir data (simulasi): tutup semua.
    pub fn akhiri(&mut self) {
        let ids: Vec<String> = self.sims.keys().cloned().collect();
        for id in ids {
            let lim = self.sims[&id].m.init + self.hz();
            let mut g = 0;
            loop {
                let d = self.sims[&id].next_deadline();
                if !(d <= lim) || self.sims[&id].all_done() || g >= 50 {
                    break;
                }
                self.sims.get_mut(&id).unwrap().tick(d);
                self.kuras(&id);
                g += 1;
            }
            self.sims.get_mut(&id).unwrap().finalize();
            self.kuras(&id);
            self.sims.remove(&id);
        }
    }
}
