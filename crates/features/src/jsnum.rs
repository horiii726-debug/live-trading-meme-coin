//! Semantik angka/string JavaScript yang dibutuhkan supaya hasil identik dengan V8:
//! `Number(string)`, `Math.max`, pembanding sort `(a,b)=>a-b`, perbandingan string JS
//! (unit kode UTF-16), `String.prototype.trim`, dan `Number.prototype.toString`.

use std::cmp::Ordering;

/// Spasi menurut JS (WhiteSpace + LineTerminator). Beda dengan `char::is_whitespace`:
/// JS memasukkan U+FEFF dan TIDAK memasukkan U+0085.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `String.prototype.toLowerCase` untuk teks yang dipakai di sini (alamat hex ASCII).
/// Untuk karakter non-ASCII, pemetaan Unicode Rust dipakai (sama dengan JS untuk
/// hampir semua karakter; alamat 0x.. selalu ASCII).
pub fn js_lower(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        s.to_lowercase()
    }
}

fn all_digits(s: &str, radix: u32) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_digit(radix))
}

/// Validasi StrUnsignedDecimalLiteral (tanpa "Infinity"): digits [. digits?] [exp] | . digits [exp]
fn is_unsigned_decimal(s: &str) -> bool {
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(p) => (&s[..p], Some(&s[p + 1..])),
        None => (s, None),
    };
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if !all_digits(e, 10) {
            return false;
        }
    }
    match mant.find('.') {
        Some(p) => {
            let (a, b) = (&mant[..p], &mant[p + 1..]);
            let a_ok = a.is_empty() || all_digits(a, 10);
            let b_ok = b.is_empty() || all_digits(b, 10);
            a_ok && b_ok && !(a.is_empty() && b.is_empty())
        }
        None => all_digits(mant, 10),
    }
}

/// Bilangan bulat basis 2/8/16 -> f64 dibulatkan ke terdekat (seri -> genap), seperti V8.
fn parse_pow2_radix(digits: &str, bits_per_digit: u32) -> f64 {
    // kumpulkan bit dari digit paling signifikan
    let mut bits: Vec<bool> = Vec::with_capacity(digits.len() * bits_per_digit as usize);
    for c in digits.chars() {
        let d = c.to_digit(1 << bits_per_digit).unwrap_or(0);
        for b in (0..bits_per_digit).rev() {
            bits.push((d >> b) & 1 == 1);
        }
    }
    let first = match bits.iter().position(|&b| b) {
        Some(p) => p,
        None => return 0.0,
    };
    let bits = &bits[first..];
    let nbits = bits.len();
    if nbits <= 53 {
        let mut m: u64 = 0;
        for &b in bits {
            m = (m << 1) | u64::from(b);
        }
        return m as f64;
    }
    let mut m: u64 = 0;
    for &b in &bits[..53] {
        m = (m << 1) | u64::from(b);
    }
    let round = bits[53];
    let sticky = bits[54..].iter().any(|&b| b);
    if round && (sticky || (m & 1) == 1) {
        m += 1;
    }
    let shift = (nbits - 53) as i32;
    // m <= 2^53, dikali 2^shift (eksak, kecuali overflow -> Infinity seperti JS)
    (m as f64) * 2f64.powi(shift)
}

/// `Number(string)` / unary `+string` menurut ECMAScript StringToNumber.
pub fn js_string_to_number(s: &str) -> f64 {
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let rest = &t[2..];
        match b[1] {
            b'x' | b'X' => return if all_digits(rest, 16) { parse_pow2_radix(rest, 4) } else { f64::NAN },
            b'o' | b'O' => return if all_digits(rest, 8) { parse_pow2_radix(rest, 3) } else { f64::NAN },
            b'b' | b'B' => return if all_digits(rest, 2) { parse_pow2_radix(rest, 1) } else { f64::NAN },
            _ => {}
        }
    }
    let (neg, body) = match b[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    let v = if body == "Infinity" {
        f64::INFINITY
    } else if is_unsigned_decimal(body) {
        // Parser std Rust membulatkan IEEE ke terdekat (benar untuk semua panjang digit).
        match body.parse::<f64>() {
            Ok(v) => v,
            Err(_) => return f64::NAN,
        }
    } else {
        return f64::NAN;
    };
    if neg {
        -v
    } else {
        v
    }
}

/// `Math.max(a, b)`: NaN menular; +0 dianggap lebih besar dari -0.
#[inline]
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_positive() || b.is_sign_positive() { 0.0 } else { -0.0 };
    }
    if a > b {
        a
    } else {
        b
    }
}

/// `Math.max(...xs)`.
pub fn js_max_all<I: IntoIterator<Item = f64>>(xs: I) -> f64 {
    xs.into_iter().fold(f64::NEG_INFINITY, js_max)
}

/// Nilai kebenaran JS untuk angka: false untuk 0, -0, NaN.
#[inline]
pub fn js_truthy(v: f64) -> bool {
    !(v == 0.0 || v.is_nan())
}

