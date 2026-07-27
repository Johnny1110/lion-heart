//! Antiderivative anti-aliasing (ADAA) and the shaping curves that use it.
//!
//! # The problem
//!
//! A memoryless shaper `y = f(x)` creates harmonics without limit. Everything
//! past Nyquist folds back as *inharmonic* aliasing, and that is what "fizzy,
//! sandy, digital up the neck" sounds like. Running the shaper inside
//! [`super::oversample`] pushes the foldover two octaves up and buys ~70 dB of
//! stopband, but a hard corner generates harmonics that fall off so slowly that
//! 4× is not enough on its own.
//!
//! # The fix
//!
//! Instead of point-sampling `f(x[n])`, convolve the *continuous* signal with a
//! short kernel before sampling — which, for an `x` that is linearly
//! interpolated between samples, has a closed form in the **antiderivatives**
//! of `f` (Parker, Zavalishin & Le Bivic, DAFx-16).
//!
//! **First order** ([`Adaa1`]) — a one-sample rectangular kernel, i.e. the mean
//! of `f` over the segment:
//!
//! ```text
//! y[n] = (F₁(x[n]) − F₁(x[n−1])) / (x[n] − x[n−1])
//! ```
//!
//! **Second order** ([`Adaa2`]) — a two-sample triangular kernel. Integrating
//! `∫₀¹ (1−s)·f(x₁ + s·Δ) ds` by parts gives one closed form per half, sharing
//! the centre sample `x₁`:
//!
//! ```text
//! y[n] = A(Δ₀) + A(Δ₂),  Δ₀ = x[n] − x[n−1],  Δ₂ = x[n−2] − x[n−1]
//! A(Δ)  = (F₂(x₁+Δ) − F₂(x₁)) / Δ²  −  F₁(x₁) / Δ
//! ```
//!
//! Each half degenerates independently as `Δ → 0`, and expanding `F₂` there
//! leaves `A → f(x₁)/2 + Δ·f′(x₁)/6`, which is exactly `½·f(x₁ + Δ/3)` to the
//! same order — so the near-zero branch is one evaluation of `f`, with no
//! nested cases. (Derived here rather than transcribed; the published form is
//! written differently. `A + A` reproduces the triangular kernel's `(x₀ + 4x₁ +
//! x₂)/6` for a linear `f`, and a test pins both that and the defining integral.)
//!
//! # Cost of admission: group delay
//!
//! ADAA1 delays by half a sample, ADAA2 by one. **These run at the oversampled
//! rate**, so at 4× that is 0.125 and 0.25 samples at the base rate. That
//! matters because several pedals sum a *dry* path against the shaped one
//! (`ts9`'s `x + clipped`): an undelayed sum combs. Measured
//! (`dry_sum_comb_error_is_small_at_4x`), the worst-case ripple is 0.01 dB at
//! 1 kHz, 0.09 dB at 10 kHz and 0.22 dB at 16 kHz — below audibility on an
//! instrument whose cab rolls off above 6 kHz, so the retrofit needs no delay
//! compensation. At 1× it would be four times worse and would.
//!
//! # Numerics
//!
//! Both forms are difference quotients: `F₁` values that agree to many digits
//! are subtracted and divided by a small `Δ`. In `f32` that loses the result
//! outright for slow-moving signals, so **all ADAA arithmetic is `f64`** and
//! the curves are written in `f64` too. The near-zero branches then only have
//! to cover `Δ` small enough that their own truncation error is negligible.
//!
//! Every `F₁` and `F₂` here is normalised to `F(0) = 0`. That is not cosmetic:
//! it keeps the subtracted values small near the origin, where guitar signals
//! spend most of their time, and it lets [`Adaa1::reset`] zero the state
//! without evaluating anything.

/// Below this the first-order difference quotient is replaced by the midpoint.
/// In `f64` the crossover between cancellation error and midpoint truncation
/// sits far below this; the branch is about avoiding division by zero.
const EPS1: f64 = 1e-6;
/// The second-order form divides by `Δ²`, so it gives up on a wider band.
const EPS2: f64 = 1e-5;

/// Flush a decaying tail before it reaches denormal territory (RT rule 7).
#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1e-30 { 0.0 } else { v }
}

/// First-order ADAA: one sample of state, half a sample of delay.
///
/// Use for smooth curves (`tanh`, algebraic diodes) and anywhere the shape can
/// change at runtime — every curve supports it.
#[derive(Default, Clone, Copy)]
pub struct Adaa1 {
    x1: f64,
    f1_x1: f64,
}

impl Adaa1 {
    pub const fn new() -> Self {
        Self {
            x1: 0.0,
            f1_x1: 0.0,
        }
    }

