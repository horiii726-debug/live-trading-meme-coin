//! Uji parity model pohon: Rust predict vs file *_check.json (hasil sklearn) dan, opsional, vs prediksi JS.
//! Pakai: model_parity <model.json> <check.json> [js_pred.json]
//!   js_pred.json = array angka hasil loadModel(model).predict(X[k]) di Node (lihat uji/js_predict.js)
use port_engine::model::{to_num, Model};
use serde_json::Value;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("pakai: model_parity <model.json> <check.json> [js_pred.json]");
        std::process::exit(2);
    }
    let m = Model::load(&a[0]).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1) });
    let ck: Value = serde_json::from_str(&std::fs::read_to_string(&a[1]).expect("baca check")).expect("json check");
    let xs = ck["X"].as_array().expect("X");
    let ps = ck["p"].as_array().expect("p");
    let js: Option<Vec<f64>> = a.get(2).map(|f| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(f).expect("baca js_pred")).expect("json js_pred");
        v.as_array().expect("array").iter().map(to_num).collect()
    });
    let (mut mx, mut mx_js, mut n_bit_js) = (0.0f64, 0.0f64, 0usize);
    for (k, x) in xs.iter().enumerate() {
        let xv: Vec<f64> = x.as_array().unwrap().iter().map(|e| if e.is_null() { f64::NAN } else { to_num(e) }).collect();
        let p = m.predict(&xv);
        let d = (p - to_num(&ps[k])).abs();
        if !(d <= mx) { mx = d; }
        if let Some(j) = &js {
            let dj = (p - j[k]).abs();
            if !(dj <= mx_js) { mx_js = dj; }
            if p.to_bits() == j[k].to_bits() { n_bit_js += 1; }
        }
    }
    println!("model={} sampel={} selisih_maks_vs_sklearn={:e}", a[0], xs.len(), mx);
    if js.is_some() {
        println!("  vs_JS: selisih_maks={:e} identik_bit={}/{}", mx_js, n_bit_js, xs.len());
    }
    let ok = mx < 1e-12 && (js.is_none() || n_bit_js == xs.len());
    println!("  {}", if ok { "LOLOS" } else { "GAGAL" });
    std::process::exit(if ok { 0 } else { 1 });
}
