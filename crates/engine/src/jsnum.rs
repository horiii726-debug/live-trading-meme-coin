//! Semantik angka JavaScript (V8) yang dibutuhkan supaya hasil Rust identik bit-demi-bit
//! dengan v4engine.js / replay.js:
//! - `exp`, `log`  : port fdlibm persis seperti `base::ieee754::exp/log` di V8 (dipakai Math.exp/Math.log)
//! - `to_fixed_num`: `parseFloat(x.toFixed(k))` (pembulatan setengah-ke-atas atas nilai biner eksak)
//! - `js_max/js_min`: Math.max/Math.min (NaN menular, +0/-0 dibedakan)
//! - `to_number`   : `+str` / `Number(str)` (ToNumber untuk string)
//! - `num_to_string`: Number.prototype.toString() (dipakai JSON.stringify)
//! - `h32`         : FNV-1a 32-bit dengan Math.imul atas unit kode UTF-16

#[inline]
fn hi_word(x: f64) -> u32 {
    (x.to_bits() >> 32) as u32
}
#[inline]
fn lo_word(x: f64) -> u32 {
    x.to_bits() as u32
}
#[inline]
fn from_words(hi: u32, lo: u32) -> f64 {
    f64::from_bits(((hi as u64) << 32) | lo as u64)
}
#[inline]
fn set_high_word(x: f64, hi: u32) -> f64 {
    from_words(hi, lo_word(x))
}

/// Math.exp milik V8 (fdlibm e_exp.c versi FreeBSD, dengan kasus khusus exp(1) == E).
pub fn exp(x_in: f64) -> f64 {
    const ONE: f64 = 1.0;
    const HALF: [f64; 2] = [0.5, -0.5];
    const O_THRESHOLD: f64 = 7.09782712893383973096e+02;
    const U_THRESHOLD: f64 = -7.45133219101941108420e+02;
    const LN2HI: [f64; 2] = [6.93147180369123816490e-01, -6.93147180369123816490e-01];
    const LN2LO: [f64; 2] = [1.90821492927058770002e-10, -1.90821492927058770002e-10];
    const INVLN2: f64 = 1.44269504088896338700e+00;
    const P1: f64 = 1.66666666666666019037e-01;
    const P2: f64 = -2.77777777770155933842e-03;
    const P3: f64 = 6.61375632143793436117e-05;
    const P4: f64 = -1.65339022054652515390e-06;
    const P5: f64 = 4.13813679705723846039e-08;
    const E: f64 = 2.718281828459045;
    const HUGE: f64 = 1.0e+300;
    const TWOM1000: f64 = 9.33263618503218878990e-302;
    const TWO1023: f64 = 8.988465674311579539e307;

    let mut x = x_in;
    let mut hi = 0.0f64;
    let mut lo = 0.0f64;
    let mut k: i32 = 0;
    let mut hx = hi_word(x);
    let xsb = ((hx >> 31) & 1) as usize;
    hx &= 0x7FFF_FFFF;

    if hx >= 0x4086_2E42 {
        if hx >= 0x7FF0_0000 {
            if ((hx & 0xF_FFFF) | lo_word(x)) != 0 {
                return x + x; // NaN
            }
            return if xsb == 0 { x } else { 0.0 };
        }
        if x > O_THRESHOLD {
            return f64::INFINITY;
        }
        if x < U_THRESHOLD {
            return TWOM1000 * TWOM1000;
        }
    }

    if hx > 0x3FD6_2E42 {
        if hx < 0x3FF0_A2B2 {
            if x == 1.0 {
                return E;
            }
            hi = x - LN2HI[xsb];
            lo = LN2LO[xsb];
            k = 1 - xsb as i32 - xsb as i32;
        } else {
            k = (INVLN2 * x + HALF[xsb]) as i32; // (int) di C: potong ke arah nol
            let t = k as f64;
            hi = x - t * LN2HI[0];
            lo = t * LN2LO[0];
        }
        x = hi - lo;
    } else if hx < 0x3E30_0000 {
        if HUGE + x > ONE {
            return ONE + x;
        }
    } else {
        k = 0;
    }

    let t = x * x;
    let twopk = if k >= -1021 {
        from_words((0x3FF0_0000i32 + (k << 20)) as u32, 0)
    } else {
        from_words((0x3FF0_0000i32 + ((k + 1000) << 20)) as u32, 0)
    };
    let c = x - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    if k == 0 {
        return ONE - ((x * c) / (c - 2.0) - x);
    }
    let y = ONE - ((lo - (x * c) / (2.0 - c)) - hi);
    if k >= -1021 {
        if k == 1024 {
            return y * 2.0 * TWO1023;
        }
        return y * twopk;
    }
    y * twopk * TWOM1000
}

