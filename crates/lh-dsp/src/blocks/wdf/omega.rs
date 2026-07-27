//! The **Wright omega** function `ω(x)` — the closed-form kernel that lets a
//! WDF diode root be evaluated without iterating.
//!
//! # Why this exists
//!
//! A WDF diode root has to solve `a = v + R·i(v)` for `v` every (oversampled)
//! sample. Newton iteration does it exactly but pays a `f64::exp` and a divide
//! per step — the single biggest cost in a white-box drive pedal (`screamer`
//! spent ~68 µs/block on it). Werner et al. showed the same equation can be
//! *rearranged* into an evaluation of Wright's `ω`, defined as the solution of
//!
//! ```text
//! ω + ln ω = x
//! ```
//!
//! and D'Angelo showed `ω` itself can be approximated to audio accuracy with a
//! polynomial guess plus a Newton correction, where the `exp`/`log` inside the
//! correction are themselves polynomial + bit-trick approximations. The result
//! is a fixed, branch-light, iteration-free evaluation.
//!
//! # Accuracy
//!
//! This is an *approximation ladder*, not an exact function — see the pinned
//! bounds in [`OMEGA_MAX_ABS_ERR`] and the tests below. Every stage is measured
//! against a `f64` Newton reference, and the diode root that consumes it is in
//! turn measured against the `f64` Newton oracle it replaces
//! (`DiodePair::solve_newton`).
//!
//! # Sources, and what is ours
//!
//! - S. D'Angelo, L. Gabrielli, L. Turchet, *"Fast Approximation of the Lambert
//!   W Function for Virtual Analog Modelling"*, DAFx-19 — the ladder's shape:
//!   range-reduced polynomial `exp`/`log`, a piecewise polynomial guess, and a
//!   Newton correction on `ω − e^{x−ω}`. Original implementation MIT-licensed;
//!   rewritten here in Rust (bit tricks via `f32::to_bits`/`from_bits` rather
//!   than a C `union`). [`log2_approx`]/[`pow2_approx`] use its coefficients.
//! - `chowdsp_wdf` `math/omega.h` (BSD-3) — the C++ adaptation this port
//!   follows for structure.
//!
//! [`omega_guess`] is **not** the reference's cubic. Its quartic coefficients,
//! its region boundaries and its two-term asymptote were fitted here (offline,
//! by minimising the error *after* the correction rather than the error of the
//! guess itself). That buys reference-quality accuracy from a single correction
//! where the reference cubic needs two — see [`CORRECTIONS`].

/// How many Newton corrections refine [`omega_guess`].
///
/// One, because the guess was fitted for it. The alternatives were all measured
/// inside `screamer` (whole-pedal cost at 48 kHz / 64 frames, and worst-case
/// node-voltage error against the Newton oracle):
///
/// | guess                     | corrections | pedal   | worst Δv |
/// | ------------------------- | ----------- | ------- | -------- |
/// | reference cubic           | 1           | 29.8 µs | 2.03 mV  |
/// | reference cubic           | 2           | 40.4 µs | 39.5 µV  |
/// | fitted quartic (this one) | 1           | 30.5 µs | 30.5 µV  |
///
/// So the fitted guess beats the reference's two-correction accuracy at close
/// to its one-correction cost. A second correction on top of it buys nothing:
/// the correction evaluates `e^{x−ω}` with [`exp_approx`], whose ~6e-4 relative
/// error floors the result at about `ω·6e-4/(1+ω)`, and the fit already sits on
/// that floor.
const CORRECTIONS: usize = 1;

/// Pinned worst-case **absolute** error of [`omega`] against a `f64` Newton
/// reference over `x ∈ [-30, 2000]`. Measured 7.0e-4, and that peak is `f32`
/// resolution at `x ≈ 1400` rather than anything the approximation did — in the
/// audio-relevant range it is under 2e-4. On a diode node this is `Vt` times
/// smaller again, i.e. tens of µV. Asserted by `omega_is_accurate`.
pub const OMEGA_MAX_ABS_ERR: f32 = 1.0e-3;

