//! Architecture-specific prefetch primitives.
//!
//! Provides safe wrappers around hardware prefetch instructions that hint the
//! CPU to bring data into cache before it is needed, reducing memory latency
//! on hot paths such as sequential buffer scanning and conversion loops.
//!
//! | Architecture | Backend                                      |
//! |--------------|----------------------------------------------|
//! | x86_64       | `_mm_prefetch` (SSE, always available)       |
//! | aarch64      | `PRFM` instruction (base ISA, always available) |
//! | *other*      | no-op (compile to nothing)                   |
//!
//! All functions are safe to call with any pointer, including null. Prefetch
//! instructions are performance hints only -- they never fault on invalid
//! addresses on modern hardware.

// ---------------------------------------------------------------------------
// x86_64 implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::arch::x86_64::{
        _mm_prefetch, _MM_HINT_ET0, _MM_HINT_NTA, _MM_HINT_T0, _MM_HINT_T1,
    };

    /// Prefetch for reading into L1 cache.
    ///
    /// Cost: 1 uop, non-blocking. Brings the cache line containing `ptr`
    /// into L1d (and all higher levels).
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn prefetch_read_l1(ptr: *const u8) {
        // SAFETY: `_mm_prefetch` is a hint instruction that never faults, even
        // on null or unmapped addresses. SSE is always available on x86_64.
        unsafe {
            _mm_prefetch(ptr as *const i8, _MM_HINT_T0);
        }
    }

    /// Prefetch for reading into L2 cache.
    ///
    /// Cost: 1 uop, non-blocking. Brings the cache line containing `ptr`
    /// into L2 (and L3 if present), but not necessarily L1.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn prefetch_read_l2(ptr: *const u8) {
        // SAFETY: `_mm_prefetch` is a hint instruction that never faults, even
        // on null or unmapped addresses. SSE is always available on x86_64.
        unsafe {
            _mm_prefetch(ptr as *const i8, _MM_HINT_T1);
        }
    }

    /// Prefetch for writing into L1 cache.
    ///
    /// Cost: 1 uop, non-blocking. Brings the cache line into L1d in
    /// exclusive/modified state, avoiding a later RFO (Read For Ownership)
    /// on the first store.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn prefetch_write_l1(ptr: *const u8) {
        // SAFETY: `_mm_prefetch` is a hint instruction that never faults, even
        // on null or unmapped addresses. SSE is always available on x86_64.
        // `_MM_HINT_ET0` requests exclusive ownership into L1 (PREFETCHW).
        unsafe {
            _mm_prefetch(ptr as *const i8, _MM_HINT_ET0);
        }
    }

    /// Non-temporal prefetch (streaming, avoid cache pollution).
    ///
    /// Cost: 1 uop, non-blocking. Hints that the data is used only once
    /// and should not pollute higher cache levels.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn prefetch_nta(ptr: *const u8) {
        // SAFETY: `_mm_prefetch` is a hint instruction that never faults, even
        // on null or unmapped addresses. SSE is always available on x86_64.
        unsafe {
            _mm_prefetch(ptr as *const i8, _MM_HINT_NTA);
        }
    }
}

