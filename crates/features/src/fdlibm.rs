//! `Math.log` dan `Math.log10` persis seperti V8 13.6 (Node 24).
//!
//! V8 tidak memakai libm sistem untuk `Math.log`/`Math.log10`; ia memakai port fdlibm
//! di `src/base/ieee754.cc` (`ieee754::log`, `ieee754::log10`). `f64::ln` di Rust memanggil
//! libm platform (UCRT di Windows) yang boleh beda 1 ulp. Satu ulp pada `log(p[i]) - log(p[i-1])`
//! (pembatalan) bisa membesar menjadi selisih relatif jauh di atas 1e-9 pada fitur price impact,
//! jadi kedua fungsi di-port baris per baris dari sumber V8 13.6.233.17.
//! Kesamaan bit diuji oleh binary `math_check` terhadap Node.

const TWO54: f64 = 1.80143985094819840000e+16; // 0x43500000 00000000

#[inline]
fn high_word(x: f64) -> i32 {
    (x.to_bits() >> 32) as u32 as i32
}

#[inline]
fn low_word(x: f64) -> u32 {
    x.to_bits() as u32
}

#[inline]
fn set_high_word(x: f64, hi: u32) -> f64 {
    f64::from_bits((x.to_bits() & 0x0000_0000_FFFF_FFFF) | (u64::from(hi) << 32))
}

#[inline]
fn set_low_word(x: f64, lo: u32) -> f64 {
    f64::from_bits((x.to_bits() & 0xFFFF_FFFF_0000_0000) | u64::from(lo))
}

/// `Math.log(x)` versi V8 (`base::ieee754::log`, fdlibm e_log.c).
pub fn log(x: f64) -> f64 {
    const LN2_HI: f64 = 6.93147180369123816490e-01; /* 3fe62e42 fee00000 */
    const LN2_LO: f64 = 1.90821492927058770002e-10; /* 3dea39ef 35793c76 */
    const LG1: f64 = 6.666666666666735130e-01; /* 3FE55555 55555593 */
    const LG2: f64 = 3.999999999940941908e-01; /* 3FD99999 9997FA04 */
    const LG3: f64 = 2.857142874366239149e-01; /* 3FD24924 94229359 */
    const LG4: f64 = 2.222219843214978396e-01; /* 3FCC71C5 1D8E78AF */
    const LG5: f64 = 1.818357216161805012e-01; /* 3FC74664 96CB03DE */
    const LG6: f64 = 1.531383769920937332e-01; /* 3FC39A09 D078C69F */
    const LG7: f64 = 1.479819860511658591e-01; /* 3FC2F112 DF3E5244 */

    let mut x = x;
    let mut hx = high_word(x);
    let lx = low_word(x);

    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        /* x < 2**-1022  */
        if ((hx & 0x7FFF_FFFF) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; /* log(+-0)=-inf */
        }
        if hx < 0 {
            return f64::NAN; /* log(-#) = NaN */
        }
        k -= 54;
        x *= TWO54; /* subnormal number, scale up x */
        hx = high_word(x);
    }
    if hx >= 0x7FF0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000F_FFFF;
    let i0 = (hx + 0x95F64) & 0x10_0000;
    x = set_high_word(x, (hx | (i0 ^ 0x3FF0_0000)) as u32); /* normalize x or x/2 */
    k += i0 >> 20;
    let f = x - 1.0;
    if (0x000F_FFFF & (2 + hx)) < 3 {
        /* -2**-20 <= f < 2**-20 */
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            let dk = f64::from(k);
            return dk * LN2_HI + dk * LN2_LO;
        }
        let r = f * f * (0.5 - 0.33333333333333333 * f);
        if k == 0 {
            return f - r;
        }
        let dk = f64::from(k);
        return dk * LN2_HI - ((r - dk * LN2_LO) - f);
    }
    let s = f / (2.0 + f);
    let dk = f64::from(k);
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
            f - (hfsq - s * (hfsq + r))
        } else {
            dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f)
        }
    } else if k == 0 {
        f - s * (f - r)
    } else {
        dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
    }
}

/// `Math.log10(x)` versi V8 (`base::ieee754::log10`).
pub fn log10(x: f64) -> f64 {
    const IVLN10: f64 = 4.34294481903251816668e-01;
    const LOG10_2HI: f64 = 3.01029995663611771306e-01; /* 0x3FD34413, 0x509F6000 */
    const LOG10_2LO: f64 = 3.69423907715893078616e-13; /* 0x3D59FEF3, 0x11F12B36 */

    let mut x = x;
    let mut hx = high_word(x);
    let mut lx = low_word(x);

    let mut k: i32 = 0;
    if hx < 0x0010_0000 {
        /* x < 2**-1022  */
        if ((hx & 0x7FFF_FFFF) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; /* log(+-0)=-inf */
        }
        if hx < 0 {
            return f64::NAN; /* log(-#) = NaN */
        }
        k -= 54;
        x *= TWO54; /* subnormal number, scale up x */
        hx = high_word(x);
        lx = low_word(x);
    }
    if hx >= 0x7FF0_0000 {
        return x + x;
    }
    if hx == 0x3FF0_0000 && lx == 0 {
        return 0.0; /* log(1) = +0 */
    }
    k += (hx >> 20) - 1023;

    // C: i = (k & 0x80000000) >> 31;  (aritmetika unsigned -> 0 atau 1)
    let i = (((k as u32) & 0x8000_0000) >> 31) as i32;
    hx = (hx & 0x000F_FFFF) | ((0x3FF - i) << 20);
    let y = f64::from(k + i);
    x = set_high_word(x, hx as u32);
    x = set_low_word(x, lx);

    let z = y * LOG10_2LO + IVLN10 * log(x);
    z + y * LOG10_2HI
}