/// Arguments are clamped to `±OMEGA_ARG_LIMIT` before evaluation. The limit is
/// ~30 orders of magnitude beyond anything a diode root can present (a 1 MV
/// incident wave lands at `x ≈ 2e7`), and it keeps [`exp_approx`]'s exponent
/// arithmetic inside `i32` no matter what a caller hands us — RT rule 7, in the
/// same spirit as the Newton path's exponent clamp. Written with `max`/`min`
/// (not `clamp`) so a NaN argument collapses to a finite value instead of
/// propagating.
const OMEGA_ARG_LIMIT: f32 = 1.0e30;

/// Evaluate `c[0]·x³ + c[1]·x² + c[2]·x + c[3]` with Estrin's scheme: the two
/// halves are independent, so a superscalar core overlaps them instead of
/// serialising a Horner chain.
#[inline(always)]
fn estrin3(c: [f32; 4], x: f32) -> f32 {
    let hi = c[1] + c[0] * x;
    let lo = c[3] + c[2] * x;
    lo + hi * (x * x)
}

/// Evaluate `c[0]·x⁴ + … + c[4]` as `(c₄ + c₃x) + x²·((c₂ + c₁x) + c₀x²)` —
/// the Estrin split for a quartic: three independent sub-expressions, then two
/// dependent steps, so its latency is barely above [`estrin3`]'s.
#[inline(always)]
fn estrin4(c: [f32; 5], x: f32) -> f32 {
    let x2 = x * x;
    let lo = c[4] + c[3] * x;
    let hi = (c[2] + c[1] * x) + c[0] * x2;
    lo + x2 * hi
}

/// `log2(x)` for `x ∈ [1, 2)` — a minimax cubic. Outside that range the result
/// is meaningless; [`log_approx`] supplies the range reduction.
#[inline(always)]
fn log2_approx(x: f32) -> f32 {
    estrin3([0.16404256, -1.0988653, 3.148298, -2.2134752], x)
}

/// `2^x` for `x ∈ [0, 1)` — a minimax cubic. [`exp_approx`] supplies the range
/// reduction.
#[inline(always)]
fn pow2_approx(x: f32) -> f32 {
    // The linear term is `ln 2` exactly — it is `d(2^x)/dx` at 0, not a fitted
    // value — and the constant term is 1 so the fit is pinned at `2^0 = 1`.
    estrin3([0.07944154, 0.22741129, core::f32::consts::LN_2, 1.0], x)
}

/// `ln(x)` for **strictly positive, finite** `x`, to ~4e-3 *absolute* (the
/// cubic is pinned exact at both ends of `[1, 2)` and sags ~5e-3 in log2 units
/// mid-interval, so this is a seed for [`omega`]'s asymptotic branch, not a
/// drop-in `f32::ln`).
///
/// Range reduction is pure integer work: an IEEE-754 `f32` is `m · 2^e` with
/// `m ∈ [1, 2)`, so pulling the exponent field out leaves the mantissa as a
/// float in exactly the interval [`log2_approx`] is fitted on.
#[inline]
pub fn log_approx(x: f32) -> f32 {
    let i = x.to_bits() as i32;
    let ex = i & 0x7f80_0000;
    let e = (ex >> 23) - 127;
    // `i - ex` zeroes the exponent field (the sign bit is 0 for x > 0); the
    // `or` then writes exponent 127, i.e. the mantissa as a value in [1, 2).
    let m = f32::from_bits(((i - ex) | 0x3f80_0000) as u32);
    core::f32::consts::LN_2 * (e as f32 + log2_approx(m))
}