    /// Valid because every curve here normalises `F₁(0) = 0`.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.f1_x1 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32, f: impl Fn(f64) -> f64, f1: impl Fn(f64) -> f64) -> f32 {
        let x0 = f64::from(x);
        let f1_x0 = f1(x0);
        let d = x0 - self.x1;
        let y = if d.abs() > EPS1 {
            (f1_x0 - self.f1_x1) / d
        } else {
            f(0.5 * (x0 + self.x1))
        };
        self.x1 = flush(x0);
        self.f1_x1 = flush(f1_x0);
        y as f32
    }
}

/// Second-order ADAA: two samples of state, one sample of delay, and a
/// markedly lower alias floor on hard corners.
///
/// Needs a second antiderivative, so it is only available for the curves whose
/// `F₂` is elementary ([`Curve::order`] says which).
#[derive(Default, Clone, Copy)]
pub struct Adaa2 {
    x1: f64,
    x2: f64,
    f1_x1: f64,
    f2_x1: f64,
    f2_x2: f64,
}

impl Adaa2 {
    pub const fn new() -> Self {
        Self {
            x1: 0.0,
            x2: 0.0,
            f1_x1: 0.0,
            f2_x1: 0.0,
            f2_x2: 0.0,
        }
    }

    /// Valid because every curve here normalises `F₁(0) = F₂(0) = 0`.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    #[inline]
    pub fn process(
        &mut self,
        x: f32,
        f: impl Fn(f64) -> f64,
        f1: impl Fn(f64) -> f64,
        f2: impl Fn(f64) -> f64,
    ) -> f32 {
        let x0 = f64::from(x);
        let f2_x0 = f2(x0);

        // The two halves of the triangular kernel, each hinged on x[n−1].
        // (Rewriting these through a reciprocal to trade two divisions for
        // multiplies was benched and made no measurable difference on this
        // hardware — left in the form that reads like the derivation.)
        let d0 = x0 - self.x1;
        let a = if d0.abs() > EPS2 {
            (f2_x0 - self.f2_x1) / (d0 * d0) - self.f1_x1 / d0
        } else {
            0.5 * f(self.x1 + d0 / 3.0)
        };
        let d2 = self.x2 - self.x1;
        let b = if d2.abs() > EPS2 {
            (self.f2_x2 - self.f2_x1) / (d2 * d2) - self.f1_x1 / d2
        } else {
            0.5 * f(self.x1 + d2 / 3.0)
        };

        self.x2 = self.x1;
        self.f2_x2 = self.f2_x1;
        self.x1 = flush(x0);
        self.f1_x1 = flush(f1(x0));
        self.f2_x1 = flush(f2_x0);
        (a + b) as f32
    }
}

// --- the shape bank ----------------------------------------------------------

/// Asymmetric hard clip and its antiderivatives — the workhorse behind the
/// `hard` and `fuzz` curves, and behind the retrofitted LED/diode clippers in
/// the drive family. `lo < 0 < hi`.
#[inline]
pub fn clip(x: f64, lo: f64, hi: f64) -> f64 {
    x.clamp(lo, hi)
}

#[inline]
pub fn clip_f1(x: f64, lo: f64, hi: f64) -> f64 {
    if x > hi {
        hi * x - hi * hi / 2.0
    } else if x < lo {
        lo * x - lo * lo / 2.0
    } else {
        x * x / 2.0
    }
}

#[inline]
pub fn clip_f2(x: f64, lo: f64, hi: f64) -> f64 {
    if x > hi {
        hi * hi * hi / 6.0 + hi * x * x / 2.0 - hi * hi * x / 2.0
    } else if x < lo {
        lo * lo * lo / 6.0 + lo * x * x / 2.0 - lo * lo * x / 2.0
    } else {
        x * x * x / 6.0
    }
}

