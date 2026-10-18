//! feat_dump: hitung fitur untuk semua koin dataset dan tulis JSONL dengan format yang
//! sama dengan acuan/dump_js.js (angka diformat seperti JSON.stringify JS).
//!
//! Pakai: feat_dump <out.jsonl> [--root DIR] [--repeat N] [ds ...]

use port_features::dataset::load_dataset;
use port_features::features::{features, Features, KEYS};
use port_features::jsnum::js_number_to_string;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

/// Seperti `JSON.stringify(string)`.
fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\u{08}' => o.push_str("\\b"),
            '\u{0C}' => o.push_str("\\f"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Sama dengan `enc` di dump_js.js: NaN/Infinity/-0 sebagai string.
fn enc(v: f64) -> String {
    if v.is_nan() {
        "\"NaN\"".into()
    } else if v == f64::INFINITY {
        "\"Infinity\"".into()
    } else if v == f64::NEG_INFINITY {
        "\"-Infinity\"".into()
    } else if v == 0.0 && v.is_sign_negative() {
        "\"-0\"".into()
    } else {
        js_number_to_string(v)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out_path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("pakai: feat_dump <out.jsonl> [--root DIR] [--repeat N] [ds ...]");
            std::process::exit(2);
        }
    };
    let mut root = PathBuf::from("C:/Users/horii/rhc-quant/rhc_vps/data");
    let mut repeat: u32 = 1;
    let mut ds_list: Vec<String> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = PathBuf::from(args.next().expect("--root butuh nilai")),
            "--repeat" => repeat = args.next().expect("--repeat butuh nilai").parse().expect("angka"),
            _ => ds_list.push(a),
        }
    }
    if ds_list.is_empty() {
        ds_list = ["m1", "m2", "u_ds5d", "u_ds24", "ags1", "ags4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    let repeat = repeat.max(1);

    let t0 = Instant::now();
    let mut t_load = 0f64;
    let mut t_feat = 0f64;
    let mut n_rows = 0usize;
    let mut out = String::new();
    for ds in &ds_list {
        let l0 = Instant::now();
        let jobs = match load_dataset(&root.join(ds)) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("{}: GAGAL memuat: {}", ds, e);
                std::process::exit(1);
            }
        };
        t_load += l0.elapsed().as_secs_f64();
        let mut res: Option<Vec<Features>> = None;
        for _ in 0..repeat {
            let f0 = Instant::now();
            let cur: Vec<Features> = jobs
                .iter()
                .map(|j| features(&j.meta, &j.swaps, &j.transfers, j.eth_usd))
                .collect();
            t_feat += f0.elapsed().as_secs_f64();
            if res.is_none() {
                res = Some(cur);
            }
        }
        let res = res.unwrap_or_default();
        for (i, (j, f)) in jobs.iter().zip(res.iter()).enumerate() {
            out.push_str("{\"ds\":");
            out.push_str(&json_str(ds));
            out.push_str(",\"i\":");
            out.push_str(&i.to_string());
            out.push_str(",\"pool\":");
            out.push_str(&json_str(&j.pool));
            out.push_str(",\"f\":{");
            for (k, key) in KEYS.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push_str(&json_str(key));
                out.push(':');
                out.push_str(&enc(f.vals[k]));
            }
            out.push_str("}}\n");
        }
        n_rows += jobs.len();
        eprintln!("{}: {} koin", ds, jobs.len());
    }
    let mut fh = std::fs::File::create(&out_path).expect("buat berkas keluaran");
    fh.write_all(out.as_bytes()).expect("tulis keluaran");
    eprintln!(
        "RUST rows={} total_ms={:.1} load_ms={:.1} feat_ms_per_pass={:.1} repeat={}",
        n_rows,
        t0.elapsed().as_secs_f64() * 1e3,
        t_load * 1e3,
        t_feat * 1e3 / f64::from(repeat),
        repeat
    );
}
