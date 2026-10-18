//! math_check: bandingkan port fdlibm (`port_features::fdlibm`) dengan Math.log/Math.log10
//! V8 bit-per-bit, memakai berkas dari acuan/math_ref.js. Juga melaporkan berapa kasus
//! `f64::ln`/`f64::log10` bawaan Rust berbeda (alasan port diperlukan).
//!
//! Pakai: math_check <math_ref.bin>

use port_features::fdlibm;

fn same(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}

fn main() {
    let path = std::env::args().nth(1).expect("pakai: math_check <math_ref.bin>");
    let bytes = std::fs::read(&path).expect("baca berkas acuan");
    assert!(bytes.len() % 24 == 0, "ukuran berkas bukan kelipatan 24");
    let rd = |i: usize| -> f64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[i * 8..i * 8 + 8]);
        f64::from_le_bytes(b)
    };
    let n = bytes.len() / 24;
    let (mut bad_log, mut bad_log10, mut std_log, mut std_log10) = (0usize, 0usize, 0usize, 0usize);
    let mut shown = 0;
    for i in 0..n {
        let x = rd(3 * i);
        let jl = rd(3 * i + 1);
        let jl10 = rd(3 * i + 2);
        let rl = fdlibm::log(x);
        let rl10 = fdlibm::log10(x);
        if !same(rl, jl) {
            bad_log += 1;
            if shown < 10 {
                eprintln!("log beda x={:e} ({:016x}) js={:e} rust={:e}", x, x.to_bits(), jl, rl);
                shown += 1;
            }
        }
        if !same(rl10, jl10) {
            bad_log10 += 1;
            if shown < 10 {
                eprintln!("log10 beda x={:e} ({:016x}) js={:e} rust={:e}", x, x.to_bits(), jl10, rl10);
                shown += 1;
            }
        }
        if !same(x.ln(), jl) {
            std_log += 1;
        }
        if !same(x.log10(), jl10) {
            std_log10 += 1;
        }
    }
    println!(
        "math_check: kasus={} | port log beda={} | port log10 beda={} | (info) std ln beda={} | std log10 beda={}",
        n, bad_log, bad_log10, std_log, std_log10
    );
    let mut bad_fmt = 0usize;
    if let Some(fmt_path) = std::env::args().nth(2) {
        let txt = std::fs::read_to_string(&fmt_path).expect("baca fmt_ref");
        let mut m = 0usize;
        let mut shown = 0;
        for line in txt.lines() {
            let (h, s) = line.split_once(' ').expect("baris fmt_ref");
            let x = f64::from_bits(u64::from_str_radix(h, 16).expect("hex"));
            let r = port_features::jsnum::js_number_to_string(x);
            m += 1;
            if r != s {
                bad_fmt += 1;
                if shown < 10 {
                    eprintln!("toString beda bits={} js={} rust={}", h, s, r);
                    shown += 1;
                }
            }
        }
        println!("fmt_check: kasus={} | Number.toString beda={}", m, bad_fmt);
    }
    if bad_log + bad_log10 + bad_fmt > 0 {
        std::process::exit(1);
    }
}