/// `tanh`'s antiderivative `ln cosh`, in the form that survives large `x`
/// (`cosh` overflows around 710 in `f64`, and the pedals reach far past it at
/// full drive).
///
/// This is the hottest function in the retrofit — most of the drive family
/// clips with a `tanh` knee, and ADAA evaluates it once per *oversampled*
/// sample. Past `|x| = 20` the correction term `ln(1 + e^{−2|x|})` is under
/// 4e-18, which is smaller than one `f64` ulp of `|x|` there, so the early
/// return is not an approximation — it is the same number, without the two
/// transcendentals. At the gains these pedals run, most samples take it.
#[inline]
pub fn tanh_f1(x: f64) -> f64 {
    let a = x.abs();
    if a > 20.0 {
        return a - std::f64::consts::LN_2;
    }
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

/// A `tanh` knee that bends at a different height on each polarity — one diode
/// drop against two, the shape most of the drive family clips with. Both
/// branches vanish at the origin, so `f` and `F₁` are continuous there.
#[inline]
pub fn asym_tanh(x: f64, k_pos: f64, k_neg: f64) -> f64 {
    let k = if x >= 0.0 { k_pos } else { k_neg };
    k * (x / k).tanh()
}

#[inline]
pub fn asym_tanh_f1(x: f64, k_pos: f64, k_neg: f64) -> f64 {
    let k = if x >= 0.0 { k_pos } else { k_neg };
    k * k * tanh_f1(x / k)
}

/// The algebraic diode curve `x/√(1+x²)` — a soft knee with, unlike `tanh`, an
/// elementary second antiderivative.
#[inline]
pub fn algebraic(x: f64) -> f64 {
    x / (1.0 + x * x).sqrt()
}

#[inline]
pub fn algebraic_f1(x: f64) -> f64 {
    (1.0 + x * x).sqrt() - 1.0
}

#[inline]
pub fn algebraic_f2(x: f64) -> f64 {
    0.5 * (x * (1.0 + x * x).sqrt() + x.asinh()) - x
}

/// Chebyshev polynomials `T₂..T₅`, ascending powers. `Tₙ(cos θ) = cos nθ`, so
/// driven with a sine at unit amplitude each one produces exactly its own
/// harmonic — the reason to have them at all.
const CHEBY: [&[f64]; 4] = [
    &[-1.0, 0.0, 2.0],
    &[0.0, -3.0, 0.0, 4.0],
    &[1.0, 0.0, -8.0, 0.0, 8.0],
    &[0.0, 5.0, 0.0, -20.0, 0.0, 16.0],
];

/// `Tₙ` on `[−1, 1]`, DC-corrected so `f(0) = 0`, held flat outside — the
/// polynomial itself diverges violently past ±1.
fn cheby(c: &[f64], x: f64) -> f64 {
    let u = x.clamp(-1.0, 1.0);
    let mut acc = 0.0;
    for coeff in c.iter().rev() {
        acc = acc * u + coeff;
    }
    acc - c[0]
}

fn cheby_f1(c: &[f64], x: f64) -> f64 {
    let u = x.clamp(-1.0, 1.0);
    let mut acc = 0.0;
    for (i, coeff) in c.iter().enumerate().rev() {
        acc += coeff * u.powi(i as i32 + 1) / (i as f64 + 1.0);
    }
    let inner = acc - c[0] * u;
    // Outside the clamp the curve is constant, so F₁ continues linearly.
    inner + cheby(c, x) * (x - u)
}

/// One selectable shaping curve. **Append-only** — the pedal stores the index
/// in presets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Curve {
    /// Valve-ish symmetric saturation.
    Soft,
    /// Flat-topped clip: the LED/diode-to-ground sound, and the curve ADAA was
    /// invented for.
    Hard,
    /// One diode drop against two — the even harmonics.
    Asym,
    /// Algebraic soft knee: between `Soft` and `Hard`, and cheap.
    Diode,
    /// Sine fold: past its first fold the curve turns back on itself, so drive
    /// buys harmonics instead of compression.
    Sine,
    /// Triangle wavefolder — the West Coast reflection, harder-edged than the
    /// sine fold.
    Fold,
    /// Quantiser staircase: bit-crush without the sample-rate drop.
    Digital,
    Cheby2,
    Cheby3,
    Cheby4,
    Cheby5,
    /// Heavily asymmetric hard clip — squared off, gated, no clean floor.
    Fuzz,
}

pub const CURVE_COUNT: usize = 12;

impl Curve {
    pub const ALL: [Curve; CURVE_COUNT] = [
        Curve::Soft,
        Curve::Hard,
        Curve::Asym,
        Curve::Diode,
        Curve::Sine,
        Curve::Fold,
        Curve::Digital,
        Curve::Cheby2,
        Curve::Cheby3,
        Curve::Cheby4,
        Curve::Cheby5,
        Curve::Fuzz,
    ];

    /// Faceplate labels, aligned with [`Curve::ALL`].
    pub const LABELS: [&'static str; CURVE_COUNT] = [
        "Soft", "Hard", "Asym", "Diode", "Sine", "Fold", "Digital", "Cheby 2", "Cheby 3",
        "Cheby 4", "Cheby 5", "Fuzz",
    ];

    pub fn from_index(i: usize) -> Curve {
        Curve::ALL[i.min(CURVE_COUNT - 1)]
    }