/// `e^x`, to ~6e-4 relative (uniform — it is [`pow2_approx`]'s cubic error,
/// the exponent split being exact), flushing to `0` below `x ≈ -87`.
///
/// Splits `x·log2(e)` into `floor` (written straight into an exponent field)
/// and fraction (fed to [`pow2_approx`]). The `max` keeps the exponent field in
/// range; `as i32` saturates rather than wrapping, and [`omega`] clamps its
/// argument, so the shift can never see a wild value.
#[inline]
pub fn exp_approx(x: f32) -> f32 {
    let x = (core::f32::consts::LOG2_E * x).max(-126.0);
    let xi = x as i32;
    // Truncation rounds toward zero, so negatives need one step down to floor.
    let l = if x < 0.0 { xi - 1 } else { xi };
    let f = x - l as f32;
    let p = f32::from_bits(((l + 127) << 23) as u32);
    p * pow2_approx(f)
}

/// The starting point [`omega`] corrects — three regions, each chosen so that
/// **one** Newton step lands within [`OMEGA_MAX_ABS_ERR`]:
///
/// - `x < X1`: zero. The correction then evaluates to exactly `e^x`, and down
///   here `ω(x) ≈ e^x` to 1.2e-4 — the tail is its own best approximation.
/// - `X1 ≤ x < X2`: a quartic, fitted offline against the *post*-correction
///   error rather than against `ω` itself. That objective is the whole trick:
///   the correction is forgiving in very different degrees across the interval
///   (its error goes as `ω/(2(1+ω))` times the guess error squared, and `ω`
///   spans three decades here), so the optimiser is free to spend guess error
///   where it will be crushed and hoard accuracy where it will not. Fitting
///   `ω` uniformly instead measured 1.0e-3 — 40% worse.
/// - `x ≥ X2`: `x − ln x + ln x / x`. The bare `x − ln x` is 0.26 low at `X2`
///   (it is only the leading asymptotic term) and one correction cannot recover
///   that; the second term drops the guess error to ~1.6e-3 for one divide.
///
/// Not accurate on its own — up to 0.034 absolute, and at the bottom of the
/// quartic's range it overshoots `ω` threefold — so it is exposed only because
/// a future root that wants the rawest, cheapest tier can call it directly.
///
/// The regions are selected with real branches. Evaluating both sides and
/// blending was tried — a diode clipper parks its operating point near the `X2`
/// seam, so mispredicts looked likely — and measured ~7% *slower* inside
/// `screamer`; the branches predict well enough that eager evaluation is pure
/// added work.
#[inline]
pub fn omega_guess(x: f32) -> f32 {
    /// Below this, `e^x` is the better guess (and free — it is what the
    /// correction computes anyway).
    const X1: f32 = -4.5;
    /// Above this the asymptote beats the polynomial.
    const X2: f32 = 8.0;
    /// Fitted here, descending powers. See the module docs — these are ours,
    /// not the reference's.
    const GUESS: [f32; 5] = [
        -0.00028977805,
        0.00025240693,
        0.0586734,
        0.35646075,
        0.5953384,
    ];

    if x < X1 {
        0.0
    } else if x < X2 {
        estrin4(GUESS, x)
    } else {
        let l = log_approx(x);
        x - l + l / x
    }
}

