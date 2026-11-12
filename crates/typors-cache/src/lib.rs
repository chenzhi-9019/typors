//! Persistent typors state: stable hashing, the cache root, the render cache
//! store and the per-file reading-position store.
//!
//! Deliberately independent of GPUI so names, formats, invalidation and
//! corruption handling are unit-testable without a window.

pub mod features;
pub mod hash;
pub mod root;
pub mod session;
pub mod store;