    /// The highest ADAA order this curve can run: 2 where `F₂` is elementary,
    /// 1 otherwise (`tanh`'s second antiderivative is a polylogarithm, and the
    /// staircase's is not worth the branches).
    pub fn order(self) -> u8 {
        match self {
            Curve::Hard | Curve::Asym | Curve::Diode | Curve::Sine | Curve::Fold | Curve::Fuzz => 2,
            _ => 1,
        }
    }

    // Asymmetric knees: one diode drop one way, two the other.
    const ASYM_POS: f64 = 1.0;
    const ASYM_NEG: f64 = 0.55;
    // The fuzz's clip window, squashed hard on the negative swing.
    const FUZZ_LO: f64 = -0.35;
    const FUZZ_HI: f64 = 1.0;
    /// Quantiser step: 13 levels across the clamp, about 3.7 bits. Finer than
    /// this and the staircase stops being audible as one — at 1/24 the curve
    /// sat within 0.017 of a plain hard clip everywhere, which is a bit-crush
    /// nobody would hear (`no_two_curves_are_the_same_function` caught it).
    const STEP: f64 = 1.0 / 6.0;

    pub fn f(self, x: f64) -> f64 {
        match self {
            Curve::Soft => x.tanh(),
            Curve::Hard => clip(x, -1.0, 1.0),
            Curve::Asym => {
                let k = if x >= 0.0 {
                    Self::ASYM_POS
                } else {
                    Self::ASYM_NEG
                };
                k * algebraic(x / k)
            }
            Curve::Diode => algebraic(x),
            Curve::Sine => x.sin(),
            Curve::Fold => {
                let t = fold_reduce(x);
                if t.abs() <= 1.0 {
                    t
                } else {
                    t.signum() * (2.0 - t.abs())
                }
            }
            // Clamped first: a bare staircase follows its input for ever, and
            // a drive knob would push it straight out of the mix.
            Curve::Digital => Self::STEP * (x.clamp(-1.0, 1.0) / Self::STEP).round(),
            Curve::Cheby2 => cheby(CHEBY[0], x),
            Curve::Cheby3 => cheby(CHEBY[1], x),
            Curve::Cheby4 => cheby(CHEBY[2], x),
            Curve::Cheby5 => cheby(CHEBY[3], x),
            Curve::Fuzz => clip(x, Self::FUZZ_LO, Self::FUZZ_HI),
        }
    }

    pub fn f1(self, x: f64) -> f64 {
        match self {
            Curve::Soft => tanh_f1(x),
            Curve::Hard => clip_f1(x, -1.0, 1.0),
            Curve::Asym => {
                let k = if x >= 0.0 {
                    Self::ASYM_POS
                } else {
                    Self::ASYM_NEG
                };
                k * k * algebraic_f1(x / k)
            }
            Curve::Diode => algebraic_f1(x),
            Curve::Sine => 1.0 - x.cos(),
            Curve::Fold => {
                let t = fold_reduce(x);
                if t.abs() <= 1.0 {
                    t * t / 2.0
                } else {
                    2.0 * t.abs() - t * t / 2.0 - 1.0
                }
            }
            Curve::Digital => {
                // Steps are centred on multiples of STEP, so the k-th spans
                // ±STEP/2 around it; the sum of the completed steps is closed
                // form and the partial one is a rectangle. Past the clamp the
                // curve is flat, so F₁ continues linearly at the top step.
                let s = Self::STEP;
                let u = x.clamp(-1.0, 1.0);
                let k = (u / s).round();
                let inner = s * s * k * (k - 1.0) / 2.0 + k * s * (u - (k - 0.5) * s);
                inner + u.signum() * (x - u)
            }
            Curve::Cheby2 => cheby_f1(CHEBY[0], x),
            Curve::Cheby3 => cheby_f1(CHEBY[1], x),
            Curve::Cheby4 => cheby_f1(CHEBY[2], x),
            Curve::Cheby5 => cheby_f1(CHEBY[3], x),
            Curve::Fuzz => clip_f1(x, Self::FUZZ_LO, Self::FUZZ_HI),
        }
    }

    /// Only defined where [`Curve::order`] is 2.
    pub fn f2(self, x: f64) -> f64 {
        debug_assert_eq!(self.order(), 2, "{self:?} has no elementary F₂");
        match self {
            Curve::Hard => clip_f2(x, -1.0, 1.0),
            Curve::Asym => {
                let k = if x >= 0.0 {
                    Self::ASYM_POS
                } else {
                    Self::ASYM_NEG
                };
                k * k * k * algebraic_f2(x / k)
            }
            Curve::Diode => algebraic_f2(x),
            Curve::Sine => x - x.sin(),
            Curve::Fold => {
                // F₁ has a non-zero mean (½) over the fold period, so F₂ is a
                // ramp plus a periodic part.
                let t = fold_reduce(x);
                let a = t.abs();
                let p = if a <= 1.0 {
                    a * a * a / 6.0 - a / 2.0
                } else {
                    a * a - a * a * a / 6.0 - 1.5 * a + 1.0 / 3.0
                };
                0.5 * x + t.signum() * p
            }
            Curve::Fuzz => clip_f2(x, Self::FUZZ_LO, Self::FUZZ_HI),
            _ => 0.0,
        }
    }
}