/// Wright's `ω(x)`: the solution of `ω + ln ω = x`.
///
/// [`omega_guess`] refined by [`CORRECTIONS`] Newton steps on
/// `g(ω) = ω − e^{x−ω}` (whose root is the same, and whose derivative
/// `1 + e^{x−ω}` is `1 + ω` at the root — so the step is a divide, not another
/// transcendental). Worst-case absolute error [`OMEGA_MAX_ABS_ERR`]; monotone
/// and finite for every input, including NaN.
#[inline]
// Not `clamp`: `f32::clamp` propagates NaN, `max`/`min` fold it to a bound.
// Folding is the whole point here (RT rule 7 — nothing non-finite leaves).
#[allow(clippy::manual_clamp)]
pub fn omega(x: f32) -> f32 {
    let x = x.max(-OMEGA_ARG_LIMIT).min(OMEGA_ARG_LIMIT);
    let mut y = omega_guess(x);
    for _ in 0..CORRECTIONS {
        y -= (y - exp_approx(x - y)) / (y + 1.0);
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ω(x)` to `f64` accuracy. Solves in log space — `h(u) = e^u + u − x` is
    /// monotone *and* convex, so Newton from any point with `h > 0` walks down
    /// to the root without ever needing a bracket — then returns `e^u`.
    fn omega_reference(x: f64) -> f64 {
        let mut u = if x > 1.0 { x.ln() } else { x };
        for _ in 0..200 {
            let e = u.exp();
            let du = (e + u - x) / (e + 1.0);
            u -= du;
            if du.abs() <= 1e-16 * (1.0 + u.abs()) {
                break;
            }
        }
        u.exp()
    }

    #[test]
    fn omega_reference_satisfies_its_own_definition() {
        for &x in &[-40.0, -5.0, -1.0, 0.0, 1.0, 8.0, 50.0, 1e6] {
            let w = omega_reference(x);
            assert!((w + w.ln() - x).abs() < 1e-12 * (1.0 + x.abs()), "x={x}");
        }
    }

    /// The headline accuracy claim: [`omega`] is within [`OMEGA_MAX_ABS_ERR`]
    /// of the truth across everything a diode root can present.
    #[test]
    fn omega_is_accurate() {
        let mut worst = (0.0f64, 0.0f64);
        // Dense through the knee (where the cubic and the asymptote meet), then
        // sparse out to the arguments a slammed input produces.
        let grid = (0..24_001)
            .map(|k| -30.0 + k as f64 * 60.0 / 24_000.0)
            .chain((0..4_001).map(|k| 30.0 + k as f64 * 1970.0 / 4_000.0));
        for x in grid {
            let got = f64::from(omega(x as f32));
            let want = omega_reference(x);
            let err = (got - want).abs();
            if err > worst.0 {
                worst = (err, x);
            }
        }
        assert!(
            worst.0 < f64::from(OMEGA_MAX_ABS_ERR),
            "worst |Δω| = {:.3e} at x = {:.3} (bound {OMEGA_MAX_ABS_ERR:e})",
            worst.0,
            worst.1
        );
    }

    /// The low tail, where the "reverse diode" of an antiparallel pair lives.
    /// Down here the correction returns `e^x` and the leading term `ω` drops —
    /// `ω(x) = e^x(1 − e^x + …)` — so the relative error is inherently about
    /// `e^x`, worst at the top of the region and vanishing below. What reaches
    /// a node voltage is the absolute error, and that stays tiny throughout.
    #[test]
    fn omega_low_tail_error_is_bounded_by_its_leading_term() {
        for k in 0..2_000 {
            let x = -60.0 + f64::from(k) * 0.0275;
            let want = omega_reference(x);
            let got = f64::from(omega(x as f32));
            let err = (got - want).abs();
            assert!(err <= 1.5e-4, "x={x}: |Δω| = {err:e}");
            assert!(err <= 1.2e-2 * want, "x={x}: got {got:e} want {want:e}");
        }
    }

    /// `ω` is strictly increasing; a monotone root is what keeps a clipper's
    /// transfer curve free of kinks, and it must survive the branch seams at
    /// `X1`/`X2` and the far tails.
    #[test]
    fn omega_is_monotonic() {
        let mut prev = f32::NEG_INFINITY;
        let mut x = -200.0f32;
        while x < 1.0e7 {
            let y = omega(x);
            assert!(y.is_finite(), "x={x} -> {y}");
            assert!(y >= prev, "not monotonic at x={x}: {prev} -> {y}");
            prev = y;
            // Fine near the seams and the audio range, coarse out in the tails.
            x += if x < 20.0 { 1.0e-3 } else { x * 1.0e-4 };
        }
    }

    /// The low tail has a clean closed form (`ω(x) → e^x`); the high tail does
    /// not (`x − ln x` is only the *first* asymptotic term and is still 0.9%
    /// low at `x = 20`), so up there we check the defining relation instead —
    /// which is the property that actually matters.
    #[test]
    fn omega_asymptotics() {
        for &x in &[-40.0f32, -25.0, -12.0] {
            let want = x.exp();
            assert!((omega(x) - want).abs() <= 2e-3 * want, "low tail x={x}");
        }
        for &x in &[20.0f32, 200.0, 1.0e4, 1.0e6] {
            let w = f64::from(omega(x));
            let residual = w + w.ln() - f64::from(x);
            assert!(
                residual.abs() <= 2e-3 * f64::from(x).max(1.0),
                "high tail x={x}: ω + ln ω − x = {residual:e}"
            );
        }
    }

    /// RT rule 7 at the leaf: no argument — however absurd, including NaN —
    /// produces a non-finite result.
    #[test]
    fn omega_extremes_stay_finite() {
        for &x in &[
            f32::MAX,
            f32::MIN,
            1.0e30,
            -1.0e30,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            0.0,
            -0.0,
        ] {
            let y = omega(x);
            assert!(y.is_finite(), "omega({x}) = {y}");
            assert!(y >= 0.0, "omega is positive: omega({x}) = {y}");
        }
    }

    /// Pins the two range-reduced cubics at the accuracy they actually deliver
    /// — which is `pow2_approx`/`log2_approx`'s error, not `f32::exp`'s. Both
    /// bounds matter downstream: `exp_approx`'s relative error is what floors
    /// [`omega`] (see [`CORRECTIONS`]), and `log_approx` only ever seeds a
    /// guess.
    #[test]
    fn exp_and_log_approximations_are_accurate() {
        for k in 0..20_000 {
            let x = -87.0 + f64::from(k) * 0.005;
            let want = x.exp();
            let got = f64::from(exp_approx(x as f32));
            assert!(
                (got - want).abs() <= 7e-4 * want,
                "exp x={x}: {got:e} vs {want:e}"
            );
        }
        for k in 1..20_000 {
            let x = f64::from(k) * 0.02;
            let want = x.ln();
            let got = f64::from(log_approx(x as f32));
            assert!((got - want).abs() < 5e-3, "log x={x}: {got} vs {want}");
        }
    }

    /// `exp_approx` flushes to exact zero below the `f32` normal range rather
    /// than producing a denormal (RT rule 7).
    #[test]
    fn exp_approx_flushes_far_underflow() {
        assert_eq!(exp_approx(-200.0), 0.0);
        assert!(exp_approx(-80.0) > 0.0);
    }

    /// `omega_guess` alone is only a starting point — this pins how far off it
    /// is, so a regression in the fitted coefficients shows up here rather than
    /// as a mystery in the corrected value.
    #[test]
    fn omega_guess_is_a_usable_starting_point() {
        for k in 0..6_000 {
            let x = -20.0 + f64::from(k) * 0.01;
            let want = omega_reference(x);
            let got = f64::from(omega_guess(x as f32));
            assert!(
                (got - want).abs() <= 0.045 + 0.01 * want,
                "x={x}: {got} vs {want}"
            );
        }
    }

    /// The guess switches formula at two seams, and a *step* there would put a
    /// kink in every clipper's transfer curve. One correction has to close the
    /// gap from both sides: the change across each seam must match the true
    /// function's own change, to within the pinned error bound on each side.
    #[test]
    fn omega_has_no_step_at_the_region_seams() {
        for &seam in &[-4.5f32, 8.0] {
            for scale in [1e-4f32, 1e-3, 1e-2] {
                let (lo, hi) = (seam - scale, seam + scale);
                let jump = f64::from(omega(hi) - omega(lo));
                let want = omega_reference(f64::from(hi)) - omega_reference(f64::from(lo));
                assert!(
                    (jump - want).abs() < 2.0 * f64::from(OMEGA_MAX_ABS_ERR),
                    "seam {seam} (±{scale}): step {jump:e}, true {want:e}"
                );
            }
        }
    }
}