/// Math.log milik V8 (fdlibm e_log.c).
pub fn log(x_in: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01;
    const LN2_LO: f64 = 1.90821492927058770002e-10;
    const TWO54: f64 = 1.80143985094819840000e+16;
    const LG1: f64 = 6.666666666666735130e-01;
    const LG2: f64 = 3.999999999940941908e-01;
    const LG3: f64 = 2.857142874366239149e-01;
    const LG4: f64 = 2.222219843214978396e-01;
    const LG5: f64 = 1.818357216161805012e-01;
    const LG6: f64 = 1.531383769920937332e-01;
    const LG7: f64 = 1.479819860511658591e-01;

    let mut x = x_in;
    let mut hx = hi_word(x) as i32;
    let lx = lo_word(x);
    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        if ((hx & 0x7FFF_FFFF) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; // log(+-0) = -inf
        }
        if hx < 0 {
            return f64::NAN; // log(-#) = NaN
        }
        k -= 54;
        x *= TWO54;
        hx = hi_word(x) as i32;
    }
    if hx >= 0x7FF0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000F_FFFF;
    let i0 = (hx + 0x95F64) & 0x10_0000;
    x = set_high_word(x, (hx | (i0 ^ 0x3FF0_0000)) as u32);
    k += i0 >> 20;
    let f = x - 1.0;
    if (0x000F_FFFF & (2 + hx)) < 3 {
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            let dk = k as f64;
            return dk * LN2_HI + dk * LN2_LO;
        }
        let r = f * f * (0.5 - 0.33333333333333333 * f);
        if k == 0 {
            return f - r;
        }
        let dk = k as f64;
        return dk * LN2_HI - ((r - dk * LN2_LO) - f);
    }
    let s = f / (2.0 + f);
    let dk = k as f64;
    let z = s * s;
    let mut i = hx - 0x6147A;
    let w = z * z;
    let j = 0x6B851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            return f - (hfsq - s * (hfsq + r));
        }
        return dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f);
    }
    if k == 0 {
        return f - s * (f - r);
    }
    dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
}

/// Math.max(a, b): NaN menular; max(-0, +0) = +0.
#[inline]
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == b {
        // satu-satunya kasus a==b dengan bit beda: +0 vs -0
        if a == 0.0 && (a.is_sign_negative() != b.is_sign_negative()) {
            return 0.0;
        }
        return a;
    }
    if a > b {
        a
    } else {
        b
    }
}

/// Math.min(a, b): NaN menular; min(-0, +0) = -0.
#[inline]
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a == b {
        if a == 0.0 && (a.is_sign_negative() != b.is_sign_negative()) {
            return -0.0;
        }
        return a;
    }
    if a < b {
        a
    } else {
        b
    }
}

/// Math.round: bilangan bulat terdekat, seri dibulatkan ke arah +Infinity.
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    if r == 0.0 && x < 0.0 {
        -0.0
    } else {
        r
    }
}

