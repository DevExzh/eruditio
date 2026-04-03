//! SIMD-accelerated primitives for performance-critical parsing operations.
//!
//! Each submodule provides a public dispatch function that selects the best
//! available implementation at runtime (x86_64: SSE2/AVX2, aarch64: NEON,
//! wasm32: SIMD128) with a scalar fallback for all other targets.
//!
//! # Safety
//!
//! All `unsafe` code is confined to architecture-specific inner modules.
//! Public functions are safe to call from any context.

pub(crate) mod match_length;
pub(crate) mod byte_scan;
pub(crate) mod cp1252;
pub(crate) mod hex_decode;
pub(crate) mod case_fold;
pub(crate) mod histogram;
