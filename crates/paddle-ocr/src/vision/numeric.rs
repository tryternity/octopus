//! 集中 paddle-ocr vision 子模块共用的数值转换与几何工具函数。

/// L2 欧氏距离（2D 点对）。
pub(crate) fn l2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// f32 → i32 银行家舍入（NaN/Inf → 0，溢出饱和）。
pub(crate) fn cv_round_ties_even_f32(v: f32) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let r = v.round_ties_even();
    if r < i32::MIN as f32 {
        i32::MIN
    } else if r > i32::MAX as f32 {
        i32::MAX
    } else {
        r as i32
    }
}

/// f64 → i32 银行家舍入（NaN/Inf → 0，溢出饱和）。
pub(crate) fn saturate_cast_i32_round(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let r = v.round_ties_even();
    if r < i32::MIN as f64 {
        i32::MIN
    } else if r > i32::MAX as f64 {
        i32::MAX
    } else {
        r as i32
    }
}

/// f32 → i16（先银行家舍入到 i32，再饱和到 i16 范围）。
pub(crate) fn saturate_cast_i16_from_f32(v: f32) -> i16 {
    cv_round_ties_even_f32(v).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// i32 → i16 饱和转换。
pub(crate) fn saturate_cast_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// 三次样条插值系数（A=-0.75 的 bicubic kernel）。
pub(crate) fn interpolate_cubic_coeffs(x: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let c0 = ((A * (x + 1.0) - 5.0 * A) * (x + 1.0) + 8.0 * A) * (x + 1.0) - 4.0 * A;
    let c1 = ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let one_minus_x = 1.0 - x;
    let c2 = ((A + 2.0) * one_minus_x - (A + 3.0)) * one_minus_x * one_minus_x + 1.0;
    let c3 = 1.0 - c0 - c1 - c2;
    [c0, c1, c2, c3]
}

/// 区间裁剪——上界为 exclusive（返回值 ∈ [lo, hi_exclusive-1]）。
pub(crate) fn clip_i32_exclusive_upper(x: i32, lo: i32, hi_exclusive: i32) -> i32 {
    if x < lo {
        lo
    } else if x >= hi_exclusive {
        hi_exclusive - 1
    } else {
        x
    }
}

/// 区间裁剪——上界为 inclusive（返回值 ∈ [min_v, max_v]）。
pub(crate) fn clamp_i32_inclusive(v: i32, min_v: i32, max_v: i32) -> i32 {
    v.max(min_v).min(max_v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_basic_and_zero() {
        assert_eq!(l2([0.0, 0.0], [3.0, 4.0]), 5.0);
        assert_eq!(l2([1.0, 1.0], [1.0, 1.0]), 0.0);
    }

    #[test]
    fn cv_round_ties_even_f32_normal() {
        assert_eq!(cv_round_ties_even_f32(2.5), 2);
        assert_eq!(cv_round_ties_even_f32(3.5), 4);
        assert_eq!(cv_round_ties_even_f32(2.4), 2);
        assert_eq!(cv_round_ties_even_f32(2.6), 3);
    }

    #[test]
    fn cv_round_ties_even_f32_nan_inf() {
        assert_eq!(cv_round_ties_even_f32(f32::NAN), 0);
        assert_eq!(cv_round_ties_even_f32(f32::INFINITY), 0);
        assert_eq!(cv_round_ties_even_f32(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn saturate_cast_i32_round_normal() {
        assert_eq!(saturate_cast_i32_round(2.5_f64), 2);
        assert_eq!(saturate_cast_i32_round(-2.5_f64), -2);
    }

    #[test]
    fn saturate_cast_i16_from_f32_normal() {
        assert_eq!(saturate_cast_i16_from_f32(0.0), 0);
        assert_eq!(saturate_cast_i16_from_f32(100.7), 101);
        assert_eq!(saturate_cast_i16_from_f32(-100.7), -101);
    }

    #[test]
    fn saturate_cast_i16_from_f32_saturation() {
        assert_eq!(saturate_cast_i16_from_f32(99999.0), i16::MAX);
        assert_eq!(saturate_cast_i16_from_f32(-99999.0), i16::MIN);
        assert_eq!(saturate_cast_i16_from_f32(f32::NAN), 0);
        // 统一版本含 is_finite 检查，Inf → 0（非 i16::MAX）
        assert_eq!(saturate_cast_i16_from_f32(f32::INFINITY), 0);
    }

    #[test]
    fn saturate_cast_i16_from_i32() {
        assert_eq!(saturate_cast_i16(0), 0);
        assert_eq!(saturate_cast_i16(32767), 32767);
        assert_eq!(saturate_cast_i16(32768), 32767);
        assert_eq!(saturate_cast_i16(-32768), -32768);
        assert_eq!(saturate_cast_i16(-32769), -32768);
    }

    #[test]
    fn interpolate_cubic_coeffs_sum_to_one() {
        for i in 0..10 {
            let x = i as f32 / 10.0;
            let c = interpolate_cubic_coeffs(x);
            let sum: f32 = c.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "x={}, sum={}", x, sum);
        }
    }

    #[test]
    fn clip_exclusive_upper() {
        assert_eq!(clip_i32_exclusive_upper(5, 0, 10), 5);
        assert_eq!(clip_i32_exclusive_upper(-1, 0, 10), 0);
        assert_eq!(clip_i32_exclusive_upper(10, 0, 10), 9);
        assert_eq!(clip_i32_exclusive_upper(15, 0, 10), 9);
    }

    #[test]
    fn clamp_inclusive() {
        assert_eq!(clamp_i32_inclusive(5, 0, 10), 5);
        assert_eq!(clamp_i32_inclusive(-1, 0, 10), 0);
        assert_eq!(clamp_i32_inclusive(10, 0, 10), 10);
        assert_eq!(clamp_i32_inclusive(15, 0, 10), 10);
    }
}
