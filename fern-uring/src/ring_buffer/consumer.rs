//! A thread-safe and lock-free ring buffer consumer.
//!
//! Reads from head -> tail. When an entry is no longer needed, it can be committed, where the head
//! is incremented. The tail is assumed to be incremented by an external process (the kernel).

use std::sync::atomic::Ordering;

use super::{ReservedEntry, RingBufferError};
use crate::sync::atomic::AtomicU32;

/// A thread-safe and lock-free ring buffer consumer.
///
/// Reads from head -> tail. When an entry is no longer needed, it can be committed, where the head
/// is incremented. The tail is assumed to be incremented by an external process (the kernel).
#[derive(Debug)]
pub struct RingBufferConsumer<'ring, T> {
    head: &'ring AtomicU32,
    tail: &'ring AtomicU32,
    uncommitted_head: AtomicU32,
    entries: &'ring [T],
    mask: u32,
    shift: u32,
}

impl<'ring, T> RingBufferConsumer<'ring, T> {
    /// Creates a new `RingBufferConsumer`, taking existing indicies for the head and tail.
    ///
    /// # Errors
    /// - `entries.len()` must be a power of two. If this is not the case, the
    ///   [`RingBufferError::LengthNotPowerOfTwo`] error is returned.
    /// - `mask` must represent bits of a valid index into `entries`. If this is not the case, the
    ///   [`RingBufferError::InvalidMaskValue`] error is returned.
    pub fn new(
        entries: &'ring [T],
        head: &'ring AtomicU32,
        tail: &'ring AtomicU32,
        mask: u32,
    ) -> Result<Self, RingBufferError> {
        Self::new_internal(entries, head, tail, mask, false)
    }

    /// Creates a new `RingBufferConsumer` for large objects that span two entries, taking existing indicies for the head and tail.
    ///
    /// # Errors
    /// - `entries.len()` must be a power of two. If this is not the case, the
    ///   [`RingBufferError::LengthNotPowerOfTwo`] error is returned.
    /// - `mask` must represent bits of a valid index into `entries`. If this is not the case, the
    ///   [`RingBufferError::InvalidMaskValue`] error is returned.
    pub fn new_big(
        entries: &'ring [T],
        head: &'ring AtomicU32,
        tail: &'ring AtomicU32,
        mask: u32,
    ) -> Result<Self, RingBufferError> {
        Self::new_internal(entries, head, tail, mask, true)
    }

    fn new_internal(
        entries: &'ring [T],
        head: &'ring AtomicU32,
        tail: &'ring AtomicU32,
        mask: u32,
        big: bool,
    ) -> Result<Self, RingBufferError> {
        if entries.len() as u64 > u64::from(u32::MAX) {
            return Err(RingBufferError::EntriesSliceTooLong);
        }
        if (entries.len() as u64).next_power_of_two() != entries.len() as u64 {
            return Err(RingBufferError::LengthNotPowerOfTwo);
        }
        if mask as usize != entries.len() - 1 {
            return Err(RingBufferError::InvalidMaskValue);
        }

        let uncommitted_head = AtomicU32::new(head.load(Ordering::Relaxed));

        Ok(Self {
            head,
            tail,
            uncommitted_head,
            entries,
            mask,
            shift: u32::from(big),
        })
    }

    /// Get the size of the ring buffer.
    #[must_use]
    pub fn size(&self) -> usize {
        self.entries.len() >> self.shift
    }

    /// Get the number of available entries between tail and head. This represents the number of
    /// entries that can currently be reserved.
    #[must_use]
    pub fn available(&self) -> u32 {
        self.tail
            .load(Ordering::Acquire)
            .wrapping_sub(self.head.load(Ordering::Acquire))
    }