/// `parseFloat(x.toFixed(k))` (k <= 20). Perhitungan eksak atas nilai biner x, seri -> n yang lebih besar
/// (sama dengan spesifikasi Number.prototype.toFixed), lalu string desimal diurai ulang (dibulatkan benar).
pub fn to_fixed_num(x: f64, k: u32) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.abs() >= 1e21 {
        // toFixed mengembalikan ToString(x); parseFloat(ToString(x)) == x (termasuk +-Infinity)
        return x;
    }
    let neg = x < 0.0;
    let a = x.abs();
    let bits = a.to_bits();
    let e_field = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (mant, exp2) = if e_field == 0 {
        (frac, -1074i32)
    } else {
        (frac | (1u64 << 52), e_field - 1075)
    };
    let p10: u128 = 10u128.pow(k);
    let num: u128 = (mant as u128) * p10;
    let n: u128 = if exp2 >= 0 {
        num << exp2 // a < 1e21 < 2^70 dan 10^k kecil -> muat di u128 untuk k <= 4
    } else {
        let sh = (-exp2) as u32;
        if sh >= 128 {
            0
        } else {
            let q = num >> sh;
            let rem = num - (q << sh);
            let half = 1u128 << (sh - 1);
            if rem >= half {
                q + 1
            } else {
                q
            }
        }
    };
    let s = if neg {
        format!("-{}e-{}", n, k)
    } else {
        format!("{}e-{}", n, k)
    };
    s.parse::<f64>().expect("to_fixed_num: parse")
}

/// Fungsi `fx` di v4engine.js: parseFloat(x.toFixed(k)), non-finite -> -1 (meniru CSV + fillna(-1)).
#[inline]
pub fn fx(x: f64, k: u32) -> f64 {
    let v = to_fixed_num(x, k);
    if v.is_finite() {
        v
    } else {
        -1.0
    }
}

/// Fungsi `num` di v4engine.js.
#[inline]
pub fn num(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        -1.0
    }
}

fn is_js_whitespace(c: char) -> bool {
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

/// String.prototype.trim()
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

fn parse_radix_exact(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    // nilai eksak lalu dibulatkan ke double terdekat (seri ke genap)
    let mut v = num_bigint::BigUint::from(0u32);
    for c in digits.chars() {
        match c.to_digit(radix) {
            Some(d) => v = v * radix + d,
            None => return f64::NAN,
        }
    }
    num_traits::ToPrimitive::to_f64(&v).unwrap_or(f64::NAN)
}

/// ToNumber(string) — `+str` / `Number(str)` di JS.
pub fn to_number(s: &str) -> f64 {
    // jalur cepat: [-]digit+ dengan <= 38 digit (bentuk dominan di CSV: sqrt_price, liquidity, amount).
    // Nilai bulat eksak di u128 lalu `as f64` (dibulatkan ke terdekat, seri ke genap) = hasil ToNumber.
    // (parse::<f64> bawaan untuk >19 digit sering lewat jalur lambat ~1 mikrodetik)
    let b = s.as_bytes();
    let (neg, body) = if !b.is_empty() && b[0] == b'-' { (true, &b[1..]) } else { (false, b) };
    if !body.is_empty() && body.len() <= 38 && body.iter().all(|c| c.is_ascii_digit()) {
        let mut v: u128 = 0;
        for &c in body {
            v = v * 10 + (c - b'0') as u128;
        }
        let f = v as f64;
        return if neg { -f } else { f };
    }
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let b = t.as_bytes();
    if b.len() >= 2 && b[0] == b'0' {
        match b[1] {
            b'x' | b'X' => return parse_radix_exact(&t[2..], 16),
            b'o' | b'O' => return parse_radix_exact(&t[2..], 8),
            b'b' | b'B' => return parse_radix_exact(&t[2..], 2),
            _ => {}
        }
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // StrDecimalLiteral: [+-] (digits [. digits?] | . digits) ([eE] [+-] digits)?
    let mut i = 0usize;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut nd = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        nd += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            nd += 1;
        }
    }
    if nd == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let mut ne = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            ne += 1;
        }
        if ne == 0 {
            return f64::NAN;
        }
    }
    if i != b.len() {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// Number.prototype.toString() (radix 10), dipakai juga oleh JSON.stringify untuk angka finite.
/// Digit terpendek diambil dari Ryu (terpendek, terdekat, seri -> genap) = aturan ECMAScript
/// ("if there are two such possible values of n, choose the one that is even").
/// Catatan: format `{:e}` bawaan Rust TIDAK dipakai karena seri dibulatkan ke atas (beda dengan V8).
pub fn num_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x == 0.0 {
        return "0".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    let neg = x < 0.0;
    let mut buf = ryu::Buffer::new();
    let rs = buf.format_finite(x.abs());
    let (mant, e10) = match rs.find(|c| c == 'e' || c == 'E') {
        Some(k) => (&rs[..k], rs[k + 1..].parse::<i32>().expect("eksponen ryu")),
        None => (rs, 0),
    };
    let (ip, fp) = match mant.find('.') {
        Some(k) => (&mant[..k], &mant[k + 1..]),
        None => (mant, ""),
    };
    // nilai = 0.D x 10^n
    let mut digits: String = format!("{}{}", ip, fp);
    let mut n: i32 = ip.len() as i32 + e10;
    let lead = digits.len() - digits.trim_start_matches('0').len();
    digits = digits[lead..].to_string();
    n -= lead as i32;
    let t = digits.trim_end_matches('0').len();
    digits.truncate(t);
    let k = digits.len() as i32;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let e = n - 1;
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.abs().to_string());
    }
    out
}

