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

#[cfg(test)]
mod test {
    use loom::thread::{self, yield_now};

    use crate::sync::Arc;
    use crate::sync::atomic::{AtomicU32, Ordering};
    use crate::{RingBufferConsumer, RingBufferProducer};

    #[test]
    fn producer_and_consumer_work_together_to_avoid_deadlocks() {
        let mut model = loom::model::Builder::new();
        // limit search space or this will run for a long time
        model.preemption_bound = Some(3);

        model.check(|| {
            const ENTRIES: usize = 2;
            let entries = Arc::new(vec![0u32; ENTRIES]);
            let c_entries = entries.clone();
            let p_entries = entries.clone();

            let mask = u32::try_from(ENTRIES).unwrap() - 1;

            let head = Arc::new(AtomicU32::new(0));
            let c_head = head.clone();
            let p_head = head.clone();

            let tail = Arc::new(AtomicU32::new(u32::try_from(ENTRIES).unwrap()));
            let c_tail = tail.clone();
            let p_tail = tail.clone();

            thread::spawn(move || {
                let consumer = RingBufferConsumer::new(&c_entries, &c_head, &c_tail, mask).unwrap();

                for _ in 0..=ENTRIES {
                    loop {
                        if let Some(result) = consumer.reserve() {
                            let _ = consumer.commit(result);
                            break;
                        }

                        yield_now();
                    }
                }

                // c_tail can be anything when we get here. We only care that the head reached a
                // certain point, consuming a certain number of entries.
                assert_eq!(ENTRIES + 1, c_head.load(Ordering::Acquire) as usize);
            });

            thread::spawn(move || {
                let producer = RingBufferProducer::new(&p_entries, &p_head, &p_tail, mask).unwrap();

                for _ in 0..=ENTRIES {
                    loop {
                        if let Some(result) = producer.reserve() {
                            let _ = producer.commit(result);
                            break;
                        }

                        yield_now();
                    }
                }

                // p_head must be in a certain location for p_tail to reach its desired point,
                // producing a certain number of entries.
                assert_eq!(ENTRIES + 1, p_head.load(Ordering::Acquire) as usize);
                assert_eq!(ENTRIES * 2 + 1, p_tail.load(Ordering::Acquire) as usize);
            });
        });
    }
}
