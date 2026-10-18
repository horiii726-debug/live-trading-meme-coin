//! Pemuat dataset yang meniru persis penyiapan input di `rhc_vps/pipeline/build_feats2.js`:
//! trades.jsonl (plan v4) + swaps_m1.json + transfers.json + pools.csv.
//!
//! Supaya cepat, swaps_m1.json dan transfers.json tidak diubah menjadi pohon `Value`:
//! tingkat atas dibaca sebagai peta `pool -> &RawValue` (tanpa salin), lalu hanya pool
//! yang dipakai trade yang diurai, sel demi sel dari teks JSON aslinya. Semantik nilai
//! tetap semantik JS (`ToNumber`, kebenaran `!x`), lihat fungsi `raw_*` di bawah.

use crate::features::{Meta, Swap, Transfer};
use crate::jsnum::{js_lower, js_number_to_string, js_string_to_number, js_trim};
use serde_json::value::RawValue;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

/// `Math.pow(2, 96)` (eksak).
pub const Q96: f64 = 79228162514264337593543950336.0;

/// Satu baris kerja = satu panggilan `features(meta, swaps, tr, t.ethUsd)` di build_feats2.js.
pub struct Job {
    pub pool: String,
    pub meta: Meta,
    pub swaps: Vec<Swap>,
    pub transfers: Vec<Transfer>,
    pub eth_usd: f64,
}