/// Number(BigInt): dibulatkan ke double terdekat, seri ke genap (eksak untuk panjang berapa pun).
pub fn bigint_to_f64(b: &num_bigint::BigInt) -> f64 {
    let (sign, d) = b.to_u64_digits(); // little-endian, ternormalisasi
    let mag = if d.is_empty() {
        0.0
    } else if d.len() == 1 {
        d[0] as f64 // konversi u64 -> f64 Rust dibulatkan benar (seri ke genap)
    } else {
        let n = d.len();
        let top = d[n - 1];
        let lz = top.leading_zeros();
        let bits = (n as i64) * 64 - lz as i64;
        let (hi, rest_second) = if lz == 0 {
            (top, d[n - 2])
        } else {
            ((top << lz) | (d[n - 2] >> (64 - lz)), d[n - 2] & ((1u64 << (64 - lz)) - 1))
        };
        let sticky = rest_second != 0 || d[..n - 2].iter().any(|&x| x != 0);
        let m = hi | (sticky as u64); // round-to-odd lalu dibulatkan sekali ke 53 bit
        let e = bits - 64;
        if e > 1023 {
            f64::INFINITY
        } else {
            (m as f64) * f64::from_bits(((e + 1023) as u64) << 52)
        }
    };
    if sign == num_bigint::Sign::Minus {
        -mag
    } else {
        mag
    }
}

/// Number.prototype.toExponential(f) — hanya untuk pesan log.
pub fn to_exponential(x: f64, f: usize) -> String {
    if !x.is_finite() {
        return num_to_string(x);
    }
    let s = format!("{:.*e}", f, x);
    let (m, e) = s.split_once('e').unwrap();
    let ei: i32 = e.parse().unwrap();
    format!("{}e{}{}", m, if ei >= 0 { "+" } else { "-" }, ei.abs())
}

/// h32 di v4engine.js: FNV-1a 32-bit atas unit kode UTF-16 (charCodeAt) dengan Math.imul, hasil / 2^32.
pub fn h32(s: &str) -> f64 {
    let mut h: u32 = 2_166_136_261;
    for c in s.encode_utf16() {
        h ^= c as u32;
        h = h.wrapping_mul(16_777_619);
    }
    (h as f64) / 4_294_967_296.0
}

/// JSON.stringify untuk string.
pub fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// JSON.stringify untuk angka: non-finite -> null.
pub fn json_num(x: f64, out: &mut String) {
    if x.is_finite() {
        out.push_str(&num_to_string(x));
    } else {
        out.push_str("null");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn to_fixed_ties() {
        assert_eq!(to_fixed_num(2.5, 0), 3.0);
        assert_eq!(to_fixed_num(-2.5, 0), -3.0);
        assert_eq!(to_fixed_num(0.25, 1), 0.3);
        assert_eq!(to_fixed_num(1.005, 2), 1.0);
        assert_eq!(to_fixed_num(1.45, 1), 1.4);
    }
    #[test]
    fn to_string() {
        assert_eq!(num_to_string(1e21), "1e+21");
        assert_eq!(num_to_string(1e-7), "1e-7");
        assert_eq!(num_to_string(0.000001), "0.000001");
        assert_eq!(num_to_string(123.456), "123.456");
        assert_eq!(num_to_string(5.0), "5");
        assert_eq!(num_to_string(-1.5e-9), "-1.5e-9");
    }
}
