//! A generic ring buffer

pub mod producer;
pub use producer::*;

pub mod consumer;
pub use consumer::*;

use std::{fmt::Display, ops::Deref};

/// Errors that occur as a result of using [`RingBufferConsumer`]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum RingBufferError {
    #[default]
    /// There are too many entries in the slice.
    EntriesSliceTooLong,
    /// Length of entries was not a power of two.
    LengthNotPowerOfTwo,
    /// Mask has incorrect value for length of entries.
    InvalidMaskValue,
    /// A commit was attempted out of order. Another thread may have the next entry to commit.
    /// Retrying the operation may succeed.
    CommitOutOfOrder,
}

impl Display for RingBufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EntriesSliceTooLong => f.write_str("Entries slice was too long for the ring buffer."),
            Self::LengthNotPowerOfTwo => f.write_str("Length of entries was not a power of two."),
            Self::InvalidMaskValue => {
                f.write_str("Mask has incorrect value for length of entries.")
            }
            Self::CommitOutOfOrder => f.write_str("A commit was attempted out of order. Another thread may have the next entry to commit. Retrying the operation may succeed.")
        }
    }
}

/// An entry returned as part of a reserve operation.
#[derive(Debug)]
pub struct ReservedEntry<'ring, T> {
    index: u32,
    entry: &'ring T,
}

impl<'ring, T> ReservedEntry<'ring, T> {
    fn new(index: u32, entry: &'ring T) -> Self {
        Self { index, entry }
    }
}

impl<T> Deref for ReservedEntry<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.entry
    }
}
