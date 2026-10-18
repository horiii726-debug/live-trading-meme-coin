//! feat_flow: fitur orderflow tambahan (lihat orderflow.rs) untuk semua koin dataset -> CSV.
//!
//! Pakai: feat_flow <out.csv> [--root DIR] [ds ...]

use port_features::dataset::load_dataset;
use port_features::orderflow::{orderflow, KEYS};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let out_path = args.next().expect("pakai: feat_flow <out.csv> [--root DIR] [ds ...]");
    let mut root = PathBuf::from("C:/Users/horii/rhc-quant/data_b");
    let mut ds_list: Vec<String> = Vec::new();
    while let Some(a) = args.next() {
        if a == "--root" { root = PathBuf::from(args.next().expect("--root butuh nilai")); } else { ds_list.push(a); }
    }
    let mut fh = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("buat keluaran"));
    write!(fh, "ds,pool").unwrap();
    for k in KEYS { write!(fh, ",{}", k).unwrap(); }
    writeln!(fh).unwrap();
    for ds in &ds_list {
        let jobs = load_dataset(&root.join(ds)).unwrap_or_else(|e| { eprintln!("{}: GAGAL {}", ds, e); std::process::exit(1) });
        for j in &jobs {
            let f = orderflow(&j.meta, &j.swaps, &j.transfers, j.eth_usd);
            write!(fh, "{},{}", ds, j.pool).unwrap();
            for v in f { if v.is_finite() { write!(fh, ",{}", v).unwrap(); } else { write!(fh, ",").unwrap(); } }
            writeln!(fh).unwrap();
        }
        eprintln!("{}: {} koin", ds, jobs.len());
    }
}
