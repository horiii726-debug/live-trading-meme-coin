//! Port 1:1 dari `rhc_vps/pipeline/v4engine.js` (kelas PoolSim, CONFIGS, safe, h32).
//! Diproses BLOK DEMI BLOK: (1) event likuiditas, (2) swap baru + potret fitur, (3) aturan keluar.
//!
//! Aturan port:
//! - Semua nomor blok disimpan sebagai f64 (sama dengan Number di JS; Infinity dipakai sebagai "tidak ada").
//! - Urutan operasi floating point ditulis persis sama dengan JS (tanpa penyusunan ulang, tanpa FMA).
//! - Math.max/Math.min/Math.log/Math.exp/toFixed memakai padanan JS di `jsnum`.
//! - Likuiditas memakai BigInt (num-bigint) seperti BigInt di JS.
//! - `onTrade` / `onEvent` / `onSnap` diganti antrean (`trades`, `events`, `snaps_out`) yang dikuras pemanggil;
//!   urutan kejadiannya sama persis dengan urutan pemanggilan callback di JS.

use crate::jsnum::{self, fx, h32, js_max, js_min, js_round, num, num_to_string};
use crate::model::Model;
use num_bigint::{BigInt, Sign};
use num_traits::Zero;
use std::collections::HashMap;
use std::sync::Arc;

pub const NATIVE: &str = "0x0000000000000000000000000000000000000000";
pub const HZ: f64 = 216000.0;
pub const MIN: f64 = 600.0;
pub const STEP: f64 = 100.0;
pub const GAS: f64 = 0.02;

/// `x || 0` untuk angka
#[inline]
fn or0(v: f64) -> f64 {
    if v != 0.0 && !v.is_nan() {
        v
    } else {
        0.0
    }
}
/// truthiness angka JS
#[inline]
fn tr(v: f64) -> bool {
    v != 0.0 && !v.is_nan()
}

#[inline]
fn big_to_f64(b: &BigInt) -> f64 {
    // Number(BigInt): dibulatkan ke double terdekat (seri ke genap)
    jsnum::bigint_to_f64(b)
}

/// BigInt(str) di JS: spasi dipangkas, "" -> 0n, awalan 0x/0o/0b, atau desimal bertanda. Selain itu SyntaxError.
pub fn parse_bigint(s: &str) -> Result<BigInt, String> {
    let t = jsnum::js_trim(s);
    if t.is_empty() {
        return Ok(BigInt::zero());
    }
    let b = t.as_bytes();
    let (radix, body, neg) = if b.len() >= 2 && b[0] == b'0' && matches!(b[1], b'x' | b'X' | b'o' | b'O' | b'b' | b'B') {
        let r = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            _ => 2,
        };
        (r, &t[2..], false)
    } else if b[0] == b'-' {
        (10, &t[1..], true)
    } else if b[0] == b'+' {
        (10, &t[1..], false)
    } else {
        (10, t, false)
    };
    if body.is_empty() || !body.chars().all(|c| c.is_digit(radix)) {
        return Err(format!("SyntaxError: Cannot convert {} to a BigInt", s));
    }
    let mag = BigInt::parse_bytes(body.as_bytes(), radix).ok_or_else(|| format!("BigInt tidak sah: {}", s))?;
    Ok(if neg { -mag } else { mag })
}

#[derive(Clone, Debug)]
pub struct PoolMeta {
    pub id: String,
    pub c0: String,
    pub c1: String,
    pub fee: f64,
    pub hooks: String,
    pub init: f64,
}

/// `safe(m)` di v4engine.js
/// Highest pool fee accepted (ppm). Env MAX_FEE, default 5000 (0.5%, original behaviour).
fn max_fee() -> f64 {
    static V: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("MAX_FEE").ok().and_then(|s| s.parse().ok()).unwrap_or(5000.0))
}

pub fn safe(m: &PoolMeta) -> Result<bool, String> {
    if !((m.c0 == NATIVE || m.c1 == NATIVE) && m.fee <= max_fee()) {
        return Ok(false);
    }
    let h = parse_bigint(&m.hooks)?;
    let bit: BigInt = (h >> 7usize) & BigInt::from(1);
    Ok(bit.is_zero())
}

#[derive(Clone, Debug)]
pub struct Swap {
    pub blk: f64,
    pub li: f64,
    pub tx: String,
    pub a0: f64,
    pub a1: f64,
    pub sqrt_p: f64,
    pub l: f64,
    pub fee: f64,
}

/// Bagian swap yang dipakai perhitungan nilai jual (hasil `adj` / `stateNow` di JS).
#[derive(Clone, Copy, Debug)]
struct St {
    sqrt_p: f64,
    l: f64,
    fee: f64,
}
impl St {
    #[inline]
    fn of(x: &Swap) -> St {
        St { sqrt_p: x.sqrt_p, l: x.l, fee: x.fee }
    }
}