/// Ubah hasil pembanding sort JS (angka) menjadi Ordering. V8 hanya melihat tanda;
/// NaN dan 0 = sama.
#[inline]
pub fn cmp_from_js(r: f64) -> Ordering {
    if r < 0.0 {
        Ordering::Less
    } else if r > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// Perbandingan string JS (`<`, sort bawaan): urutan unit kode UTF-16.
pub fn js_str_cmp(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        return a.as_bytes().cmp(b.as_bytes());
    }
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Pecah keluaran `{:e}` / `{:.Ne}` Rust menjadi (digit mantisa, eksponen desimal).
fn split_exp(s: &str) -> (String, i32) {
    let (mant, exp) = s.split_once('e').expect("format eksponensial selalu punya 'e'");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    (digits, exp.parse().expect("eksponen valid"))
}

/// Tambah satu pada digit terakhir string desimal; None jika terjadi carry keluar (99 -> 100).
fn incr_digits(d: &str) -> Option<String> {
    let mut b = d.as_bytes().to_vec();
    for i in (0..b.len()).rev() {
        if b[i] == b'9' {
            b[i] = b'0';
        } else {
            b[i] += 1;
            return String::from_utf8(b).ok();
        }
    }
    None
}

/// Digit terpendek yang round-trip untuk v > 0 (hingga), dengan aturan ECMAScript
/// Number::toString: jika dua kandidat k-digit sama dekat dengan nilai eksak, pilih yang
/// genap (V8 bignum-dtoa: "round towards even"). Rust memilih yang lain pada seri
/// semacam itu (mis. 154853132016341.125 -> Rust "...341.13", V8 "...341.12"),
/// jadi kasus seri diperiksa secara eksak di sini.
/// Mengembalikan (digit, n) dengan nilai = 0.digit x 10^n.
fn shortest_digits(v: f64) -> (String, i32) {
    let (digits, exp) = split_exp(&format!("{:e}", v));
    let n = exp + 1;
    let k = digits.len();
    let last = digits.as_bytes()[k - 1] - b'0';
    if last % 2 == 0 {
        // Kalau ini hasil seri, Rust sudah memilih yang genap; kalau bukan seri, ia yang terdekat.
        return (digits, n);
    }
    // Periksa seri: ekspansi eksak v harus berbentuk P5000... dengan P = k digit.
    let probe = |prec: usize| -> Option<String> {
        let (d, e) = split_exp(&format!("{:.*e}", prec, v));
        if e + 1 != n {
            return None;
        }
        let (p, rest) = d.split_at(k);
        let rb = rest.as_bytes();
        if rb[0] == b'5' && rb[1..].iter().all(|&c| c == b'0') {
            Some(p.to_string())
        } else {
            None
        }
    };
    // Cek murah dulu (k+41 digit), lalu konfirmasi eksak: ekspansi desimal double paling
    // panjang 767 digit signifikan, jadi 800 digit = nilai eksak.
    let p = match probe(k + 40).and_then(|_| probe(800)) {
        Some(p) => p,
        None => return (digits, n),
    };
    let even = if (p.as_bytes()[k - 1] - b'0') % 2 == 0 { Some(p) } else { incr_digits(&p) };
    if let Some(cand) = even {
        if cand != digits {
            let back: f64 = format!("{}e{}", cand, n - k as i32).parse().unwrap_or(f64::NAN);
            if back.to_bits() == v.to_bits() {
                return (cand, n);
            }
        }
    }
    (digits, n)
}

/// `Number.prototype.toString()` / angka di `JSON.stringify` (untuk nilai hingga).
/// Digit terpendek yang round-trip diambil dari format `{:e}` Rust (algoritma shortest
/// Grisu/Dragon), lalu disusun menurut aturan Number::toString ECMAScript.
pub fn js_number_to_string(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    if v < 0.0 {
        return format!("-{}", js_number_to_string(-v));
    }
    let (digits, n) = shortest_digits(v);
    let k = digits.len() as i32;
    if k <= n && n <= 21 {
        let mut s = digits;
        for _ in 0..(n - k) {
            s.push('0');
        }
        s
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        let mut s = String::from("0.");
        for _ in 0..(-n) {
            s.push('0');
        }
        s.push_str(&digits);
        s
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{}e{}{}", digits, sign, e.abs())
        } else {
            format!("{}.{}e{}{}", &digits[..1], &digits[1..], sign, e.abs())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_number() {
        assert_eq!(js_string_to_number("123"), 123.0);
        assert_eq!(js_string_to_number(" -5 "), -5.0);
        assert_eq!(js_string_to_number(""), 0.0);
        assert!(js_string_to_number("abc").is_nan());
        assert!(js_string_to_number("inf").is_nan());
        assert_eq!(js_string_to_number("Infinity"), f64::INFINITY);
        assert_eq!(js_string_to_number("0x10"), 16.0);
        assert_eq!(js_string_to_number(".5"), 0.5);
        assert_eq!(js_string_to_number("5."), 5.0);
        assert!(js_string_to_number(".").is_nan());
        assert!(js_string_to_number("-0x10").is_nan());
        assert!(js_string_to_number("-0").is_sign_negative());
        assert_eq!(js_string_to_number("1e3"), 1000.0);
    }

    #[test]
    fn to_string() {
        assert_eq!(js_number_to_string(1.0), "1");
        assert_eq!(js_number_to_string(0.1), "0.1");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1e20), "100000000000000000000");
        assert_eq!(js_number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(js_number_to_string(0.000001), "0.000001");
        assert_eq!(js_number_to_string(123.456), "123.456");
        assert_eq!(js_number_to_string(-2.5), "-2.5");
        assert_eq!(js_number_to_string(5e-324), "5e-324");
        // seri: nilai eksak ...341.125 / ...075.25 -> V8 memilih digit genap
        assert_eq!(js_number_to_string(154853132016341.125), "154853132016341.12");
        assert_eq!(js_number_to_string(615344021164075.25), "615344021164075.2");
        assert_eq!(js_number_to_string(1130714070575159.25), "1130714070575159.2");
        assert_eq!(js_number_to_string(21212274386785.8125), "21212274386785.812");
    }

    #[test]
    fn max() {
        assert!(js_max(f64::NAN, 1.0).is_nan());
        assert!(js_max(-0.0, 0.0).is_sign_positive());
        assert_eq!(js_max_all([1.0, 3.0, 2.0]), 3.0);
    }
}