    /// Determines if the ring buffer is empty, or has no more elements to reserve.
    ///
    /// If this is true, the consuming side of the ring buffer must consume entries to free up
    /// space. If this returns false, a [`Self::reserve`] operation is only guaranteed to succeed
    /// if there is only one thread producing on this ring buffer.
    #[must_use]
    pub fn empty(&self) -> bool {
        (self.available() as usize) < self.entries.len()
    }

    /// Reserves an entry from the head of the ring buffer.
    #[must_use]
    pub fn reserve(&self) -> Option<ReservedEntry<'ring, T>> {
        let head = self.uncommitted_head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);

        if head == tail {
            None
        } else {
            let entry = &self.entries[((head & self.mask) << self.shift) as usize];
            if self
                .uncommitted_head
                .compare_exchange(head, head + 1, Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                None
            } else {
                Some(ReservedEntry::new(head, entry))
            }
        }
    }

    /// Commit the reserved entry.
    ///
    /// Ensures the reserved entry is the next to be committed, then advances the head of the ring,
    /// making space available to the producer.
    ///
    /// # Errors
    /// - If `entry` is not the next entry to be committed, either because the same thread reserved
    ///   and committed entries out of order, or another thread reserved the next entry, returns
    ///   [`RingBufferError::CommitOutOfOrder`].
    // Taking `entry` by value is intended to ensure access is no longer possible after committing.
    #[allow(clippy::needless_pass_by_value)]
    pub fn commit(&self, entry: ReservedEntry<'ring, T>) -> Result<(), RingBufferError> {
        if entry.index != self.head.load(Ordering::Acquire) {
            return Err(RingBufferError::CommitOutOfOrder);
        }

        self.head.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use loom::thread::{self, yield_now};

    use crate::sync::Arc;
    use crate::sync::atomic::{AtomicU32, Ordering};
    use crate::{RingBufferConsumer, RingBufferError};

    #[test]
    fn new_returns_err_when_entries_is_larger_than_u32() {
        loom::model(|| {
            let entries = vec![0u32; (u32::MAX as usize) + 1];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;

            let result = RingBufferConsumer::new(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::EntriesSliceTooLong));
        });
    }

    #[test]
    fn new_returns_err_when_entries_not_power_of_two() {
        loom::model(|| {
            let entries = vec![0u32; 31];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;

            let result = RingBufferConsumer::new(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::LengthNotPowerOfTwo));
        });
    }

    #[test]
    fn new_returns_invalid_mask_value() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 * 2 - 2;

            let result = RingBufferConsumer::new(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::InvalidMaskValue));
        });
    }

    #[test]
    fn new_size_returns_entries_len() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;

            let consumer = RingBufferConsumer::new(&entries, &head, &tail, mask).unwrap();
            let result = consumer.size();

            assert_eq!(result, entries.len());
        });
    }

    #[test]
    fn new_big_returns_err_when_entries_is_larger_than_u32() {
        loom::model(|| {
            let entries = vec![0u32; (u32::MAX as usize) + 1];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;

            let result = RingBufferConsumer::new_big(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::EntriesSliceTooLong));
        });
    }

    #[test]
    fn new_big_returns_err_when_entries_not_power_of_two() {
        loom::model(|| {
            let entries = vec![0u32; 31];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;

            let result = RingBufferConsumer::new_big(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::LengthNotPowerOfTwo));
        });
    }

    #[test]
    fn new_big_returns_invalid_mask_value() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 2;

            let result = RingBufferConsumer::new_big(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::InvalidMaskValue));
        });
    }

    #[test]
    fn new_big_size_returns_half_entries_len() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;
            let consumer = RingBufferConsumer::new_big(&entries, &head, &tail, mask).unwrap();

            let result = consumer.size();

            assert_eq!(result, entries.len() / 2);
        });
    }

    #[test]
    fn reserves_no_entries_when_none_are_available() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;
            let consumer = RingBufferConsumer::new(&entries, &head, &tail, mask).unwrap();

            let result = consumer.reserve();

            assert!(result.is_none());
        });
    }

    #[test]
    fn does_not_commit_head_until_entry_is_returned() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;
            let consumer = RingBufferConsumer::new(&entries, &head, &tail, mask).unwrap();

            tail.fetch_add(1, Ordering::Acquire);
            let result = consumer.reserve().unwrap();
            assert_eq!(head.load(Ordering::Acquire), 0);
            let _ = consumer.commit(result);
            assert_eq!(head.load(Ordering::Acquire), 1);
        });
    }

    #[test]
    fn committing_entries_out_of_order_returns_error() {
        loom::model(|| {
            const ENTRIES: usize = 2;
            let entries = vec![0u32; ENTRIES];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(32);
            let mask = u32::try_from(ENTRIES).unwrap() - 1;
            let consumer = RingBufferConsumer::new(&entries, &head, &tail, mask).unwrap();

            let _entry1 = consumer.reserve().unwrap();
            let entry2 = consumer.reserve().unwrap();

            let result = consumer.commit(entry2);

            assert!(result.is_err_and(|e| e == RingBufferError::CommitOutOfOrder));
        });
    }

    #[test]
    fn reserves_entry_when_some_are_available() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = Arc::new(AtomicU32::new(0));
            let r_head = head.clone();
            let tail = Arc::new(AtomicU32::new(0));
            let k_tail = tail.clone();
            let r_tail = tail.clone();
            let mask = 32 - 1;

            thread::spawn(move || {
                k_tail.fetch_add(1, Ordering::Relaxed);
            });

            thread::spawn(move || {
                let consumer = RingBufferConsumer::new(&entries, &r_head, &r_tail, mask).unwrap();

                loop {
                    if let Some(result) = consumer.reserve() {
                        let _ = consumer.commit(result);
                        assert_eq!(1, r_head.load(Ordering::Acquire));
                        return;
                    }

                    yield_now();
                }
            });
        });
    }

    #[test]
    fn can_consume_all_entries_available() {
        loom::model(|| {
            const ENTRIES: usize = 2;
            let entries = vec![0u32; ENTRIES];
            let head = Arc::new(AtomicU32::new(0));
            let r_head = head.clone();
            let tail = Arc::new(AtomicU32::new(0));
            let k_tail = tail.clone();
            let r_tail = tail.clone();
            let mask = u32::try_from(ENTRIES).unwrap() - 1;

            thread::spawn(move || {
                for _ in 0..=ENTRIES {
                    k_tail.fetch_add(1, Ordering::Relaxed);
                }
            });

            thread::spawn(move || {
                let consumer = RingBufferConsumer::new(&entries, &r_head, &r_tail, mask).unwrap();
                for _ in 0..=ENTRIES {
                    loop {
                        if let Some(result) = consumer.reserve() {
                            let _ = consumer.commit(result);
                            break;
                        }

                        yield_now();
                    }
                }

                assert_eq!(ENTRIES + 1, r_head.load(Ordering::Acquire) as usize);
                assert!(consumer.reserve().is_none());
            });
        });
    }
}

#[cfg(feature = "internal_benches")]
mod benches {
    use divan::{Bencher, counter::ItemsCount};

    use super::{AtomicU32, Ordering, RingBufferConsumer};

    const LENGTHS: &[usize] = &[64, 128, 1024, 2048];

    #[divan::bench(consts = LENGTHS)]
    fn consumer<const N: usize>(bencher: Bencher) {
        let entries = vec![0u32; N];
        let head = AtomicU32::new(0);
        let tail = AtomicU32::new(0);
        let mask = u32::try_from(N).unwrap() - 1;
        let consumer = RingBufferConsumer::new(&entries, &head, &tail, mask).unwrap();

        bencher.counter(ItemsCount::new(N)).bench(|| {
            tail.fetch_add(u32::try_from(N).unwrap(), Ordering::Release);
            for _ in 0..N {
                if let Some(item) = consumer.reserve() {
                    let _ = consumer.commit(item);
                }
            }
        });
    }
}