// ---------------------------------------------------------------------------
// aarch64 implementation
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::arch::asm;

    /// Prefetch for reading into L1 cache.
    ///
    /// Cost: 1 uop, non-blocking. Uses `PRFM PLDL1STRM` to bring the cache
    /// line containing `ptr` into L1d with a streaming (transient) hint.
    #[inline]
    pub(crate) fn prefetch_read_l1(ptr: *const u8) {
        // SAFETY: `PRFM` is a hint instruction (part of base AArch64 ISA) that
        // never faults, even on null or unmapped addresses.
        // - `nomem`: prefetch has no memory-ordering semantics from the
        //   compiler's perspective; it is a performance hint only.
        // - `nostack`: does not use the stack.
        // - `preserves_flags`: does not modify condition flags (NZCV).
        unsafe {
            asm!(
                "prfm pldl1strm, [{0}]",
                in(reg) ptr,
                options(nomem, nostack, preserves_flags),
            );
        }
    }

    /// Prefetch for reading into L2 cache.
    ///
    /// Cost: 1 uop, non-blocking. Uses `PRFM PLDL2STRM` to bring the cache
    /// line containing `ptr` into L2 with a streaming (transient) hint.
    #[inline]
    pub(crate) fn prefetch_read_l2(ptr: *const u8) {
        // SAFETY: `PRFM` is a hint instruction (part of base AArch64 ISA) that
        // never faults, even on null or unmapped addresses.
        unsafe {
            asm!(
                "prfm pldl2strm, [{0}]",
                in(reg) ptr,
                options(nomem, nostack, preserves_flags),
            );
        }
    }

    /// Prefetch for writing into L1 cache.
    ///
    /// Cost: 1 uop, non-blocking. Uses `PRFM PSTL1KEEP` to bring the cache
    /// line containing `ptr` into L1d in exclusive state, avoiding a later
    /// coherence upgrade on the first store.
    #[inline]
    pub(crate) fn prefetch_write_l1(ptr: *const u8) {
        // SAFETY: `PRFM` is a hint instruction (part of base AArch64 ISA) that
        // never faults, even on null or unmapped addresses.
        unsafe {
            asm!(
                "prfm pstl1keep, [{0}]",
                in(reg) ptr,
                options(nomem, nostack, preserves_flags),
            );
        }
    }

    /// Non-temporal prefetch (streaming, avoid cache pollution).
    ///
    /// Cost: 1 uop, non-blocking. AArch64 has no direct NTA equivalent;
    /// uses `PRFM PLDL1STRM` (streaming hint) as the closest approximation.
    #[inline]
    pub(crate) fn prefetch_nta(ptr: *const u8) {
        // SAFETY: `PRFM` is a hint instruction (part of base AArch64 ISA) that
        // never faults, even on null or unmapped addresses.
        unsafe {
            asm!(
                "prfm pldl1strm, [{0}]",
                in(reg) ptr,
                options(nomem, nostack, preserves_flags),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch functions
// ---------------------------------------------------------------------------

/// Prefetch for reading into L1 cache.
///
/// Brings the cache line containing `ptr` into L1d (and all higher levels).
/// On architectures without a dedicated prefetch instruction, this is a no-op.
///
/// Cost: 1 uop on x86_64 and aarch64, zero on other targets.
#[allow(dead_code)]
#[inline]
pub(crate) fn prefetch_read_l1(ptr: *const u8) {
    #[cfg(target_arch = "x86_64")]
    {
        x86::prefetch_read_l1(ptr);
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::prefetch_read_l1(ptr);
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = ptr;
    }
}

/// Prefetch for reading into L2 cache.
///
/// Brings the cache line containing `ptr` into L2 (and L3 if present), but
/// not necessarily L1. On architectures without a dedicated prefetch
/// instruction, this is a no-op.
///
/// Cost: 1 uop on x86_64 and aarch64, zero on other targets.
#[allow(dead_code)]
#[inline]
pub(crate) fn prefetch_read_l2(ptr: *const u8) {
    #[cfg(target_arch = "x86_64")]
    {
        x86::prefetch_read_l2(ptr);
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::prefetch_read_l2(ptr);
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = ptr;
    }
}

/// Prefetch for writing into L1 cache.
///
/// Brings the cache line into L1d in exclusive/modified state, avoiding a
/// later RFO (Read For Ownership) stall on the first store. On architectures
/// without a dedicated prefetch instruction, this is a no-op.
///
/// Cost: 1 uop on x86_64 and aarch64, zero on other targets.
#[allow(dead_code)]
#[inline]
pub(crate) fn prefetch_write_l1(ptr: *const u8) {
    #[cfg(target_arch = "x86_64")]
    {
        x86::prefetch_write_l1(ptr);
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::prefetch_write_l1(ptr);
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = ptr;
    }
}

/// Non-temporal prefetch (streaming, avoid cache pollution).
///
/// Hints that the data is used only once and should not pollute higher cache
/// levels. On architectures without a dedicated NTA prefetch, this falls back
/// to a streaming hint (aarch64) or a no-op (other targets).
///
/// Cost: 1 uop on x86_64 and aarch64, zero on other targets.
#[allow(dead_code)]
#[inline]
pub(crate) fn prefetch_nta(ptr: *const u8) {
    #[cfg(target_arch = "x86_64")]
    {
        x86::prefetch_nta(ptr);
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::prefetch_nta(ptr);
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = ptr;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_read_l1_valid_pointer() {
        let data = vec![0u8; 128];
        prefetch_read_l1(data.as_ptr());
    }

    #[test]
    fn prefetch_read_l2_valid_pointer() {
        let data = vec![0u8; 128];
        prefetch_read_l2(data.as_ptr());
    }

    #[test]
    fn prefetch_write_l1_valid_pointer() {
        let data = vec![0u8; 128];
        prefetch_write_l1(data.as_ptr());
    }

    #[test]
    fn prefetch_nta_valid_pointer() {
        let data = vec![0u8; 128];
        prefetch_nta(data.as_ptr());
    }

    #[test]
    fn prefetch_read_l1_null_pointer() {
        // Prefetch on null is a no-op on all modern CPUs -- must not crash.
        prefetch_read_l1(core::ptr::null());
    }

    #[test]
    fn prefetch_read_l2_null_pointer() {
        prefetch_read_l2(core::ptr::null());
    }

    #[test]
    fn prefetch_write_l1_null_pointer() {
        prefetch_write_l1(core::ptr::null());
    }

    #[test]
    fn prefetch_nta_null_pointer() {
        prefetch_nta(core::ptr::null());
    }

    #[test]
    fn prefetch_into_middle_of_buffer() {
        // Prefetch at various offsets within a heap-allocated buffer.
        let data = vec![0xAAu8; 4096];
        for offset in (0..4096).step_by(64) {
            let ptr = unsafe { data.as_ptr().add(offset) };
            prefetch_read_l1(ptr);
            prefetch_read_l2(ptr);
            prefetch_write_l1(ptr);
            prefetch_nta(ptr);
        }
    }
}
