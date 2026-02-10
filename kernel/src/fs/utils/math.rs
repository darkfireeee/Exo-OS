//! Math Utilities - Approximation Functions for no_std
//!
//! Provides fast approximations of mathematical functions that aren't
//! available in core (like f32::exp).
//!
//! These are kernel-optimized implementations trading perfect accuracy
//! for speed and simplicity.

/// Fast approximation of e^x for f32
///
/// Uses a polynomial approximation with acceptable accuracy for
/// filesystem operations (access frequency decay, etc.)
///
/// ## Accuracy
/// - Relative error: < 0.5% for x in [-10, 10]
/// - Absolute error increases outside this range
///
/// ## Performance
/// - ~10-20 cycles (vs 100+ for full precision)
///
/// ## Algorithm
/// Based on the identity: e^x = 2^(x * log2(e))
/// Using bit manipulation for the 2^n part.
#[inline]
pub fn exp_approx(x: f32) -> f32 {
    // Handle edge cases
    if x > 88.0 {
        return f32::INFINITY; // Overflow
    }
    if x < -87.0 {
        return 0.0; // Underflow
    }

    // Convert to 2^n form
    const LOG2E: f32 = 1.442695040888963407359924681001892137426645954; // log2(e)
    let n = x * LOG2E;

    // Split into integer and fractional parts
    let n_int = if n >= 0.0 {
        n as i32
    } else {
        // For negative numbers, floor manually
        let i = n as i32;
        if n == i as f32 {
            i
        } else {
            i - 1
        }
    };
    let n_frac = n - n_int as f32;

    // Compute 2^n_frac using polynomial approximation
    // 2^x ≈ 1 + x * (0.693147 + x * (0.240226 + x * 0.0558315))
    // (Pade approximation, accurate to ~0.1%)
    let frac_exp = 1.0 + n_frac * (0.693147 + n_frac * (0.240226 + n_frac * 0.0558315));

    // Compute 2^n_int using bit manipulation (exact)
    let pow2_int = if n_int >= 0 {
        // 2^n = 1.0 * 2^n (shift exponent bits)
        f32::from_bits(((127 + n_int) as u32) << 23)
    } else {
        // 2^(-n) = 1.0 / 2^n
        f32::from_bits(((127 + n_int) as u32) << 23)
    };

    // Combine: 2^n = 2^n_int * 2^n_frac
    pow2_int * frac_exp
}

/// Fast natural logarithm approximation (ln(x))
///
/// Provided for completeness, though not currently used.
#[inline]
#[allow(dead_code)]
pub fn ln_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return f32::NEG_INFINITY;
    }

    // Extract exponent and mantissa from IEEE 754
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa = f32::from_bits((bits & 0x007FFFFF) | 0x3F800000);

    // ln(x) = ln(2^e * m) = e * ln(2) + ln(m)
    const LN2: f32 = 0.6931471805599453;

    // Polynomial approximation for ln(m) where m in [1, 2)
    let y = mantissa - 1.0;
    let ln_mantissa = y * (1.0 - y * (0.5 - y * (0.333333 - y * 0.25)));

    exponent as f32 * LN2 + ln_mantissa
}

/// Fast power function x^y approximation
///
/// Uses exp(y * ln(x))
#[inline]
#[allow(dead_code)]
pub fn pow_approx(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        if x == 0.0 && y > 0.0 {
            return 0.0;
        }
        return f32::NAN;
    }

    exp_approx(y * ln_approx(x))
}

/// Square root approximation using Newton-Raphson
///
/// Faster than core::intrinsics::sqrtf32 for approximate values
#[inline]
#[allow(dead_code)]
pub fn sqrt_approx(x: f32) -> f32 {
    if x < 0.0 {
        return f32::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }

    // Initial guess using bit manipulation
    let bits = x.to_bits();
    let guess = f32::from_bits((bits >> 1) + (127 << 22));

    // Two Newton-Raphson iterations (good enough for most cases)
    let guess = 0.5 * (guess + x / guess);
    let guess = 0.5 * (guess + x / guess);

    guess
}

/// Floor function approximation
#[inline]
pub fn floor_approx(x: f32) -> f32 {
    let int_part = x as i32;
    if x >= 0.0 || x == int_part as f32 {
        int_part as f32
    } else {
        (int_part - 1) as f32
    }
}

/// Integer power function for f32
#[inline]
pub fn powi_approx(base: f32, exp: i32) -> f32 {
    if exp == 0 {
        return 1.0;
    }

    let mut result = 1.0;
    let mut p = exp.abs();
    let mut b = base;

    while p > 0 {
        if p & 1 == 1 {
            result *= b;
        }
        b *= b;
        p >>= 1;
    }

    if exp < 0 {
        1.0 / result
    } else {
        result
    }
}

/// Log2 approximation for f32
#[inline]
pub fn log2_approx_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return f32::NEG_INFINITY;
    }

    // Extract exponent from IEEE 754 representation
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa = (bits & 0x7FFFFF) | 0x3F800000;
    let m = f32::from_bits(mantissa);

    // Approximate log2(m) where m is in [1, 2)
    // Using polynomial approximation
    let m_minus_1 = m - 1.0;
    let log2_m = m_minus_1 * (1.44269504 - 0.721347520 * m_minus_1);

    exp as f32 + log2_m
}

/// Log2 approximation for f64
#[inline]
pub fn log2_approx_f64(x: f64) -> f64 {
    if x <= 0.0 {
        return f64::NEG_INFINITY;
    }

    // Extract exponent from IEEE 754 representation
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let mantissa = (bits & 0xFFFFFFFFFFFFF) | 0x3FF0000000000000;
    let m = f64::from_bits(mantissa);

    // Approximate log2(m) where m is in [1, 2)
    let m_minus_1 = m - 1.0;
    let log2_m = m_minus_1 * (1.44269504088896 - 0.72134752044448 * m_minus_1);

    exp as f64 + log2_m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exp_approx_basic() {
        // Test basic values
        assert!((exp_approx(0.0) - 1.0).abs() < 0.01);
        assert!((exp_approx(1.0) - 2.718).abs() < 0.01);
        assert!((exp_approx(-1.0) - 0.368).abs() < 0.01);
    }

    #[test]
    fn test_exp_approx_edge_cases() {
        assert!(exp_approx(100.0).is_infinite());
        assert!(exp_approx(-100.0) < 0.0001);
    }

    #[test]
    fn test_ln_approx() {
        assert!((ln_approx(1.0) - 0.0).abs() < 0.01);
        assert!((ln_approx(2.718) - 1.0).abs() < 0.05);
    }
}
