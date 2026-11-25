//! Compile-only probe for the Rust/LLVM VNNI signature used by CPU inference.
//! The test never executes an AVX-512 instruction on the build machine.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{__m512i, _mm512_dpbusd_epi32};

/// Force code generation of the intrinsic even on builders without AVX-512.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512vnni,avx512bw")]
pub fn dot_product(accumulator: __m512i, unsigned: __m512i, signed: __m512i) -> __m512i {
    _mm512_dpbusd_epi32(accumulator, unsigned, signed)
}