/// `String(v)` JS untuk nilai JSON (dipakai oleh ToNumber pada array).
fn js_to_string(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number_to_string(json_number(n)),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|e| if e.is_null() { String::new() } else { js_to_string(e) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// Angka JSON -> f64 seperti `JSON.parse` (pembulatan IEEE terdekat dari teks aslinya).
fn json_number(n: &serde_json::Number) -> f64 {
    // arbitrary_precision: as_str() = teks asli di berkas; parser std Rust membulatkan benar.
    n.as_str().parse::<f64>().unwrap_or(f64::NAN)
}

/// ToNumber JS untuk nilai JSON; `None` = undefined -> NaN.
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(n)) => json_number(n),
        Some(Value::String(s)) => js_string_to_number(s),
        Some(a @ Value::Array(_)) => js_string_to_number(&js_to_string(a)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// Isi string JSON dari teks mentah (tanda kutip dibuang; escape diurai bila ada).
fn raw_string(text: &str) -> Result<Cow<'_, str>, String> {
    if text.len() >= 2 && !text[1..text.len() - 1].contains('\\') {
        Ok(Cow::Borrowed(&text[1..text.len() - 1]))
    } else {
        serde_json::from_str::<String>(text).map(Cow::Owned).map_err(|e| e.to_string())
    }
}

/// ToNumber JS untuk satu sel JSON mentah; `None` = undefined -> NaN.
fn raw_to_number(r: Option<&RawValue>) -> Result<f64, String> {
    let r = match r {
        None => return Ok(f64::NAN),
        Some(r) => r,
    };
    let t = r.get();
    Ok(match t.as_bytes().first() {
        Some(b'"') => js_string_to_number(&raw_string(t)?),
        Some(b'-') | Some(b'0'..=b'9') => t.parse::<f64>().unwrap_or(f64::NAN),
        Some(b'n') => 0.0, // null
        Some(b't') => 1.0, // true
        Some(b'f') => 0.0, // false
        Some(b'[') => {
            let v: Value = serde_json::from_str(t).map_err(|e| e.to_string())?;
            to_number(Some(&v))
        }
        _ => f64::NAN, // objek
    })
}

/// Sel yang di JS dipanggil `.toLowerCase()`: harus string (JS melempar TypeError jika tidak).
fn raw_lower(r: Option<&RawValue>, what: &str) -> Result<String, String> {
    match r {
        Some(r) if r.get().starts_with('"') => Ok(js_lower(&raw_string(r.get())?)),
        _ => Err(format!("{}: bukan string (JS akan error di toLowerCase)", what)),
    }
}

/// Kebenaran JS (`!!x`) untuk nilai JSON mentah hasil lookup objek.
fn raw_truthy(r: Option<&RawValue>) -> bool {
    let t = match r {
        None => return false,
        Some(r) => r.get(),
    };
    match t.as_bytes().first() {
        Some(b'n') => false,
        Some(b'f') => false,
        Some(b't') => true,
        Some(b'"') => t != "\"\"",
        Some(b'-') | Some(b'0'..=b'9') => {
            let x = t.parse::<f64>().unwrap_or(f64::NAN);
            !(x == 0.0 || x.is_nan())
        }
        _ => true, // array / objek
    }
}

fn read(dir: &Path, name: &str) -> Result<String, String> {
    std::fs::read_to_string(dir.join(name)).map_err(|e| format!("{}/{}: {}", dir.display(), name, e))
}

type Rows<'a> = Vec<Vec<&'a RawValue>>;

/// Muat satu folder dataset. Mengembalikan job dalam urutan yang sama dengan build_feats2.js.
pub fn load_dataset(dir: &Path) -> Result<Vec<Job>, String> {
    // const tr = readFileSync(trades.jsonl).trim().split("\n").map(JSON.parse).filter(t => t.plan === "v4");
    let trades_txt = read(dir, "trades.jsonl")?;
    let mut trades: Vec<Value> = Vec::new();
    for (ln, line) in js_trim(&trades_txt).split('\n').enumerate() {
        let t: Value = serde_json::from_str(line).map_err(|e| format!("trades.jsonl baris {}: {}", ln + 1, e))?;
        if t.get("plan").and_then(|p| p.as_str()) == Some("v4") {
            trades.push(t);
        }
    }
    // JSON.parse(swaps_m1.json) / JSON.parse(transfers.json): kunci ganda -> yang terakhir menang (sama dgn HashMap).
    let sw_txt = read(dir, "swaps_m1.json")?;
    let sw: HashMap<&str, &RawValue> = serde_json::from_str(&sw_txt).map_err(|e| format!("swaps_m1.json: {}", e))?;
    let tf_txt = read(dir, "transfers.json")?;
    let tf: HashMap<&str, &RawValue> =
        serde_json::from_str(&tf_txt).map_err(|e| format!("transfers.json: {}", e))?;
    // const pools = new Map(csv.trim().split("\n").slice(1).map(l => l.trim().split(",")).map(p => [p[0].toLowerCase(), p]));
    let pools_txt = read(dir, "pools.csv")?;
    let mut pools: HashMap<String, &str> = HashMap::new();
    for line in js_trim(&pools_txt).split('\n').skip(1) {
        let l = js_trim(line);
        let p0 = l.split(',').next().unwrap_or("");
        pools.insert(js_lower(p0), l);
    }

    let mut jobs = Vec::new();
    for t in &trades {
        let pool = t
            .get("pool")
            .and_then(|p| p.as_str())
            .ok_or("trade tanpa pool string")?
            .to_string();
        // const s0 = sw[t.pool], tt = tf[t.pool]; if (!s0 || !tt) { skip++; continue; }
        let s0 = sw.get(pool.as_str()).copied();
        let tt = tf.get(pool.as_str()).copied();
        if !raw_truthy(s0) || !raw_truthy(tt) {
            continue;
        }
        let s0: Rows = serde_json::from_str(s0.map(|r| r.get()).unwrap_or("null"))
            .map_err(|e| format!("{}: swaps bukan array baris: {}", pool, e))?;
        let tt: HashMap<&str, &RawValue> = serde_json::from_str(tt.map(|r| r.get()).unwrap_or("null"))
            .map_err(|e| format!("{}: entri transfers bukan objek: {}", pool, e))?;
        let tr_rows: Rows = serde_json::from_str(tt.get("tr").map(|r| r.get()).unwrap_or("null"))
            .map_err(|e| format!("{}: transfers.tr bukan array baris: {}", pool, e))?;
        let line = pools
            .get(&pool)
            .ok_or_else(|| format!("{}: tidak ada di pools.csv (JS akan error)", pool))?;
        let p: Vec<&str> = line.split(',').collect();
        if p.len() < 6 {
            return Err(format!("{}: baris pools.csv kurang kolom", pool));
        }
        let meta = Meta {
            c0: js_lower(p[1]),
            c1: js_lower(p[2]),
            fee: js_string_to_number(p[3]),
            hooks: js_lower(p[5]),
            init: to_number(t.get("init")),
        };
        // swaps = s0.map(x => ({blk:x[0], li:x[1], a0:+x[2], a1:+x[3], sqrtP:Number(x[4])/Q96, L:Number(x[5]), fee:x[6]}))
        let mut swaps = Vec::with_capacity(s0.len());
        for x in &s0 {
            let c = |i: usize| x.get(i).copied();
            swaps.push(Swap {
                blk: raw_to_number(c(0))?,
                li: raw_to_number(c(1))?,
                a0: raw_to_number(c(2))?,
                a1: raw_to_number(c(3))?,
                sqrt_p: raw_to_number(c(4))? / Q96,
                l: raw_to_number(c(5))?,
            });
        }
        // tt.tr.map(r => [r[0], r[1], r[2].toLowerCase(), r[3].toLowerCase(), r[4]]); Number(amt) di features()
        let mut transfers = Vec::with_capacity(tr_rows.len());
        for r in &tr_rows {
            let c = |i: usize| r.get(i).copied();
            transfers.push(Transfer {
                blk: raw_to_number(c(0))?,
                li: raw_to_number(c(1))?,
                from: raw_lower(c(2), &pool)?,
                to: raw_lower(c(3), &pool)?,
                amt: raw_to_number(c(4))?,
            });
        }
        jobs.push(Job { pool, meta, swaps, transfers, eth_usd: to_number(t.get("ethUsd")) });
    }
    Ok(jobs)
}
