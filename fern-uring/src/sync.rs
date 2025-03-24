//! Std library sync replacements that enable loom tests.

#[cfg(test)]
pub use loom::sync::*;

#[cfg(not(test))]
pub use std::sync::*;
