//! Shared build instrumentation preserves original main.rs and module locations.
include!(concat!(env!("OUT_DIR"),"/instrumented/main.rs"));
