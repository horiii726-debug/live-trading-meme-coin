//! Port `loadModel` (v4engine.js): pohon HistGradientBoosting dari JSON.
//! predict(x) = 1 / (1 + Math.exp(-(baseline + sum value_daun))) dengan penjumlahan urut pohon.
//! Nilai hilang (null/undefined di JS) diwakili NaN.

use crate::jsnum;
use serde_json::Value;

/// Satu simpul pohon, 16 byte (semua pohon disimpan berurutan dalam satu array supaya ramah cache).
/// `v` = ambang (simpul dalam) atau nilai daun (simpul daun). Anak memakai indeks lokal pohon.
/// Isi dan logika lintasannya identik dengan array terpisah di JSON (value, feature_idx, ...).
#[derive(Clone, Copy, Debug)]
struct Node {
    v: f64,
    left: u16,
    right: u16,
    feat: u16,
    flags: u16, // bit0 = daun, bit1 = missing_go_to_left
}
const LEAF: u16 = 1;
const MISS_LEFT: u16 = 2;

pub struct Model {
    pub feats: Vec<String>,
    pub baseline: f64,
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

/// truthiness JS untuk nilai JSON
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            !(f == 0.0 || f.is_nan())
        }
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// ToNumber JS untuk nilai JSON (null -> 0, bool -> 0/1, string -> Number(str)).
pub fn to_num(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => jsnum::to_number(s),
        Value::Array(a) if a.is_empty() => 0.0,
        Value::Array(a) if a.len() == 1 => to_num(&a[0]),
        _ => f64::NAN,
    }
}

fn arr<'a>(t: &'a Value, k: &str) -> Result<&'a Vec<Value>, String> {
    t.get(k).and_then(|v| v.as_array()).ok_or_else(|| format!("pohon tanpa array '{}'", k))
}

fn idx(v: &Value) -> Result<usize, String> {
    let f = to_num(v);
    if f >= 0.0 && f.fract() == 0.0 && f < 1e15 {
        Ok(f as usize)
    } else {
        Err(format!("indeks tidak sah: {}", v))
    }
}

impl Model {
    pub fn load(path: &str) -> Result<Model, String> {
        let txt = std::fs::read_to_string(path).map_err(|e| format!("baca {}: {}", path, e))?;
        Model::from_json_str(&txt).map_err(|e| format!("{}: {}", path, e))
    }

    pub fn from_json_str(txt: &str) -> Result<Model, String> {
        let m: Value = serde_json::from_str(txt).map_err(|e| e.to_string())?;
        let feats = m
            .get("feats")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        let baseline = to_num(m.get("baseline").unwrap_or(&Value::Null));
        let mut nodes: Vec<Node> = Vec::new();
        let mut roots = Vec::new();
        for t in m.get("trees").and_then(|v| v.as_array()).ok_or("model tanpa 'trees'")? {
            let value: Vec<f64> = arr(t, "value")?.iter().map(to_num).collect();
            let feature_idx: Vec<usize> = arr(t, "feature_idx")?.iter().map(idx).collect::<Result<_, _>>()?;
            let num_threshold: Vec<f64> = arr(t, "num_threshold")?.iter().map(to_num).collect();
            let missing_go_to_left: Vec<bool> = arr(t, "missing_go_to_left")?.iter().map(truthy).collect();
            let left: Vec<usize> = arr(t, "left")?.iter().map(idx).collect::<Result<_, _>>()?;
            let right: Vec<usize> = arr(t, "right")?.iter().map(idx).collect::<Result<_, _>>()?;
            let is_leaf: Vec<bool> = arr(t, "is_leaf")?.iter().map(truthy).collect();
            let n = is_leaf.len();
            if [value.len(), feature_idx.len(), num_threshold.len(), missing_go_to_left.len(), left.len(), right.len()].iter().any(|&l| l != n) || n == 0 {
                return Err("panjang array pohon tidak sama".into());
            }
            let off = nodes.len();
            roots.push(off);
            if n > u16::MAX as usize {
                return Err("pohon terlalu besar (>65535 simpul)".into());
            }
            for k in 0..n {
                if is_leaf[k] {
                    nodes.push(Node { v: value[k], left: 0, right: 0, feat: 0, flags: LEAF });
                    continue;
                }
                if left[k] >= n || right[k] >= n {
                    return Err("indeks anak di luar pohon".into());
                }
                if feature_idx[k] > u16::MAX as usize {
                    return Err("indeks fitur terlalu besar".into());
                }
                nodes.push(Node {
                    v: num_threshold[k],
                    left: left[k] as u16,
                    right: right[k] as u16,
                    feat: feature_idx[k] as u16,
                    flags: if missing_go_to_left[k] { MISS_LEFT } else { 0 },
                });
            }
        }
        Ok(Model { feats, baseline, nodes, roots })
    }

    /// Sama persis dengan predict di loadModel (v4engine.js).
    pub fn predict(&self, x: &[f64]) -> f64 {
        let mut raw = self.baseline;
        for &r in &self.roots {
            let t = &self.nodes[r..];
            let mut nd = &t[0];
            while nd.flags & LEAF == 0 {
                // x[i] di luar panjang array = undefined -> NaN
                let v = x.get(nd.feat as usize).copied().unwrap_or(f64::NAN);
                let go_left = if v.is_nan() { nd.flags & MISS_LEFT != 0 } else { v <= nd.v };
                nd = &t[(if go_left { nd.left } else { nd.right }) as usize];
            }
            raw += nd.v;
        }
        1.0 / (1.0 + jsnum::exp(-raw))
    }
}