/// Konstanta modul JS yang dibaca dari env saat dimuat: DANGER_TH, UP_TH, TP_UP.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub danger_th: f64,
    pub up_th: f64,
    pub tp_up: f64,
}
impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { danger_th: 0.4, up_th: 1.0, tp_up: 1.0 }
    }
}
impl Thresholds {
    /// `+(process.env.X || default)`
    pub fn from_env() -> Thresholds {
        let rd = |k: &str, d: f64| match std::env::var(k) {
            Ok(s) if !s.is_empty() => jsnum::to_number(&s),
            _ => d,
        };
        Thresholds { danger_th: rd("DANGER_TH", 0.4), up_th: rd("UP_TH", 1.0), tp_up: rd("TP_UP", 1.0) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlanCfg {
    pub tp: f64,
    pub frac: f64,
    pub pre_hard40: bool, // pre: "hard40" (lainnya "flow")
    pub run_danger: bool, // run: "danger" (lainnya "v2")
    pub th: f64,
}

/// CONFIGS di v4engine.js (urutan kunci: v4, v3, v2)
pub const PLAN_NAMES: [&str; 3] = ["v4", "v3", "v2"];
pub fn config(name: &str, th: &Thresholds) -> Option<PlanCfg> {
    match name {
        "v4" => Some(PlanCfg { tp: 1.2, frac: 0.6, pre_hard40: true, run_danger: true, th: th.danger_th }),
        "v3" => Some(PlanCfg { tp: 1.2, frac: 0.85, pre_hard40: true, run_danger: false, th: f64::NAN }),
        "v2" => Some(PlanCfg { tp: f64::INFINITY, frac: 0.0, pre_hard40: false, run_danger: false, th: f64::NAN }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SellImpact {
    Off,
    /// nilai truthy apa pun selain "full"
    On,
    Full,
}

/// Nonce antrean dompet (opt.nonce[pool] = {e,t,x}); nilai falsy -> 0.
#[derive(Clone, Copy, Debug, Default)]
pub struct Nonce {
    pub e: f64,
    pub t: f64,
    pub x: f64,
}

/// Hasil gate: None = `null` (tunggu), Some(false) = `false` (tolak), Some(true) = lainnya (lanjut)
pub type GateFn = Arc<dyn Fn(&PoolSim) -> Option<bool> + Send + Sync>;
pub type SizeFn = Arc<dyn Fn(f64, &PoolMeta) -> f64 + Send + Sync>;

/// Opsi PoolSim. Field `Option` meniru `opt.x ?? default`; field f64 bertanda "||" meniru `+(opt.x || default)`.
#[derive(Clone)]
pub struct SimOpts {
    pub eth_usd: f64,
    pub model: Option<Arc<Model>>,
    pub model_up: Option<Arc<Model>>,
    pub th: Thresholds,
    pub latb: Option<f64>,
    pub gas_usd: Option<f64>,
    pub approve_usd: Option<f64>,
    pub fail_buy: Option<f64>,
    pub fail_sell: Option<f64>,
    pub hz: Option<f64>,
    pub max_usd: Option<f64>,
    pub lp_frac: Option<f64>,
    pub min_usd: Option<f64>,
    pub size_fn: Option<SizeFn>,
    pub rug_exit: bool,
    pub warn_tick: bool,
    pub trail: f64,      // ||
    pub trail_pre: f64,  // ||
    pub dump_frac: f64,  // || 0.05
    pub tp_ladder: Option<Vec<(f64, f64)>>,
    pub arm_at: f64,     // ||
    pub arm_trail: f64,  // || 0.35
    pub arm_dump: f64,   // ||
    pub stale_min: f64,  // falsy = mati
    pub quiet_min: f64,  // falsy = mati
    pub quiet_all: bool,
    /// keluar saat momentum orderflow melemah (jendela menit; falsy = mati) — referensi bagian 14
    pub mom_min: f64,
    /// tahan dari sinyal model bahaya selama momentum masih kuat
    pub mom_hold: bool,
    pub nonce: Option<Arc<HashMap<String, Nonce>>>,
    pub plans: Option<Vec<String>>,
    /// entryLat (falsy = cara masuk lama: masuk di swap pertama >= init+600)
    pub entry_lat: f64,
    pub strict_snap: bool,
    pub keep_no_after: bool,
    pub lp_adjust: bool,
    pub fill_on_time: bool,
    pub worst_order: bool,
    pub sell_last: bool,
    pub sell_impact: SellImpact,
    pub gate: Option<GateFn>,
    pub record_events: bool,
    pub record_snaps: bool,
}

impl Default for SimOpts {
    fn default() -> Self {
        SimOpts {
            eth_usd: f64::NAN,
            model: None,
            model_up: None,
            th: Thresholds::default(),
            latb: None,
            gas_usd: None,
            approve_usd: None,
            fail_buy: None,
            fail_sell: None,
            hz: None,
            max_usd: None,
            lp_frac: None,
            min_usd: None,
            size_fn: None,
            rug_exit: false,
            warn_tick: false,
            trail: 0.0,
            trail_pre: 0.0,
            dump_frac: 0.0,
            tp_ladder: None,
            arm_at: 0.0,
            arm_trail: 0.0,
            arm_dump: 0.0,
            stale_min: 0.0,
            quiet_min: 0.0,
            quiet_all: false,
            mom_min: 0.0,
            mom_hold: false,
            nonce: None,
            plans: None,
            entry_lat: 0.0,
            strict_snap: false,
            keep_no_after: false,
            lp_adjust: false,
            fill_on_time: false,
            worst_order: false,
            sell_last: false,
            sell_impact: SellImpact::Off,
            gate: None,
            record_events: false,
            record_snaps: false,
        }
    }
}

/// Satu baris trade (objek yang dikirim ke onTrade). `fails` = Some hanya untuk trade "beli gagal",
/// yang di JS punya urutan field berbeda dan tanpa `txs`.
#[derive(Clone, Debug)]
pub struct Trade {
    pub plan: String,
    pub pool: String,
    pub init: f64,
    pub entry_blk: f64,
    pub exit_blk: f64,
    pub size: f64,
    pub pnl: f64,
    pub ret: f64,
    pub gas_tot: f64,
    pub txs: f64,
    pub hold: f64,
    pub reason: String,
    pub tp1: bool,
    pub tp1_blk: Option<f64>,
    pub tp1_v: Option<f64>,
    pub peak_v: f64,
    pub perfect: f64,
    pub eth_usd: f64,
    pub fails: Option<f64>,
}

impl Trade {
    /// Sama byte-demi-byte dengan JSON.stringify(t) di replay.js
    pub fn to_json(&self) -> String {
        let mut o = String::with_capacity(400);
        let n = |o: &mut String, k: &str, v: f64| {
            o.push_str(",\"");
            o.push_str(k);
            o.push_str("\":");
            jsnum::json_num(v, o);
        };
        let on = |o: &mut String, k: &str, v: Option<f64>| {
            o.push_str(",\"");
            o.push_str(k);
            o.push_str("\":");
            match v {
                Some(x) => jsnum::json_num(x, o),
                None => o.push_str("null"),
            }
        };
        o.push_str("{\"plan\":");
        jsnum::json_str(&self.plan, &mut o);
        o.push_str(",\"pool\":");
        jsnum::json_str(&self.pool, &mut o);
        n(&mut o, "init", self.init);
        n(&mut o, "entryBlk", self.entry_blk);
        n(&mut o, "exitBlk", self.exit_blk);
        n(&mut o, "size", self.size);
        n(&mut o, "pnl", self.pnl);
        n(&mut o, "ret", self.ret);
        if self.fails.is_none() {
            n(&mut o, "gasTot", self.gas_tot);
            n(&mut o, "txs", self.txs);
        }
        n(&mut o, "hold", self.hold);
        o.push_str(",\"reason\":");
        jsnum::json_str(&self.reason, &mut o);
        o.push_str(if self.tp1 { ",\"tp1\":true" } else { ",\"tp1\":false" });
        on(&mut o, "tp1Blk", self.tp1_blk);
        on(&mut o, "tp1V", self.tp1_v);
        n(&mut o, "peakV", self.peak_v);
        n(&mut o, "perfect", self.perfect);
        n(&mut o, "ethUsd", self.eth_usd);
        if let Some(f) = self.fails {
            n(&mut o, "gasTot", self.gas_tot);
            n(&mut o, "fails", f);
        }
        o.push('}');
        o
    }
}

/// Padanan objek onEvent di JS (hanya dicatat kalau `record_events`).
#[derive(Clone, Debug)]
pub enum Event {
    Entry { pool: String, blk: f64, size: f64, lp: f64, tx: Option<String> },
    ExitSignal { plan: String, pool: String, blk: f64, v: Option<f64>, why: String, tgt: f64, tx: String },
    Tp1 { plan: String, pool: String, blk: f64, v: f64, tgt: f64, tx: String },
}

fn jkey(o: &mut String, k: &str) {
    if o.len() > 1 {
        o.push(',');
    }
    o.push('"');
    o.push_str(k);
    o.push_str("\":");
}
fn jnum(o: &mut String, k: &str, v: f64) {
    jkey(o, k);
    jsnum::json_num(v, o);
}
fn jstr(o: &mut String, k: &str, v: &str) {
    jkey(o, k);
    jsnum::json_str(v, o);
}

impl Event {
    /// Sama byte-demi-byte dengan JSON.stringify(objek onEvent) di v4engine.js (urutan field per jenis)
    pub fn to_json(&self) -> String {
        let mut o = String::from("{");
        match self {
            Event::Entry { pool, blk, size, lp, tx } => {
                jstr(&mut o, "type", "entry");
                jstr(&mut o, "pool", pool);
                jnum(&mut o, "blk", *blk);
                jnum(&mut o, "size", *size);
                jnum(&mut o, "lp", *lp);
                if let Some(t) = tx {
                    jstr(&mut o, "tx", t);
                }
            }
            Event::ExitSignal { plan, pool, blk, v, why, tgt, tx } => {
                jstr(&mut o, "type", "exit_signal");
                jstr(&mut o, "plan", plan);
                jstr(&mut o, "pool", pool);
                jnum(&mut o, "blk", *blk);
                if let Some(v) = v {
                    jnum(&mut o, "v", *v);
                }
                jstr(&mut o, "why", why);
                jnum(&mut o, "tgt", *tgt);
                jstr(&mut o, "tx", tx);
            }
            Event::Tp1 { plan, pool, blk, v, tgt, tx } => {
                jstr(&mut o, "type", "tp1");
                jstr(&mut o, "plan", plan);
                jstr(&mut o, "pool", pool);
                jnum(&mut o, "blk", *blk);
                jnum(&mut o, "v", *v);
                jnum(&mut o, "tgt", *tgt);
                jstr(&mut o, "tx", tx);
            }
        }
        o.push('}');
        o
    }
}

impl SnapOut {
    /// Sama dengan JSON.stringify(objek onSnap): {pool, blk, f:[...], pd, pu, v}
    pub fn to_json(&self) -> String {
        let mut o = String::from("{");
        jstr(&mut o, "pool", &self.pool);
        jnum(&mut o, "blk", self.blk);
        jkey(&mut o, "f");
        o.push('[');
        for (k, x) in self.f.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            jsnum::json_num(*x, &mut o);
        }
        o.push(']');
        jnum(&mut o, "pd", self.pd);
        jkey(&mut o, "pu");
        match self.pu {
            Some(u) => jsnum::json_num(u, &mut o),
            None => o.push_str("null"),
        }
        jnum(&mut o, "v", self.v);
        o.push('}');
        o
    }
}

/// Padanan objek onSnap di JS (hanya dicatat kalau `record_snaps`).
#[derive(Clone, Debug)]
pub struct SnapOut {
    pub pool: String,
    pub blk: f64,
    pub f: Vec<f64>,
    pub pd: f64,
    pub pu: Option<f64>,
    pub v: f64,
}

#[derive(Clone, Copy, Debug)]
struct SnapRec {
    i: usize,
    pd: f64,
    pu: Option<f64>,
}

#[derive(Clone, Debug)]
struct Pend {
    trig: i64,
    tgt: f64,
    tok: f64,
    filled: bool,
}

#[derive(Clone, Copy, Debug)]
struct Trig {
    trig: i64,
    tgt: f64,
    timed: bool,
}

#[derive(Clone, Debug)]
struct Plan {
    name: String,
    c: PlanCfg,
    rem: f64,
    proceeds: f64,
    txs: i64,
    tp_done: bool,
    trig: Option<Trig>,
    pend: Vec<Pend>,
    done: bool,
    reason: Option<String>,
    tp1_blk: Option<f64>,
    tp1_v: Option<f64>,
    lad: Vec<bool>,
}

pub struct PoolSim {
    pub m: PoolMeta,
    o: SimOpts,
    pub eth_usd: f64,
    latb: f64,
    gas: f64,
    approve_usd: f64,
    fail_buy: f64,
    fail_sell: f64,
    pub n_fail: i64,
    hz: f64,
    max_usd: f64,
    lp_frac: f64,
    min_usd: f64,
    trail: f64,
    trail_pre: f64,
    dump_frac: f64,
    arm_at: f64,
    arm_trail: f64,
    arm_dump: f64,
    stale_b: Option<f64>,
    quiet_b: Option<f64>,
    mom_b: Option<f64>,
    m_p1: usize,
    m_p2: usize,
    m_nb1: i64,
    m_nb2: i64,
    m_bu1: f64,
    m_su1: f64,
    q_is_c1: bool,
    hook: f64,
    pub sw: Vec<Swap>,
    pub v: Vec<f64>,
    pub lp: Vec<f64>,
    a: BigInt,
    r: BigInt,
    warn_blk: Option<f64>,
    rug_blk: Option<f64>,
    pub rem_frac: f64,
    pub ei: i64,
    pub skip: bool,
    pub skip_why: Option<String>,
    pub closed: bool,
    lo: usize,
    b: i64,
    s: i64,
    s_usd_w: f64,
    pub peak: f64,
    s_cnt: i64,
    s_usd: f64,
    peak_s: f64,
    s_cnt_s: i64,
    s_sum_s: f64,
    last_snap: f64,
    snap_stop: bool,
    pd: Option<f64>,
    pu: Option<f64>,
    pub n_snap: u64,
    snaps: Vec<SnapRec>,
    snap_ptr: i64,
    pub perfect: f64,
    off: f64,
    nd: Option<Nonce>,
    plans: Vec<Plan>,
    decide_blk: Option<f64>,
    e_blk: f64,
    pub size: f64,
    pub base: f64,
    a_at_last: Option<BigInt>,
    r_at_last: Option<BigInt>,
    pub trades: Vec<Trade>,
    pub events: Vec<Event>,
    pub snaps_out: Vec<SnapOut>,
    // interning hash tx (pengganti `new Set()` string di features(): hitung tx unik tanpa hashing string tiap potret)
    tx_id: Vec<u32>,
    tx_map: HashMap<String, u32>,
    seen: Vec<u32>,
    stamp: u32,
}

impl PoolSim {
    pub fn new(meta: PoolMeta, opt: SimOpts) -> Result<PoolSim, String> {
        let names: Vec<String> = match &opt.plans {
            Some(p) => p.clone(),
            None => PLAN_NAMES.iter().map(|s| s.to_string()).collect(),
        };
        let mut plans = Vec::with_capacity(names.len());
        let lad_n = opt.tp_ladder.as_ref().map(|l| l.len()).unwrap_or(0);
        for name in names {
            let c = config(&name, &opt.th).ok_or_else(|| format!("plan tidak dikenal: {}", name))?;
            plans.push(Plan {
                name,
                c,
                rem: 0.0,
                proceeds: 0.0,
                txs: 1,
                tp_done: false,
                trig: None,
                pend: Vec::new(),
                done: false,
                reason: None,
                tp1_blk: None,
                tp1_v: None,
                lad: vec![false; lad_n],
            });
        }
        let nd = opt.nonce.as_ref().and_then(|n| n.get(&meta.id)).map(|n| Nonce { e: or0(n.e), t: or0(n.t), x: or0(n.x) });
        let pick = |v: f64, d: f64| if tr(v) { v } else { d };
        Ok(PoolSim {
            q_is_c1: meta.c1 == NATIVE,
            hook: if meta.hooks != NATIVE { 1.0 } else { 0.0 },
            eth_usd: opt.eth_usd,
            latb: opt.latb.unwrap_or(30.0),
            gas: opt.gas_usd.unwrap_or(GAS),
            approve_usd: opt.approve_usd.unwrap_or(0.0),
            fail_buy: opt.fail_buy.unwrap_or(0.0),
            fail_sell: opt.fail_sell.unwrap_or(0.0),
            n_fail: 0,
            hz: opt.hz.unwrap_or(HZ),
            max_usd: opt.max_usd.unwrap_or(200.0),
            lp_frac: opt.lp_frac.unwrap_or(0.01),
            min_usd: opt.min_usd.unwrap_or(20.0),
            trail: pick(opt.trail, 0.0),
            trail_pre: pick(opt.trail_pre, 0.0),
            dump_frac: pick(opt.dump_frac, 0.05),
            arm_at: pick(opt.arm_at, 0.0),
            arm_trail: pick(opt.arm_trail, 0.35),
            arm_dump: pick(opt.arm_dump, 0.0),
            stale_b: if tr(opt.stale_min) { Some(opt.stale_min * 600.0) } else { None },
            quiet_b: if tr(opt.quiet_min) { Some(opt.quiet_min * 600.0) } else { None },
            mom_b: if tr(opt.mom_min) { Some(opt.mom_min * 600.0) } else { None },
            m_p1: 0,
            m_p2: 0,
            m_nb1: 0,
            m_nb2: 0,
            m_bu1: 0.0,
            m_su1: 0.0,
            sw: Vec::new(),
            v: Vec::new(),
            lp: Vec::new(),
            a: BigInt::zero(),
            r: BigInt::zero(),
            warn_blk: None,
            rug_blk: None,
            rem_frac: 0.0,
            ei: -1,
            skip: false,
            skip_why: None,
            closed: false,
            lo: 0,
            b: 0,
            s: 0,
            s_usd_w: 0.0,
            peak: 1.0,
            s_cnt: 0,
            s_usd: 0.0,
            peak_s: 1.0,
            s_cnt_s: 0,
            s_sum_s: 0.0,
            last_snap: -1e9,
            snap_stop: false,
            pd: None,
            pu: None,
            n_snap: 0,
            snaps: Vec::new(),
            snap_ptr: -1,
            perfect: 0.0,
            off: 0.0,
            nd,
            plans,
            decide_blk: None,
            e_blk: f64::NAN,
            size: f64::NAN,
            base: f64::NAN,
            a_at_last: None,
            r_at_last: None,
            trades: Vec::new(),
            events: Vec::new(),
            snaps_out: Vec::new(),
            tx_id: Vec::new(),
            tx_map: HashMap::new(),
            seen: Vec::new(),
            stamp: 0,
            m: meta,
            o: opt,
        })
    }

    #[inline]
    fn to_usd(&self, r: f64) -> f64 {
        (r.abs() / 1e18) * self.eth_usd
    }
    #[inline]
    fn lp_usd(&self, x: St) -> f64 {
        2.0 * self.to_usd(if self.q_is_c1 { x.l * x.sqrt_p } else { x.l / x.sqrt_p })
    }
    #[inline]
    fn is_buy(&self, x: &Swap) -> bool {
        (if self.q_is_c1 { x.a0 } else { x.a1 }) > 0.0
    }
    #[inline]
    fn q_usd(&self, x: &Swap) -> f64 {
        ((if self.q_is_c1 { x.a1 } else { x.a0 }).abs() / 1e18) * self.eth_usd
    }
    fn sell_val(&self, x: St, b_tok: f64) -> f64 {
        if b_tok <= 0.0 {
            return 0.0;
        }
        let b_in = b_tok * (1.0 - x.fee / 1e6);
        let r = if self.q_is_c1 {
            let inv = 1.0 / x.sqrt_p + b_in / x.l;
            x.l * (x.sqrt_p - 1.0 / inv)
        } else {
            let spb = x.sqrt_p + b_in / x.l;
            x.l * (1.0 / x.sqrt_p - 1.0 / spb)
        };
        if !r.is_finite() || r <= 0.0 {
            0.0
        } else {
            self.to_usd(r)
        }
    }
    fn adj(&self, x: St) -> St {
        if !tr(self.off) {
            return x;
        }
        if self.q_is_c1 {
            let mut inv = 1.0 / x.sqrt_p + self.off / x.l;
            if !(inv > 0.0) {
                inv = 1e-3 / x.sqrt_p;
            }
            return St { sqrt_p: 1.0 / inv, ..x };
        }
        let mut sp = x.sqrt_p + self.off / x.l;
        if !(sp > 0.0) {
            sp = 1e-3 * x.sqrt_p;
        }
        St { sqrt_p: sp, ..x }
    }
    fn realize(&mut self, idx: usize, tok: f64) -> f64 {
        if let Some(rb) = self.rug_blk {
            if rb <= self.sw[idx].blk {
                return 0.0;
            }
        }
        let v = self.sell_val(self.adj(St::of(&self.sw[idx])), tok);
        if self.o.sell_impact != SellImpact::Off {
            self.off += tok;
        }
        v
    }

    #[inline]
    fn entry_lat_on(&self) -> bool {
        tr(self.o.entry_lat)
    }
    fn nd_e(&self) -> f64 {
        self.nd.map(|n| n.e).unwrap_or(0.0)
    }
    fn nd_t(&self) -> f64 {
        self.nd.map(|n| n.t).unwrap_or(0.0)
    }
    fn nd_x(&self) -> f64 {
        self.nd.map(|n| n.x).unwrap_or(0.0)
    }
    fn ev(&mut self, e: Event) {
        if self.o.record_events {
            self.events.push(e);
        }
    }

    /// liqs: delta BigInt urut log index; swaps: urut log index
    pub fn process_block(&mut self, blk: f64, liqs: &[BigInt], swaps: &[Swap]) {
        if self.closed || self.skip {
            return;
        }
        if blk > self.m.init + self.hz {
            return;
        }
        if self.entry_tgt() < blk || (!self.o.worst_order && self.entry_tgt() <= blk) {
            let t = self.entry_tgt();
            self.enter_at(t);
            if self.skip {
                return;
            }
        }
        // (1) likuiditas
        for d in liqs {
            if d.sign() == Sign::Plus {
                self.a += d;
            } else {
                self.r -= d; // r += -d
            }
            if self.a.sign() == Sign::Plus {
                if self.warn_blk.is_none() && (&self.r * 100u32) / &self.a > BigInt::from(30) {
                    self.warn_blk = Some(blk);
                }
                if self.rug_blk.is_none() && (&self.r * 100u32) / &self.a > BigInt::from(80) {
                    self.rug_blk = Some(blk);
                }
            }
        }
        self.rem_frac = if self.a.sign() == Sign::Plus { big_to_f64(&((&self.r * 1000u32) / &self.a)) / 1000.0 } else { 0.0 };
        if self.o.rug_exit || self.o.warn_tick {
            self.cek_darurat(blk);
        }
        // (2) swap baru
        let first = self.sw.len();
        for x in swaps {
            if !x.sqrt_p.is_finite() || x.sqrt_p <= 0.0 || !x.l.is_finite() || x.l <= 0.0 {
                continue;
            }
            let i = self.sw.len();
            let lpv = self.lp_usd(St::of(x));
            let next_id = self.tx_map.len() as u32;
            let id = *self.tx_map.entry(x.tx.clone()).or_insert(next_id);
            if id == next_id {
                self.seen.push(0);
            }
            self.tx_id.push(id);
            self.sw.push(x.clone());
            self.lp.push(lpv);
            self.v.push(0.0);
            if self.ei < 0 && self.entry_lat_on() && self.decide_blk.is_none() && x.blk >= self.m.init + MIN {
                self.decide_blk = Some(if i > 0 { self.m.init + MIN } else { x.blk });
            }
            if self.ei < 0 && !self.entry_lat_on() && x.blk >= self.m.init + MIN {
                self.ei = i as i64;
                self.e_blk = x.blk;
                self.size = self.hitung_ukuran(self.lp[i]);
                if !self.size.is_finite() || self.size < self.min_usd {
                    self.skip = true;
                    self.skip_why = Some(format!("ukuran<${}", num_to_string(self.min_usd)));
                    return;
                }
                let d_in = (self.size / self.eth_usd) * 1e18 * (1.0 - x.fee / 1e6);
                let base = if self.q_is_c1 {
                    let sp1 = x.sqrt_p + d_in / x.l;
                    x.l * (1.0 / x.sqrt_p - 1.0 / sp1)
                } else {
                    let inv = 1.0 / x.sqrt_p + d_in / x.l;
                    x.l * (x.sqrt_p - 1.0 / inv)
                };
                if !base.is_finite() || base <= 0.0 {
                    self.skip = true;
                    self.skip_why = Some("base".to_string());
                    return;
                }
                self.base = base;
                for p in self.plans.iter_mut() {
                    p.rem = base;
                }
                self.v[i] = self.sell_val(St::of(x), base) / self.size;
                let e = Event::Entry { pool: self.m.id.clone(), blk: x.blk, size: self.size, lp: self.lp[i], tx: None };
                self.ev(e);
            } else if self.ei >= 0 {
                self.v[i] = self.sell_val(self.adj(St::of(x)), self.base) / self.size;
                if self.v[i] > self.perfect {
                    self.perfect = self.v[i];
                }
            }
        }
        // potret fitur untuk swap baru (maks satu potret per STEP blok)
        if self.ei >= 0 && self.o.model.is_some() && !self.snap_stop {
            let start = first.max((self.ei + 1) as usize);
            let mut i = start;
            while i < self.sw.len() {
                let vi = self.v[i];
                if vi > self.peak_s {
                    self.peak_s = vi;
                }
                if !self.is_buy(&self.sw[i]) {
                    self.s_cnt_s += 1;
                    self.s_sum_s += self.q_usd(&self.sw[i]);
                }
                let xb = self.sw[i].blk;
                if xb - self.last_snap < STEP {
                    i += 1;
                    continue;
                }
                if vi <= 0.0 {
                    self.snap_stop = true;
                    break;
                }
                self.last_snap = xb;
                let f = self.features(i);
                let pdv = self.o.model.as_ref().unwrap().predict(&f);
                self.n_snap += 1;
                let puv = self.o.model_up.as_ref().map(|mu| mu.predict(&f));
                self.snaps.push(SnapRec { i, pd: pdv, pu: puv });
                if self.o.record_snaps {
                    self.snaps_out.push(SnapOut { pool: self.m.id.clone(), blk: xb, f, pd: pdv, pu: puv, v: vi });
                }
                if !self.o.strict_snap {
                    self.pd = Some(pdv);
                    self.pu = puv;
                }
                i += 1;
            }
        }
        if self.sw.len() > first {
            self.a_at_last = Some(self.a.clone());
            self.r_at_last = Some(self.r.clone());
        }
        // urutan terburuk: bot lain di blok yang sama dianggap lebih dulu
        if self.o.worst_order && self.entry_tgt() <= blk {
            let t = self.entry_tgt();
            self.enter_at(t);
            if self.skip {
                return;
            }
        }
        // (3) aturan keluar
        for i in first..self.sw.len() {
            self.exit_step(i);
        }
    }

    /// 27 fitur kondisi untuk model (urutan = M.feats)
    pub fn features(&mut self, i: usize) -> Vec<f64> {
        // penanda "sudah dilihat" per id tx (padanan Set tx60)
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.iter_mut().for_each(|z| *z = 0);
            self.stamp = 1;
        }
        let stamp = self.stamp;
        let mut seen = std::mem::take(&mut self.seen);
        let f = self.features_inner(i, &mut seen, stamp);
        self.seen = seen;
        f
    }

    fn features_inner(&self, i: usize, seen: &mut [u32], stamp: u32) -> Vec<f64> {
        let sw = &self.sw;
        let x = &sw[i];
        let v = &self.v;
        let lp = &self.lp;
        let ei = self.ei;
        let (mut b60, mut s60, mut b300, mut s300) = (0i64, 0i64, 0i64, 0i64);
        let (mut bu60, mut su60, mut maxs60, mut bu300, mut su300) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let mut i60 = i;
        let mut i300 = i;
        let mut tx60: usize = 0;
        let mut k = i as i64;
        while k >= 0 && sw[k as usize].blk > x.blk - 3000.0 {
            let ku = k as usize;
            let y = &sw[ku];
            let u = self.q_usd(y);
            let buy = self.is_buy(y);
            if buy {
                b300 += 1;
                bu300 += u;
            } else {
                s300 += 1;
                su300 += u;
            }
            i300 = ku;
            if y.blk > x.blk - 600.0 {
                i60 = ku;
                let id = self.tx_id[ku] as usize;
                if seen[id] != stamp {
                    seen[id] = stamp;
                    tx60 += 1;
                }
                if buy {
                    b60 += 1;
                    bu60 += u;
                } else {
                    s60 += 1;
                    su60 += u;
                    let q = u / js_max(lp[ku], 1.0);
                    if q > maxs60 {
                        maxs60 = q;
                    }
                }
            }
            k -= 1;
        }
        let rf = |k: usize| -> usize { ei.max(k as i64 - 1) as usize };
        let avgs60 = if s60 != 0 { su60 / s60 as f64 } else { 0.0 };
        let avgb60 = if b60 != 0 { bu60 / b60 as f64 } else { 0.0 };
        let surge = if s60 != 0 && self.s_cnt_s != 0 { avgs60 / (self.s_sum_s / self.s_cnt_s as f64) } else { 0.0 };
        let acc = (b60 + s60) as f64 / js_max((b300 + s300) as f64 / 5.0, 1e-9);
        let eiu = ei as usize;
        vec![
            fx(jsnum::log(v[i]), 4),
            fx(jsnum::log(self.peak_s), 4),
            fx(1.0 - v[i] / self.peak_s, 4),
            fx((x.blk - self.e_blk) * 0.1 / 60.0, 2),
            b60 as f64,
            s60 as f64,
            fx(bu60, 0),
            fx(su60, 0),
            fx(avgs60, 1),
            fx(avgb60, 1),
            fx((bu60 - su60) / js_max(lp[i], 1.0), 4),
            b300 as f64,
            s300 as f64,
            fx(bu300, 0),
            fx(su300, 0),
            fx(surge, 3),
            fx(maxs60, 4),
            fx(v[i] / js_max(v[rf(i60)], 1e-9), 4),
            fx(v[i] / js_max(v[rf(i300)], 1e-9), 4),
            fx(lp[i] / js_max(lp[eiu], 1.0), 3),
            fx(lp[i] / js_max(lp[i300], 1.0), 3),
            if acc > 50.0 { 50.0 } else { fx(acc, 3) },
            tx60 as f64,
            if i > 0 { x.blk - sw[i - 1].blk } else { 0.0 },
            num(self.rem_frac),
            self.hook,
            self.m.fee,
        ]
    }

    /// keluar darurat berbasis keadaan pool (rug / LP ditarik), berlaku walau tidak ada swap baru
    fn cek_darurat(&mut self, blk: f64) {
        if self.ei < 0 || self.skip || self.closed {
            return;
        }
        let why = if self.o.rug_exit && self.rug_blk.map_or(false, |rb| blk >= rb) {
            "rug terdeteksi"
        } else if self.o.warn_tick && self.warn_blk.map_or(false, |wb| blk >= wb) {
            "LP ditarik >30%"
        } else {
            return;
        };
        let last = self.sw.len() - 1;
        for pi in 0..self.plans.len() {
            if self.plans[pi].done || self.plans[pi].trig.is_some() {
                continue;
            }
            let k = self.retries(&format!("{}|exit", self.plans[pi].name));
            self.n_fail += k;
            let tgt = blk + self.latb * (1 + k) as f64 + self.nd_x();
            let p = &mut self.plans[pi];
            p.txs += k;
            p.trig = Some(Trig { trig: last as i64, tgt, timed: true });
            p.reason = Some(why.to_string());
            let e = Event::ExitSignal {
                plan: p.name.clone(),
                pool: self.m.id.clone(),
                blk,
                v: None,
                why: why.to_string(),
                tgt,
                tx: self.sw[last].tx.clone(),
            };
            self.ev(e);
        }
    }

    fn exit_step(&mut self, i: usize) {
        let xblk = self.sw[i].blk;
        let xbuy = self.is_buy(&self.sw[i]);
        let xq = self.q_usd(&self.sw[i]);
        if xbuy {
            self.b += 1;
        } else {
            self.s += 1;
            self.s_usd_w += xq;
        }
        while self.sw[self.lo].blk <= xblk - 600.0 {
            let y = &self.sw[self.lo];
            if self.is_buy(y) {
                self.b -= 1;
            } else {
                self.s -= 1;
                self.s_usd_w -= self.q_usd(y);
            }
            self.lo += 1;
        }
        if let Some(w) = self.mom_b {
            // jendela kini (xblk-w, xblk] dan sebelumnya (xblk-2w, xblk-w]
            if xbuy {
                self.m_nb1 += 1;
                self.m_bu1 += xq;
            } else {
                self.m_su1 += xq;
            }
            while self.m_p1 <= i && self.sw[self.m_p1].blk <= xblk - w {
                let (yb, yq) = (self.is_buy(&self.sw[self.m_p1]), self.q_usd(&self.sw[self.m_p1]));
                if yb {
                    self.m_nb1 -= 1;
                    self.m_bu1 -= yq;
                    self.m_nb2 += 1;
                } else {
                    self.m_su1 -= yq;
                }
                self.m_p1 += 1;
            }
            while self.m_p2 < self.m_p1 && self.sw[self.m_p2].blk <= xblk - 2.0 * w {
                if self.is_buy(&self.sw[self.m_p2]) {
                    self.m_nb2 -= 1;
                }
                self.m_p2 += 1;
            }
        }
        if self.ei < 0 || (i as i64) <= self.ei {
            return;
        }
        if tr(self.off) {
            self.v[i] = self.sell_val(self.adj(St::of(&self.sw[i])), self.base) / self.size;
        }
        let v = self.v[i];
        if v > self.peak {
            self.peak = v;
        }
        let mut j = i;
        if self.o.sell_last {
            while j + 1 < self.sw.len() && self.sw[j + 1].blk == xblk {
                j += 1;
            }
        }
        if !xbuy {
            self.s_cnt += 1;
            self.s_usd += xq;
        }
        if self.o.strict_snap {
            while ((self.snap_ptr + 1) as usize) < self.snaps.len() && self.snaps[(self.snap_ptr + 1) as usize].i <= i {
                self.snap_ptr += 1;
            }
            if self.snap_ptr >= 0 {
                let sr = self.snaps[self.snap_ptr as usize];
                self.pd = Some(sr.pd);
                self.pu = sr.pu;
            } else {
                self.pd = None;
                self.pu = None;
            }
        }
        let pd = self.pd;
        let pu = self.pu;
        let tp_up = self.o.th.tp_up;
        let up_th = self.o.th.up_th;
        let ii = i as i64;
        for pi in 0..self.plans.len() {
            if self.plans[pi].done {
                continue;
            }
            // isi pesanan yang tertunda (jeda eksekusi)
            for qi in 0..self.plans[pi].pend.len() {
                let (filled, qtrig, qtgt, qtok) = {
                    let q = &self.plans[pi].pend[qi];
                    (q.filled, q.trig, q.tgt, q.tok)
                };
                if !filled && qtrig < ii && xblk >= qtgt {
                    let val = self.realize(j, qtok);
                    let p = &mut self.plans[pi];
                    p.proceeds += val;
                    p.pend[qi].filled = true;
                }
            }
            if let Some(q) = self.plans[pi].trig {
                if q.trig < ii && xblk >= q.tgt {
                    self.close(pi, j);
                }
                continue;
            }
            let c = self.plans[pi].c;
            // TP1 DITUNDA kalau p_naik masih tinggi
            let tunda_tp = tp_up < 1.0 && pu.map_or(false, |u| u >= tp_up);
            if let Some(ladder) = self.o.tp_ladder.clone() {
                for (li, &(lv, lf)) in ladder.iter().enumerate() {
                    if self.plans[pi].lad[li] || v < lv {
                        continue;
                    }
                    self.plans[pi].lad[li] = true;
                    let tok = js_min(self.plans[pi].rem, self.base * lf);
                    if tok <= 0.0 {
                        continue;
                    }
                    let kk = self.retries(&format!("{}|lad{}", self.plans[pi].name, li));
                    let tgt = xblk + self.latb * (1 + kk) as f64 + self.nd_t();
                    self.n_fail += kk;
                    let p = &mut self.plans[pi];
                    p.pend.push(Pend { trig: ii, tgt, tok, filled: false });
                    p.rem -= tok;
                    p.txs += 1 + kk;
                    if !p.tp_done {
                        p.tp_done = true;
                        p.tp1_blk = Some(xblk);
                        p.tp1_v = Some(v);
                    }
                    let e = Event::Tp1 { plan: p.name.clone(), pool: self.m.id.clone(), blk: xblk, v, tgt, tx: self.sw[i].tx.clone() };
                    self.ev(e);
                }
            }
            if self.o.tp_ladder.is_none() && !self.plans[pi].tp_done && c.frac > 0.0 && v >= c.tp && !tunda_tp {
                let tok = self.base * c.frac;
                let k1 = self.retries(&format!("{}|tp1", self.plans[pi].name));
                let tgt = xblk + self.latb * (1 + k1) as f64 + self.nd_t();
                let p = &mut self.plans[pi];
                p.pend.push(Pend { trig: ii, tgt, tok, filled: false });
                p.rem -= tok;
                p.txs += 1 + k1;
                p.tp_done = true;
                p.tp1_blk = Some(xblk);
                p.tp1_v = Some(v);
                self.n_fail += k1;
                let e = Event::Tp1 { plan: self.plans[pi].name.clone(), pool: self.m.id.clone(), blk: xblk, v, tgt, tx: self.sw[i].tx.clone() };
                self.ev(e);
                continue;
            }
            let tp_done = self.plans[pi].tp_done;
            let mut why: Option<String> = None;
            let pelari = self.arm_at > 0.0 && self.peak >= self.arm_at;
            if self.o.rug_exit && self.rug_blk.map_or(false, |rb| xblk >= rb) {
                why = Some("rug terdeteksi".to_string());
            }
            if why.is_none() && self.o.warn_tick && self.warn_blk.map_or(false, |wb| xblk >= wb) {
                why = Some("LP ditarik >30%".to_string());
            }
            if why.is_none() && pelari {
                if self.warn_blk.map_or(false, |wb| xblk > wb) {
                    why = Some("LP ditarik >30%".to_string());
                } else if self.arm_dump > 0.0 && !xbuy && xq >= self.arm_dump * self.lp[i] {
                    why = Some("dump pelari".to_string());
                } else if v <= self.peak * (1.0 - self.arm_trail) {
                    why = Some(format!("trailing pelari {}%", num_to_string(js_round(100.0 * self.arm_trail))));
                }
                if let Some(w) = why {
                    self.set_exit(pi, ii, xblk, v, w, i);
                }
                continue; // mode pelari: aturan lama TIDAK berlaku
            }
            if why.is_none() {
                if let Some(sb) = self.stale_b {
                    if !tp_done && xblk >= self.e_blk + sb {
                        why = Some("tanpa kemajuan".to_string());
                    }
                }
            }
            if why.is_none() && self.warn_blk.map_or(false, |wb| xblk > wb) {
                why = Some("LP ditarik >30%".to_string());
            }
            if why.is_none() && !xbuy && xq >= self.dump_frac * self.lp[i] {
                why = Some(format!("dump >={}% LP", num_to_string(100.0 * self.dump_frac)));
            }
            if why.is_none()
                && self.peak >= 3.0
                && self.s >= 3
                && self.s_cnt >= 6
                && (self.s_usd_w / self.s as f64) >= 1.5 * (self.s_usd / self.s_cnt as f64)
            {
                why = Some("tanda pucuk".to_string());
            }
            // momentum orderflow (bagian 14): lemah = kedatangan pembeli turun separuh & aliran bersih jual
            let mom_ok = self.mom_b.map_or(false, |w| xblk >= self.e_blk + w);
            let mom_lemah = mom_ok && self.m_nb1 * 2 <= self.m_nb2 && self.m_bu1 <= self.m_su1;
            let mom_kuat = mom_ok && self.m_nb1 >= self.m_nb2 && self.m_nb1 >= 3 && self.m_bu1 > self.m_su1;
            if why.is_none() && mom_lemah {
                why = Some("momentum melemah".to_string());
            }
            let tahan = (up_th < 1.0 && pu.map_or(false, |u| u >= up_th)) || (self.o.mom_hold && mom_kuat);
            if why.is_none() && tp_done && c.run_danger && pd.map_or(false, |d| d >= c.th) && !tahan {
                why = Some("model bahaya".to_string());
            }
            if why.is_none() && tp_done && c.run_danger && self.trail > 0.0 && v <= self.peak * (1.0 - self.trail) {
                why = Some(format!("trailing {}%", num_to_string(js_round(100.0 * self.trail))));
            }
            if why.is_none() && tp_done && c.run_danger && v <= self.peak * 0.10 {
                why = Some("stop bencana".to_string());
            }
            if why.is_none() && !(tp_done && c.run_danger) {
                let trv;
                if !tp_done && c.pre_hard40 {
                    trv = if self.trail_pre > 0.0 { self.trail_pre } else { 0.40 };
                } else {
                    let strong = self.b >= self.s && self.b >= 3;
                    trv = if self.peak >= 10.0 {
                        if strong {
                            0.50
                        } else {
                            0.20
                        }
                    } else if strong {
                        0.80
                    } else {
                        0.30
                    };
                }
                if v <= self.peak * (1.0 - trv) {
                    why = Some(if tp_done { "stop arus".to_string() } else { "stop -40%".to_string() });
                }
            }
            if let Some(w) = why {
                self.set_exit(pi, ii, xblk, v, w, i);
            }
        }
    }

    fn set_exit(&mut self, pi: usize, ii: i64, xblk: f64, v: f64, why: String, i: usize) {
        let k = self.retries(&format!("{}|exit", self.plans[pi].name));
        self.n_fail += k;
        let tgt = xblk + self.latb * (1 + k) as f64 + self.nd_x();
        let p = &mut self.plans[pi];
        p.txs += k;
        p.trig = Some(Trig { trig: ii, tgt, timed: false });
        p.reason = Some(why.clone());
        let e = Event::ExitSignal { plan: p.name.clone(), pool: self.m.id.clone(), blk: xblk, v: Some(v), why, tgt, tx: self.sw[i].tx.clone() };
        self.ev(e);
    }

    fn close(&mut self, pi: usize, idx: usize) {
        let rem = self.plans[pi].rem;
        if rem > self.base * 1e-9 {
            let val = self.realize(idx, rem);
            self.plans[pi].proceeds += val;
        }
        self.emit(pi, idx, None);
    }

    /// jual pada kondisi pool SAAT INI tanpa swap baru
    fn state_now(&self) -> St {
        let mut s = St::of(&self.sw[self.sw.len() - 1]);
        if self.o.lp_adjust && self.a.sign() == Sign::Plus {
            if let (Some(aa), Some(ra)) = (&self.a_at_last, &self.r_at_last) {
                let rem_now = big_to_f64(&(&self.a - &self.r));
                let rem_at = big_to_f64(&(aa - ra));
                if rem_at > 0.0 {
                    s.l = s.l * js_max(0.0, js_min(1.0, rem_now / rem_at));
                }
            }
        }
        self.adj(s)
    }
    fn realize_now(&mut self, blk: f64, tok: f64, commit: bool) -> f64 {
        if let Some(rb) = self.rug_blk {
            if rb <= blk {
                return 0.0;
            }
        }
        let v = self.sell_val(self.state_now(), tok);
        if commit && self.o.sell_impact != SellImpact::Off {
            self.off += tok;
        }
        v
    }

    pub fn entry_tgt(&self) -> f64 {
        if !self.entry_lat_on() || self.ei >= 0 || self.skip || self.closed || self.sw.is_empty() {
            return f64::INFINITY;
        }
        let d = self.decide_blk.unwrap_or(self.m.init + MIN);
        d + self.o.entry_lat + self.nd_e()
    }

    fn enter_at(&mut self, tgt: f64) {
        let i = self.sw.len() - 1;
        let s = self.state_now();
        // FILTER ML (opsional)
        if let Some(g) = self.o.gate.clone() {
            match g(&*self) {
                None => return,
                Some(false) => {
                    self.skip = true;
                    self.skip_why = Some("disaring filter".to_string());
                    return;
                }
                Some(true) => {}
            }
        }
        if tr(self.fail_buy) && h32(&format!("{}|buy", self.m.id)) < self.fail_buy {
            // beli gagal: tidak dapat koin, gas tetap terbayar
            self.skip = true;
            self.skip_why = Some("beli gagal".to_string());
            let size = self.hitung_ukuran(self.lp_usd(s));
            if size.is_finite() && size >= self.min_usd {
                for pi in 0..self.plans.len() {
                    let t = Trade {
                        plan: self.plans[pi].name.clone(),
                        pool: self.m.id.clone(),
                        init: self.m.init,
                        entry_blk: tgt,
                        exit_blk: tgt,
                        size,
                        pnl: -self.gas,
                        ret: 1.0 - self.gas / size,
                        gas_tot: self.gas,
                        txs: f64::NAN,
                        hold: 0.0,
                        reason: "beli gagal".to_string(),
                        tp1: false,
                        tp1_blk: None,
                        tp1_v: None,
                        peak_v: 1.0,
                        perfect: 1.0,
                        eth_usd: self.eth_usd,
                        fails: Some(1.0),
                    };
                    self.trades.push(t);
                }
            }
            return;
        }
        self.ei = i as i64;
        self.e_blk = tgt;
        self.size = self.hitung_ukuran(self.lp_usd(s));
        if !self.size.is_finite() || self.size < self.min_usd {
            self.skip = true;
            self.skip_why = Some(format!("ukuran<${}", num_to_string(self.min_usd)));
            return;
        }
        let d_in = (self.size / self.eth_usd) * 1e18 * (1.0 - s.fee / 1e6);
        let base = if self.q_is_c1 {
            let sp1 = s.sqrt_p + d_in / s.l;
            s.l * (1.0 / s.sqrt_p - 1.0 / sp1)
        } else {
            let inv = 1.0 / s.sqrt_p + d_in / s.l;
            s.l * (s.sqrt_p - 1.0 / inv)
        };
        if !base.is_finite() || base <= 0.0 {
            self.skip = true;
            self.skip_why = Some("base".to_string());
            return;
        }
        self.base = base;
        for p in self.plans.iter_mut() {
            p.rem = base;
        }
        self.v[i] = self.sell_val(s, base) / self.size;
        if self.o.sell_impact == SellImpact::Full {
            self.off = -base;
        }
        let e = Event::Entry { pool: self.m.id.clone(), blk: tgt, size: self.size, lp: self.lp_usd(s), tx: Some(self.sw[i].tx.clone()) };
        self.ev(e);
    }

    fn hitung_ukuran(&self, lp: f64) -> f64 {
        if let Some(f) = &self.o.size_fn {
            return f(lp, &self.m);
        }
        js_min(self.max_usd, self.lp_frac * lp)
    }

    pub fn next_deadline(&self) -> f64 {
        if self.closed || self.skip {
            return f64::INFINITY;
        }
        if self.ei < 0 {
            return self.entry_tgt();
        }
        let mut d = f64::INFINITY;
        let last_blk = self.sw[self.sw.len() - 1].blk;
        for p in &self.plans {
            if p.done {
                continue;
            }
            if self.o.fill_on_time {
                for q in &p.pend {
                    if !q.filled {
                        d = js_min(d, q.tgt);
                    }
                }
            }
            if let Some(t) = p.trig {
                if self.o.fill_on_time || t.timed {
                    d = js_min(d, t.tgt);
                }
                continue;
            }
            if let Some(sb) = self.stale_b {
                if !p.tp_done {
                    d = js_min(d, self.e_blk + sb);
                }
            }
            if let Some(qb) = self.quiet_b {
                if p.tp_done || self.o.quiet_all {
                    d = js_min(d, last_blk + qb);
                }
            }
        }
        d
    }

    pub fn tick(&mut self, blk: f64) {
        if self.closed || self.skip || blk > self.m.init + self.hz {
            return;
        }
        if self.ei < 0 {
            if self.entry_tgt() <= blk {
                let t = self.entry_tgt();
                self.enter_at(t);
            }
            return;
        }
        let last = self.sw.len() - 1;
        let last_blk = self.sw[last].blk;
        if blk <= last_blk {
            return;
        }
        for pi in 0..self.plans.len() {
            if self.plans[pi].done {
                continue;
            }
            let timed = self.plans[pi].trig.map_or(false, |t| t.timed);
            if self.o.fill_on_time || timed {
                for qi in 0..self.plans[pi].pend.len() {
                    let (filled, qtgt, qtok) = {
                        let q = &self.plans[pi].pend[qi];
                        (q.filled, q.tgt, q.tok)
                    };
                    if !filled && blk >= qtgt {
                        let val = self.realize_now(blk, qtok, true);
                        let p = &mut self.plans[pi];
                        p.proceeds += val;
                        p.pend[qi].filled = true;
                    }
                }
            }
            if let Some(t) = self.plans[pi].trig {
                if (self.o.fill_on_time || t.timed) && blk >= t.tgt {
                    let rem = self.plans[pi].rem;
                    if rem > self.base * 1e-9 {
                        let val = self.realize_now(blk, rem, true);
                        self.plans[pi].proceeds += val;
                    }
                    self.emit(pi, last, Some(blk));
                }
                continue;
            }
            let tp_done = self.plans[pi].tp_done;
            let why = if self.stale_b.map_or(false, |sb| !tp_done && blk >= self.e_blk + sb) {
                Some("tanpa kemajuan")
            } else if self.quiet_b.map_or(false, |qb| (tp_done || self.o.quiet_all) && blk >= last_blk + qb) {
                Some("pool sepi")
            } else {
                None
            };
            if let Some(w) = why {
                let k = self.retries(&format!("{}|exit", self.plans[pi].name));
                self.n_fail += k;
                let tgt = blk + self.latb * (1 + k) as f64 + self.nd_x();
                let p = &mut self.plans[pi];
                p.txs += k;
                p.trig = Some(Trig { trig: last as i64, tgt, timed: true });
                p.reason = Some(w.to_string());
                let e = Event::ExitSignal { plan: p.name.clone(), pool: self.m.id.clone(), blk, v: None, why: w.to_string(), tgt, tx: self.sw[last].tx.clone() };
                self.ev(e);
            }
        }
    }

    /// nilai posisi SEKARANG (mark-to-market) untuk satu plan
    pub fn nilai_sekarang(&mut self, nama_plan: &str) -> Option<f64> {
        if self.ei < 0 || self.skip || self.sw.is_empty() {
            return None;
        }
        let pi = self.plans.iter().position(|p| p.name == nama_plan)?;
        if self.plans[pi].done {
            return None;
        }
        let blk = self.sw[self.sw.len() - 1].blk;
        let rem = self.plans[pi].rem;
        let sisa = if rem > 0.0 { self.realize_now(blk, rem, false) } else { 0.0 };
        Some((self.plans[pi].proceeds + sisa) / self.size)
    }

    /// nama plan sesuai urutan (padanan `sim.plans.map(p => p.name)`)
    pub fn plan_names(&self) -> Vec<String> {
        self.plans.iter().map(|p| p.name.clone()).collect()
    }

    pub fn all_done(&self) -> bool {
        self.skip || (self.ei >= 0 && self.plans.iter().all(|p| p.done))
    }

    fn retries(&self, tag: &str) -> i64 {
        let mut k: i64 = 0;
        while tr(self.fail_sell) && k < 5 && h32(&format!("{}|{}|{}", self.m.id, tag, k)) < self.fail_sell {
            k += 1;
        }
        k
    }

    fn emit(&mut self, pi: usize, idx: usize, at_blk: Option<f64>) {
        let gas = self.gas;
        let approve = self.approve_usd;
        let size = self.size;
        let e_blk = self.e_blk;
        let xb = at_blk.unwrap_or(self.sw[idx].blk);
        let p = &mut self.plans[pi];
        p.txs += 1;
        p.done = true;
        let gas_tot = gas * p.txs as f64 + approve;
        let pnl = p.proceeds - size - gas_tot;
        let t = Trade {
            plan: p.name.clone(),
            pool: self.m.id.clone(),
            init: self.m.init,
            entry_blk: e_blk,
            exit_blk: xb,
            size,
            pnl,
            ret: (p.proceeds - gas_tot) / size,
            gas_tot,
            txs: p.txs as f64,
            hold: (xb - e_blk) * 0.1 / 60.0,
            reason: p.reason.clone().unwrap_or_else(|| "batas 6 jam".to_string()),
            tp1: p.tp_done,
            tp1_blk: p.tp1_blk,
            tp1_v: p.tp1_v,
            peak_v: self.peak,
            perfect: self.perfect,
            eth_usd: self.eth_usd,
            fails: None,
        };
        self.trades.push(t);
    }

    /// dipanggil setelah semua blok <= init+HZ diproses
    pub fn finalize(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if self.skip || self.ei < 0 {
            return;
        }
        let n = self.sw.len() as i64;
        if self.ei >= n - 1 && !self.o.keep_no_after {
            return;
        }
        let last = self.sw.len() - 1;
        let mut end_sw = St::of(&self.sw[last]);
        if self.o.lp_adjust && self.a.sign() == Sign::Plus {
            let rem_now = big_to_f64(&(&self.a - &self.r));
            let rem_at = match (&self.a_at_last, &self.r_at_last) {
                (Some(aa), Some(ra)) => big_to_f64(&(aa - ra)),
                _ => f64::NAN,
            };
            if rem_at > 0.0 {
                end_sw.l = end_sw.l * js_max(0.0, js_min(1.0, rem_now / rem_at));
            }
        }
        let horizon = self.m.init + self.hz;
        for pi in 0..self.plans.len() {
            if self.plans[pi].done {
                continue;
            }
            for qi in 0..self.plans[pi].pend.len() {
                if !self.plans[pi].pend[qi].filled {
                    let tok = self.plans[pi].pend[qi].tok;
                    let val = self.realize(last, tok);
                    let p = &mut self.plans[pi];
                    p.proceeds += val;
                    p.pend[qi].filled = true;
                }
            }
            if self.plans[pi].trig.is_some() {
                self.close(pi, last);
                continue;
            }
            let add = if self.rug_blk.map_or(false, |rb| rb <= horizon) { 0.0 } else { self.sell_val(self.adj(end_sw), self.plans[pi].rem) };
            self.plans[pi].proceeds += add;
            if self.ei >= n - 1 {
                self.plans[pi].reason = Some("tanpa swap setelah masuk".to_string());
            } else if self.hz != HZ {
                self.plans[pi].reason = Some("akhir data".to_string());
            }
            let at = if self.hz != HZ && horizon.is_finite() { Some(horizon) } else { None };
            self.emit(pi, last, at);
        }
    }
}
