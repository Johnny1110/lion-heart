//! Real-time safety utilities.

/// Set the current thread's floating-point mode to flush denormals to zero.
///
/// Denormals (subnormals) — tiny values below the normal float range — are
/// handled in microcode on most CPUs and can be **100× slower** than normal
/// floats. In audio callbacks they accumulate in IIR filter states, delay
/// lines, and reverb tanks after the input goes silent, causing CPU spikes
/// and potential dropouts.
///
/// This function sets:
/// - **FTZ** (Flush-To-Zero): outputs of FP ops that would be denormal are
///   flushed to zero.
/// - **DAZ** (Denormals-Are-Zero): denormal *inputs* are treated as zero.
///
/// Call this once at the top of each audio callback. On x86/x86_64 it sets
/// the MXCSR register; on aarch64 it sets the FPCR `FZ` bit; on other
/// architectures it is a no-op (rely on per-component software flushing
/// instead).
///
/// **Safety:** This modifies thread-local CPU state. It is safe to call from
/// any thread, but the effect persists for the thread's lifetime. Non-audio
/// threads should not need this — only the audio callback thread benefits.
#[inline]
pub fn flush_denormals_to_zero() {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // MXCSR bit 15 = FTZ, bit 6 = DAZ.
        let mut mxcsr = core::mem::MaybeUninit::<u32>::uninit();
        unsafe {
            core::arch::asm!(
                "stmxcsr [{p}]",
                p = in(reg) mxcsr.as_mut_ptr(),
                options(nostack),
            );
            let mut val = mxcsr.assume_init();
            val |= (1 << 15) | (1 << 6);
            core::arch::asm!(
                "ldmxcsr [{p}]",
                p = in(reg) &val,
                options(nostack),
            );
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // Set the FZ (Flush-to-Zero) bit in FPCR. Bit 24 = FZ.
        unsafe {
            let mut fpcr: u64;
            core::arch::asm!(
                "mrs {fpcr}, fpcr",
                "orr {fpcr}, {fpcr}, #{bit}",
                "msr fpcr, {fpcr}",
                fpcr = out(reg) fpcr,
                bit = const 1 << 24,
                options(nostack, preserves_flags),
            );
        }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64",)))]
    {
        // No hardware FTZ/DAZ on this architecture — rely on per-component
        // software flushing (e.g. Biquad::process_sample flushes z1/z2).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_denormals_does_not_panic() {
        // Just verify it runs without panicking on the current arch.
        flush_denormals_to_zero();
    }
}
