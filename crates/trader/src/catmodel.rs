//! Skor CatBoost (oblivious trees) untuk filter beli.
//! Wajib cocok bit-per-bit dengan catboost Python & cat_model.js: perbandingan fitur dilakukan
//! dalam f32 (CatBoost menyimpan fitur sebagai float32), penjumlahan daun dalam f64.
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
        let j: Value = serde_json::from_str(&raw).expect("filter_cat.json rusak");
        let feats = j["feats"].as_array().unwrap().iter()
            .map(|v| v.as_str().unwrap().to_string()).collect();
        let trees = j["trees"].as_array().unwrap().iter().map(|t| {
            let sp = t["splits"].as_array().unwrap();
            Tree {
                feat: sp.iter().map(|s| s["feat"].as_u64().unwrap() as usize).collect(),
                border: sp.iter().map(|s| s["border"].as_f64().unwrap() as f32).collect(),
                leaf: t["leaf"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect(),
            }
        }).collect();
        Ok(CatModel {
            trees, feats,
            scale: j["scale"].as_f64().unwrap_or(1.0),
            bias: j["bias"].as_f64().unwrap_or(0.0),
        })
    }

    /// Peluang koin ini menang (0..1).
    pub fn predict(&self, x: &[f64]) -> f64 {
        let mut s = 0.0f64;
        for t in &self.trees {
            let mut idx = 0usize;
            for k in 0..t.feat.len() {
                if (x[t.feat[k]] as f32) > t.border[k] { idx |= 1 << k; }
            }
            s += t.leaf[idx];
        }
        let z = self.scale * s + self.bias;
        1.0 / (1.0 + (-z).exp())
    }
}