/// Reduce onto the wavefolder's period-4 triangle, `t ∈ [−2, 2]`.
#[inline]
fn fold_reduce(x: f64) -> f64 {
    x - 4.0 * (x * 0.25).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numerically integrate the kernel ADAA claims to implement, straight
    /// from the definition, so the closed forms are checked against the maths
    /// rather than against themselves.
    fn kernel_reference(curve: Curve, xs: [f64; 3], order: u8) -> f64 {
        const N: usize = 20_000;
        let f = |v: f64| curve.f(v);
        match order {
            1 => {
                // Mean of f over the segment [x1, x0].
                let (x0, x1) = (xs[0], xs[1]);
                (0..N)
                    .map(|i| {
                        let s = (i as f64 + 0.5) / N as f64;
                        f(x1 + s * (x0 - x1))
                    })
                    .sum::<f64>()
                    / N as f64
            }
            _ => {
                // Triangular kernel hinged on x1: ∫₀¹ (1−s)·f(x1 + s·Δ) ds for
                // each side.
                [xs[0], xs[2]]
                    .iter()
                    .map(|&end| {
                        let d = end - xs[1];
                        (0..N)
                            .map(|i| {
                                let s = (i as f64 + 0.5) / N as f64;
                                (1.0 - s) * f(xs[1] + s * d)
                            })
                            .sum::<f64>()
                            / N as f64
                    })
                    .sum()
            }
        }
    }

    /// The identity that fixes the kernel: for a linear `f`, first-order ADAA
    /// must be a two-point mean and second-order the triangular `(x₀+4x₁+x₂)/6`.
    #[test]
    fn a_linear_curve_reproduces_the_kernels() {
        let f = |v: f64| v;
        let f1 = |v: f64| v * v / 2.0;
        let f2 = |v: f64| v * v * v / 6.0;
        let xs = [0.3f32, -0.7, 1.9, 1.9, -2.4, 0.0];

        let mut a1 = Adaa1::new();
        let mut prev = 0.0f32;
        for x in xs {
            let got = a1.process(x, f, f1);
            assert!(
                (got - 0.5 * (x + prev)).abs() < 1e-6,
                "ADAA1 on a line must be the two-point mean: {got} vs {}",
                0.5 * (x + prev)
            );
            prev = x;
        }

        let mut a2 = Adaa2::new();
        let (mut p1, mut p2) = (0.0f32, 0.0f32);
        for x in xs {
            let got = a2.process(x, f, f1, f2);
            let want = (x + 4.0 * p1 + p2) / 6.0;
            assert!(
                (got - want).abs() < 1e-5,
                "ADAA2 on a line must be the triangular kernel: {got} vs {want}"
            );
            p2 = p1;
            p1 = x;
        }
    }

    /// Every curve, at both orders it supports, against the defining integral.
    #[test]
    fn adaa_matches_the_kernel_it_claims_to_implement() {
        let cases: [[f64; 3]; 6] = [
            [1.4, 0.6, -0.3],
            [2.0, -2.0, 0.5],
            [0.9, 1.1, 1.05],
            [-3.0, 0.0, 3.0],
            [0.05, -0.04, 0.02],
            [7.5, -6.0, 1.25],
        ];
        for curve in Curve::ALL {
            for xs in cases {
                let mut a1 = Adaa1::new();
                a1.x1 = xs[1];
                a1.f1_x1 = curve.f1(xs[1]);
                let got = f64::from(a1.process(xs[0] as f32, |v| curve.f(v), |v| curve.f1(v)));
                let want = kernel_reference(curve, xs, 1);
                assert!(
                    (got - want).abs() < 2e-4,
                    "{curve:?} ADAA1 at {xs:?}: {got:.9} vs {want:.9}"
                );

                if curve.order() == 2 {
                    let mut a2 = Adaa2::new();
                    a2.x1 = xs[1];
                    a2.x2 = xs[2];
                    a2.f1_x1 = curve.f1(xs[1]);
                    a2.f2_x1 = curve.f2(xs[1]);
                    a2.f2_x2 = curve.f2(xs[2]);
                    let got = f64::from(a2.process(
                        xs[0] as f32,
                        |v| curve.f(v),
                        |v| curve.f1(v),
                        |v| curve.f2(v),
                    ));
                    let want = kernel_reference(curve, xs, 2);
                    assert!(
                        (got - want).abs() < 2e-4,
                        "{curve:?} ADAA2 at {xs:?}: {got:.9} vs {want:.9}"
                    );
                }
            }
        }
    }

    /// `F₁` and `F₂` must really be antiderivatives of `f` — checked by
    /// central difference, which catches a sign slip or a missing factor that
    /// the kernel test could absorb.
    #[test]
    fn antiderivatives_differentiate_back_to_the_curve() {
        let h = 1e-5;
        for curve in Curve::ALL {
            assert_eq!(curve.f1(0.0), 0.0, "{curve:?} F₁ must be normalised");
            if curve.order() == 2 {
                assert_eq!(curve.f2(0.0), 0.0, "{curve:?} F₂ must be normalised");
            }
            for x in [-6.1f64, -2.3, -1.05, -0.4, 0.37, 0.95, 1.02, 3.3, 8.0] {
                // Skip points where f itself jumps (the staircase, the clip
                // corners): a central difference is meaningless there.
                if (curve.f(x + h) - curve.f(x - h)).abs() > 0.05 {
                    continue;
                }
                let d1 = (curve.f1(x + h) - curve.f1(x - h)) / (2.0 * h);
                assert!(
                    (d1 - curve.f(x)).abs() < 1e-4,
                    "{curve:?}: F₁′({x}) = {d1:.8}, f = {:.8}",
                    curve.f(x)
                );
                if curve.order() == 2 {
                    let d2 = (curve.f2(x + h) - curve.f2(x - h)) / (2.0 * h);
                    assert!(
                        (d2 - curve.f1(x)).abs() < 1e-4,
                        "{curve:?}: F₂′({x}) = {d2:.8}, F₁ = {:.8}",
                        curve.f1(x)
                    );
                }
            }
        }
    }

    /// The degenerate branches: a held signal must not divide by zero, must
    /// stay finite, and must converge on the curve itself.
    #[test]
    fn a_held_input_falls_back_cleanly() {
        for curve in Curve::ALL {
            for x in [0.0f32, 0.3, 1.0, -2.5, 40.0] {
                let mut a1 = Adaa1::new();
                let mut a2 = Adaa2::new();
                let (mut y1, mut y2) = (0.0, 0.0);
                for _ in 0..8 {
                    y1 = a1.process(x, |v| curve.f(v), |v| curve.f1(v));
                    if curve.order() == 2 {
                        y2 = a2.process(x, |v| curve.f(v), |v| curve.f1(v), |v| curve.f2(v));
                    }
                }
                assert!(y1.is_finite(), "{curve:?} ADAA1 held at {x}");
                assert!(
                    (f64::from(y1) - curve.f(f64::from(x))).abs() < 1e-4,
                    "{curve:?} held at {x}: ADAA1 settled on {y1}, curve says {}",
                    curve.f(f64::from(x))
                );
                if curve.order() == 2 {
                    assert!(y2.is_finite(), "{curve:?} ADAA2 held at {x}");
                    assert!(
                        (f64::from(y2) - curve.f(f64::from(x))).abs() < 1e-4,
                        "{curve:?} held at {x}: ADAA2 settled on {y2}"
                    );
                }
            }
        }
    }

    /// Tiny steps are where the difference quotient cancels hardest. `f64`
    /// should carry them without the output turning to noise.
    #[test]
    fn tiny_steps_stay_smooth() {
        for curve in Curve::ALL {
            let mut a1 = Adaa1::new();
            // Prime the state at the ramp's start, so the first *measured*
            // move really is 1e-7 rather than a step off the reset value.
            a1.process(0.99999, |v| curve.f(v), |v| curve.f1(v));
            let mut prev = f32::NAN;
            for i in 0..200 {
                // A ramp with 1e-7-sized steps, right through the corner at 1.
                let x = 0.99999 + i as f32 * 1e-7;
                let y = a1.process(x, |v| curve.f(v), |v| curve.f1(v));
                assert!(y.is_finite(), "{curve:?} at {x}");
                if prev.is_finite() {
                    assert!(
                        (y - prev).abs() < 0.05,
                        "{curve:?}: a 1e-7 input step moved the output by {}",
                        (y - prev).abs()
                    );
                }
                prev = y;
            }
        }
    }

    #[test]
    fn every_curve_is_bounded_and_silent_on_silence() {
        for curve in Curve::ALL {
            assert_eq!(curve.f(0.0), 0.0, "{curve:?} must pass silence");
            for i in -400..=400 {
                let x = i as f64 * 0.25;
                let y = curve.f(x);
                assert!(y.is_finite() && y.abs() <= 4.0, "{curve:?}({x}) = {y}");
            }
            // Extremes must not explode either — drive knobs reach far.
            for x in [-1e6f64, -1e3, 1e3, 1e6] {
                assert!(curve.f(x).is_finite(), "{curve:?}({x})");
                assert!(curve.f1(x).is_finite(), "{curve:?} F₁({x})");
            }
        }
    }

    /// The headline claim, isolated: the *same* curve, the *same* 4×
    /// oversampler, ADAA on versus off. Nothing else differs, so whatever
    /// separates the two floors is ADAA.
    ///
    /// The drive level is per curve, and that is the point rather than a
    /// convenience: ADAA pays for corners. A `tanh` fed a signal it can
    /// actually bend is already clean at 4× (pinned below) — it only needs
    /// help once the gain has driven it flat, which is exactly where the
    /// family's high-gain pedals run it.
    #[test]
    fn adaa_lowers_the_alias_floor_under_the_same_oversampling() {
        for (curve, gain, min_gain) in [
            (Curve::Hard, 4.0, 25.0),
            (Curve::Fold, 4.0, 15.0),
            (Curve::Soft, 60.0, 10.0),
        ] {
            let plain = alias_floor(curve, None, gain);
            let first = alias_floor(curve, Some(1), gain);
            assert!(
                plain - first > min_gain,
                "{curve:?}: ADAA1 only bought {:.1} dB ({plain:.1} → {first:.1})",
                plain - first
            );
            if curve.order() == 2 {
                let second = alias_floor(curve, Some(2), gain);
                assert!(
                    second <= first + 1.0,
                    "{curve:?}: ADAA2 ({second:.1} dB) should not be worse \
                     than ADAA1 ({first:.1} dB)"
                );
            }
        }
        // The other half of the finding: a smooth curve at a drive it can
        // still bend needs nothing. This is why the retrofit's gains ran from
        // 8 dB to 50 dB across the family rather than landing uniformly.
        let gentle = alias_floor(Curve::Soft, None, 2.0);
        assert!(
            gentle < -80.0,
            "a gently driven tanh should already be clean at 4×, got {gentle:.1} dB"
        );
    }

    /// Inharmonic energy relative to the fundamental, for a 5 kHz probe driven
    /// through the shared 4× oversampler. `adaa` selects the order, or `None`
    /// for plain point sampling.
    fn alias_floor(curve: Curve, adaa: Option<u8>, gain: f32) -> f64 {
        use crate::blocks::oversample::Oversampler4x;
        const SR: u32 = 48_000;
        const F0: f32 = 5_000.0;
        const ALIASES: [f32; 4] = [3_000.0, 8_000.0, 13_000.0, 18_000.0];

        let mut y = crate::testutil::sine(SR, F0, SR as usize);
        let mut os = Oversampler4x::new();
        let mut a1 = Adaa1::new();
        let mut a2 = Adaa2::new();
        os.process(&mut y, |buf| {
            for s in buf.iter_mut() {
                let v = gain * *s;
                *s = match adaa {
                    None => curve.f(f64::from(v)) as f32,
                    Some(1) => a1.process(v, |u| curve.f(u), |u| curve.f1(u)),
                    _ => a2.process(v, |u| curve.f(u), |u| curve.f1(u), |u| curve.f2(u)),
                };
            }
        });
        let tail = &y[y.len() / 2..];
        let goertzel = |freq: f32| -> f64 {
            let n = tail.len() as f64;
            let (mut cs, mut cc) = (0.0f64, 0.0f64);
            for (i, s) in tail.iter().enumerate() {
                let ph = std::f64::consts::TAU * f64::from(freq) * i as f64 / f64::from(SR);
                cs += f64::from(*s) * ph.sin();
                cc += f64::from(*s) * ph.cos();
            }
            ((cs * 2.0 / n).powi(2) + (cc * 2.0 / n).powi(2)).sqrt()
        };
        let fund = goertzel(F0);
        let alias = ALIASES
            .iter()
            .map(|f| goertzel(*f).powi(2))
            .sum::<f64>()
            .sqrt();
        20.0 * (alias / fund.max(1e-12)).log10()
    }

    /// No two entries in the bank are the same function. Checked directly on
    /// the curves rather than through audio: two shapes can render almost
    /// identically at one drive setting and diverge at another, but if they
    /// agree everywhere on this grid one of them is redundant.
    #[test]
    fn no_two_curves_are_the_same_function() {
        let grid: Vec<f64> = (-120..=120).map(|i| f64::from(i) * 0.05).collect();
        for (i, a) in Curve::ALL.iter().enumerate() {
            for (j, b) in Curve::ALL.iter().enumerate().skip(i + 1) {
                let apart = grid
                    .iter()
                    .map(|x| (a.f(*x) - b.f(*x)).abs())
                    .fold(0.0f64, f64::max);
                assert!(
                    apart > 0.02,
                    "{a:?} (index {i}) and {b:?} (index {j}) never differ by \
                     more than {apart:.4} — one of them is redundant"
                );
            }
        }
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(Curve::ALL.len(), CURVE_COUNT);
        assert_eq!(Curve::LABELS.len(), CURVE_COUNT);
        for (i, c) in Curve::ALL.iter().enumerate() {
            assert_eq!(Curve::from_index(i), *c);
        }
        assert_eq!(Curve::from_index(999), Curve::Fuzz, "index must saturate");
    }

    /// The `Tₙ(cos θ) = cos nθ` property: driven with a unit sine, each
    /// Chebyshev curve must put its energy on its own harmonic and (almost)
    /// nowhere else.
    #[test]
    fn chebyshev_curves_generate_their_own_harmonic() {
        const N: usize = 4_096;
        let bin = |y: &[f64], k: usize| -> f64 {
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in y.iter().enumerate() {
                let ph = std::f64::consts::TAU * k as f64 * n as f64 / N as f64;
                re += v * ph.cos();
                im -= v * ph.sin();
            }
            (re * re + im * im).sqrt() * 2.0 / N as f64
        };
        for (curve, order) in [
            (Curve::Cheby2, 2usize),
            (Curve::Cheby3, 3),
            (Curve::Cheby4, 4),
            (Curve::Cheby5, 5),
        ] {
            // 8 cycles of a unit sine — the domain Tₙ is defined on.
            let y: Vec<f64> = (0..N)
                .map(|n| curve.f((std::f64::consts::TAU * 8.0 * n as f64 / N as f64).sin()))
                .collect();
            let own = bin(&y, 8 * order);
            for other in 1..=6 {
                if other == order {
                    continue;
                }
                let level = bin(&y, 8 * other);
                assert!(
                    own > 8.0 * level,
                    "{curve:?}: harmonic {other} at {level:.4} rivals its own \
                     harmonic {order} at {own:.4}"
                );
            }
            assert!(own > 0.5, "{curve:?}: own harmonic only {own:.4}");
        }
    }

    /// The wavefolder must actually fold: more drive means more sign
    /// reversals per cycle, not more compression.
    #[test]
    fn the_wavefolder_folds_more_as_drive_rises() {
        let crossings = |gain: f64| {
            let mut n = 0;
            let mut prev = 0.0;
            for i in 0..2_000 {
                let x = gain * (std::f64::consts::TAU * i as f64 / 2_000.0).sin();
                let y = Curve::Fold.f(x);
                if i > 0 && (y > 0.0) != (prev > 0.0) {
                    n += 1;
                }
                prev = y;
            }
            n
        };
        assert!(crossings(0.9) <= 2, "below the fold it is a plain line");
        assert!(
            crossings(9.0) > crossings(3.0),
            "folds must multiply with drive: {} then {}",
            crossings(3.0),
            crossings(9.0)
        );
    }

    /// The claim from the module docs, measured rather than asserted: because
    /// ADAA runs at 4× rate, summing an undelayed dry path against its
    /// half-sample-delayed output combs only slightly. This is what lets the
    /// retrofit skip delay compensation — and the pinned numbers are what
    /// would have to be revisited if the oversampling ratio ever dropped.
    #[test]
    fn dry_sum_comb_error_is_small_at_4x() {
        let os_rate = 4.0 * 48_000.0;
        // (frequency, bound in dB) — worst case is equal dry and wet levels.
        for (freq, bound) in [
            (1_000.0f64, 0.01),
            (5_000.0, 0.03),
            (10_000.0, 0.09),
            (16_000.0, 0.23),
        ] {
            // ADAA1 on a linear curve is exactly a two-point mean, whose
            // response is cos(πf/fs) with half a sample of delay.
            let w = std::f64::consts::PI * freq / os_rate;
            let (re, im) = (w.cos() * w.cos(), -w.cos() * w.sin());
            let mag = ((1.0 + re).powi(2) + im * im).sqrt() / 2.0;
            let db = 20.0 * mag.log10();
            assert!(
                db.abs() < bound,
                "dry + ADAA1 wet combs by {db:.4} dB at {freq} Hz (bound \
                 {bound}) — the retrofit would need delay compensation"
            );
        }
    }
}
