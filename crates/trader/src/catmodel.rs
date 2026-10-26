//! Skor CatBoost (oblivious trees) untuk filter beli.
//! Wajib cocok bit-per-bit dengan CatBoost sendiri: perbandingan fitur dilakukan dalam f32 (CatBoost menyimpan fitur
//! sebagai float32), penjumlahan daun dalam f64.
use serde_json::Value;

pub struct Tree {
    pub feat: Vec<usize>,
    pub border: Vec<f32>,
    pub leaf: Vec<f64>,
}

pub struct CatModel {
    pub trees: Vec<Tree>,
    pub feats: Vec<String>,
    pub scale: f64,
    pub bias: f64,
}

impl CatModel {
    pub fn load(path: &str) -> std::io::Result<CatModel> {
        let raw = std::fs::read_to_string(path)?;
        let j: Value = serde_json::from_str(&raw).map_err(|e| std::io::Error::other(format!("{path}: {e}")))?;
        CatModel::from_value(&j).map_err(|e| std::io::Error::other(format!("{path}: {e}")))
    }

    /// filter_cat.json format: {feats, trees: [{splits: [{feat, border}], leaf}], scale, bias}. Never panics.
    pub fn from_value(j: &Value) -> Result<CatModel, String> {
        let feats: Vec<String> = j["feats"].as_array().ok_or("feats")?.iter().map(|v| v.as_str().map(|s| s.to_string()).ok_or("feats")).collect::<Result<_, _>>()?;
        let mut trees = Vec::new();
        for t in j["trees"].as_array().ok_or("trees")? {
            let sp = t["splits"].as_array().ok_or("splits")?;
            let feat: Vec<usize> = sp.iter().map(|s| s["feat"].as_u64().map(|v| v as usize).ok_or("feat")).collect::<Result<_, _>>()?;
            let border: Vec<f32> = sp.iter().map(|s| s["border"].as_f64().map(|v| v as f32).ok_or("border")).collect::<Result<_, _>>()?;
            let leaf: Vec<f64> = t["leaf"].as_array().ok_or("leaf")?.iter().map(|v| v.as_f64().ok_or("leaf")).collect::<Result<_, _>>()?;
            if leaf.len() != 1usize << feat.len() || feat.iter().any(|f| *f >= feats.len()) {
                return Err("pohon rusak (jumlah daun / indeks fitur)".into());
            }
            trees.push(Tree { feat, border, leaf });
        }
        if trees.is_empty() {
            return Err("model tanpa pohon".into());
        }
        Ok(CatModel { trees, feats, scale: j["scale"].as_f64().unwrap_or(1.0), bias: j["bias"].as_f64().unwrap_or(0.0) })
    }

    /// Peluang koin ini menang (0..1).
    pub fn predict(&self, x: &[f64]) -> f64 {
        let mut s = 0.0f64;
        for t in &self.trees {
            let mut idx = 0usize;
            for k in 0..t.feat.len() {
                if (x[t.feat[k]] as f32) > t.border[k] {
                    idx |= 1 << k;
                }
            }
            s += t.leaf[idx];
        }
        let z = self.scale * s + self.bias;
        1.0 / (1.0 + (-z).exp())
    }
}
