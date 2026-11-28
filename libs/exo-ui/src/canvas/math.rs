//! Fast `#![no_std]` integer/float trigonometry and root helpers for 2D vector graphics.

#[inline]
pub(crate) fn isqrt_u64(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = 1u64 << ((64 - n.leading_zeros()).div_ceil(2));
    loop {
        let next = (x + n / x) >> 1;
        if next >= x {
            return x;
        }
        x = next;
    }
}

#[inline(always)]
pub fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    #[cfg(all(target_arch = "x86_64", target_feature = "sse"))]
    unsafe {
        use core::arch::x86_64::{_mm_cvtss_f32, _mm_set_ss, _mm_sqrt_ss};
        _mm_cvtss_f32(_mm_sqrt_ss(_mm_set_ss(x)))
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "sse")))]
    {
        let mut g = f32::from_bits((x.to_bits() >> 1) + 0x1fc0_0000);
        g = 0.5 * (g + x / g);
        g = 0.5 * (g + x / g);
        g = 0.5 * (g + x / g);
        g
    }
}

#[inline]
pub fn sin_cos_f32(rad: f32) -> (f32, f32) {
    const PI: f32 = core::f32::consts::PI;
    const TWO_PI: f32 = 2.0 * PI;
    let mut a = rad % TWO_PI;
    if a > PI {
        a -= TWO_PI;
    } else if a < -PI {
        a += TWO_PI;
    }
    let (s_sign, c_sign, x) = if a > PI * 0.5 {
        (1.0f32, -1.0f32, PI - a)
    } else if a < -PI * 0.5 {
        (-1.0f32, -1.0f32, PI + a)
    } else {
        (1.0f32, 1.0f32, a)
    };
    let x2 = x * x;
    let s = x * (1.0 - x2 * (1.0 / 6.0 - x2 * (1.0 / 120.0 - x2 / 5040.0)));
    let c = 1.0 - x2 * (0.5 - x2 * (1.0 / 24.0 - x2 / 720.0));
    (s_sign * s, c_sign * c)
}

#[inline]
pub fn atan2_f32(y: f32, x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    const HALF_PI: f32 = core::f32::consts::FRAC_PI_2;
    if x == 0.0 {
        return if y > 0.0 { HALF_PI } else if y < 0.0 { -HALF_PI } else { 0.0 };
    }
    let abs_y = if y < 0.0 { -y } else { y };
    let abs_x = if x < 0.0 { -x } else { x };
    let a = if abs_x >= abs_y {
        let z = abs_y / abs_x;
        let z2 = z * z;
        z * (0.999_866 - z2 * (0.330_2995 - z2 * (0.180_141 - z2 * (0.085_133 - z2 * 0.020_835))))
    } else {
        let z = abs_x / abs_y;
        let z2 = z * z;
        HALF_PI - z * (0.999_866 - z2 * (0.330_2995 - z2 * (0.180_141 - z2 * (0.085_133 - z2 * 0.020_835))))
    };
    match (x < 0.0, y < 0.0) {
        (false, false) => a,
        (true, false) => PI - a,
        (true, true) => a - PI,
        (false, true) => -a,
    }
}
