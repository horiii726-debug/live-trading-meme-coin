//! Uji fungsi semantik JS (jsnum) terhadap keluaran Node yang dibuat uji/jsmath_gen.js.
//! Pakai: jsmath_selftest <jsmath_cases.txt>
//! Format tiap baris (hex bit IEEE-754): op \t input_hex \t [arg] \t expected (hex atau string)
use port_engine::jsnum;

fn hx(s: &str) -> f64 { f64::from_bits(u64::from_str_radix(s, 16).expect("hex")) }
fn hb(x: f64) -> String { format!("{:016x}", x.to_bits()) }

fn main() {
    let f = std::env::args().nth(1).expect("file kasus");
    let txt = std::fs::read_to_string(&f).expect("baca");
    let mut n = std::collections::BTreeMap::<String, (usize, usize)>::new();
    let mut shown = 0;
    for line in txt.lines() {
        let c: Vec<&str> = line.split('\t').collect();
        let op = c[0];
        let (got, exp) = match op {
            "exp" => (hb(jsnum::exp(hx(c[1]))), c[2].to_string()),
            "log" => (hb(jsnum::log(hx(c[1]))), c[2].to_string()),
            "fix" => (hb(jsnum::to_fixed_num(hx(c[1]), c[2].parse().unwrap())), c[3].to_string()),
            "str" => (jsnum::num_to_string(hx(c[1])), c[2].to_string()),
            "num" => (hb(jsnum::to_number(c[1])), c[2].to_string()),
            "big" => {
                let b = port_engine::engine::parse_bigint(c[1]).unwrap();
                (hb(jsnum::bigint_to_f64(&b)), c[2].to_string())
            }
            "h32" => (hb(jsnum::h32(c[1])), c[2].to_string()),
            "max" => (hb(jsnum::js_max(hx(c[1]), hx(c[2]))), c[3].to_string()),
            "min" => (hb(jsnum::js_min(hx(c[1]), hx(c[2]))), c[3].to_string()),
            "rnd" => (hb(jsnum::js_round(hx(c[1]))), c[2].to_string()),
            _ => continue,
        };
        let e = n.entry(op.to_string()).or_insert((0, 0));
        e.0 += 1;
        // NaN: payload bit tidak teramati di JS (selain lewat typed array) -> semua NaN dianggap sama
        let is_nan_hex = |h: &str| u64::from_str_radix(h, 16).map(|b| f64::from_bits(b).is_nan()).unwrap_or(false);
        let same = got == exp || (op != "str" && is_nan_hex(&got) && is_nan_hex(&exp));
        if same { e.1 += 1; } else if shown < 20 { shown += 1; eprintln!("BEDA {} -> rust {} js {}", line, got, exp); }
    }
    let mut all = true;
    for (op, (t, ok)) in &n {
        println!("{:4} {:>9}/{:<9} {}", op, ok, t, if ok == t { "identik" } else { "BEDA" });
        if ok != t { all = false; }
    }
    std::process::exit(if all { 0 } else { 1 });
}
