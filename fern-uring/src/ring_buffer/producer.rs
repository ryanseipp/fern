//! A thread-safe and lock-free ring buffer producer with two-stage commit.
//!
//! Writes occur after the tail, presuming the ring buffer has space. The producer first reserves
//! the slot, gives it to the caller to write data, then commits the slot to the consumer.

use std::sync::atomic::Ordering;

use super::{ReservedEntry, RingBufferError};
use crate::sync::atomic::AtomicU32;

/// A thread-safe and lock-free ring buffer producer with two-stage commit.
///
/// Writes occur after the tail, presuming the ring buffer has space. The producer first reserves
/// the slot, gives it to the caller to write data, then commits the slot to the consumer.
#[derive(Debug)]
pub struct RingBufferProducer<'ring, T> {
    head: &'ring AtomicU32,
    tail: &'ring AtomicU32,
    uncommitted_tail: AtomicU32,
    entries: &'ring [T],
    mask: u32,
    shift: u32,
}

impl<'ring, T> RingBufferProducer<'ring, T> {
    /// Creates a new `RingBufferProducer`, taking existing indicies for the head and tail.
    ///
    /// # Errors
    /// - if `entries.len()` is greater than `u32::MAX`, the
    ///   [`RingBufferError::EntriesSliceTooLong`] error is returned.
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

    /// Creates a new `RingBufferProducer` for large objects that span two entries, taking existing indicies for the head and tail.
    ///
    /// # Errors
    /// - if `entries.len()` is greater than `u32::MAX`, the
    ///   [`RingBufferError::EntriesSliceTooLong`] error is returned.
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

        let uncommitted_tail = AtomicU32::new(tail.load(Ordering::Relaxed));

        Ok(Self {
            head,
            tail,
            uncommitted_tail,
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

    /// Reserve an entry.
    ///
    /// Produces [`Option::Some`] if an entry was successfully reserved. Otherwise returns
    /// [`Option::None`] if the ring has no more space, or another thread reserved the same entry
    /// first.
    #[must_use]
    pub fn reserve(&self) -> Option<ReservedEntry<'ring, T>> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.uncommitted_tail.load(Ordering::Acquire);

        if tail.wrapping_sub(head) as usize >= self.entries.len() {
            None
        } else {
            let entry = &self.entries[((tail & self.mask) << self.shift) as usize];
            if self
                .uncommitted_tail
                .compare_exchange(tail, tail + 1, Ordering::Release, Ordering::Relaxed)
                .is_err()
            {
                None
            } else {
                Some(ReservedEntry::new(tail, entry))
            }
        }
    }

    /// Commit the reserved entry.
    ///
    /// Ensures the reserved entry is next to be committed, then advances the tail of the ring,
    /// making it visible to the consumer side.
    ///
    /// # Errors
    /// - If `entry` is not the next entry to be committed, either because the same thread reserved
    ///   and committed entries out of order, or another thread reserved the next entry, returns
    ///   [`RingBufferError::CommitOutOfOrder`].
    // Taking `entry` by value is intended to ensure access is no longer possible after committing.
    #[allow(clippy::needless_pass_by_value)]
    pub fn commit(&self, entry: ReservedEntry<'ring, T>) -> Result<(), RingBufferError> {
        if entry.index != self.tail.load(Ordering::Acquire) {
            return Err(RingBufferError::CommitOutOfOrder);
        }

        self.tail.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use loom::thread::{self, yield_now};

    use crate::sync::Arc;
    use crate::sync::atomic::{AtomicU32, Ordering};
    use crate::{RingBufferError, RingBufferProducer};

    #[test]
    fn new_returns_err_when_entries_is_larger_than_u32() {
        loom::model(|| {
            let entries = vec![0u32; (u32::MAX as usize) + 1];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;

            let result = RingBufferProducer::new(&entries, &head, &tail, mask);

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

            let result = RingBufferProducer::new(&entries, &head, &tail, mask);

            assert!(result.is_err_and(|e| e == RingBufferError::LengthNotPowerOfTwo));
        });
    }

    #[test]
    fn new_returns_invalid_mask_value() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 2;

            let result = RingBufferProducer::new(&entries, &head, &tail, mask);

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

            let producer = RingBufferProducer::new(&entries, &head, &tail, mask).unwrap();
            let result = producer.size();

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

            let result = RingBufferProducer::new_big(&entries, &head, &tail, mask);

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

            let result = RingBufferProducer::new_big(&entries, &head, &tail, mask);

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

            let result = RingBufferProducer::new_big(&entries, &head, &tail, mask);

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

            let producer = RingBufferProducer::new_big(&entries, &head, &tail, mask).unwrap();
            let result = producer.size();

            assert_eq!(result, entries.len() / 2);
        });
    }

    #[test]
    fn reserves_no_entries_when_none_are_available() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(32);
            let mask = 32 - 1;
            let producer = RingBufferProducer::new(&entries, &head, &tail, mask).unwrap();

            let result = producer.reserve();

            assert!(result.is_none());
        });
    }

    #[test]
    fn does_not_commit_tail_until_entry_is_returned() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = 32 - 1;
            let producer = RingBufferProducer::new(&entries, &head, &tail, mask).unwrap();

            let result = producer.reserve().unwrap();
            assert_eq!(tail.load(Ordering::Acquire), 0);
            let _ = producer.commit(result);
            assert_eq!(tail.load(Ordering::Acquire), 1);
        });
    }

    #[test]
    fn committing_entries_out_of_order_returns_error() {
        loom::model(|| {
            const ENTRIES: usize = 2;
            let entries = vec![0u32; ENTRIES];
            let head = AtomicU32::new(0);
            let tail = AtomicU32::new(0);
            let mask = u32::try_from(ENTRIES).unwrap() - 1;
            let producer = RingBufferProducer::new(&entries, &head, &tail, mask).unwrap();

            let _entry1 = producer.reserve().unwrap();
            let entry2 = producer.reserve().unwrap();

            let result = producer.commit(entry2);

            assert!(result.is_err_and(|e| e == RingBufferError::CommitOutOfOrder));
        });
    }

    #[test]
    fn reserves_entry_when_some_are_available() {
        loom::model(|| {
            let entries = vec![0u32; 32];
            let head = Arc::new(AtomicU32::new(0));
            let k_head = head.clone();
            let r_head = head.clone();
            let tail = Arc::new(AtomicU32::new(32));
            let r_tail = tail.clone();
            let mask = 32 - 1;

            thread::spawn(move || {
                k_head.fetch_add(1, Ordering::Relaxed);
            });

            thread::spawn(move || {
                let producer = RingBufferProducer::new(&entries, &r_head, &r_tail, mask).unwrap();

                loop {
                    if let Some(result) = producer.reserve() {
                        let _ = producer.commit(result);
                        assert_eq!(33, r_tail.load(Ordering::Acquire));
                        return;
                    }

                    yield_now();
                }
            });
        });
    }
}

#[cfg(feature = "internal_benches")]
mod benches {
    use divan::{Bencher, counter::ItemsCount};

    use super::{AtomicU32, Ordering, RingBufferProducer};

    const LENGTHS: &[usize] = &[64, 128, 1024, 2048];

    #[divan::bench(consts = LENGTHS)]
    fn producer<const N: usize>(bencher: Bencher) {
        let entries = vec![0u32; N];
        let head = AtomicU32::new(0);
        let tail = AtomicU32::new(0);
        let mask = u32::try_from(N).unwrap() - 1;
        let producer = RingBufferProducer::new(&entries, &head, &tail, mask).unwrap();

        bencher.counter(ItemsCount::new(N)).bench(|| {
            for _ in 0..N {
                if let Some(item) = producer.reserve() {
                    let _ = producer.commit(item);
                }
            }
            head.fetch_add(u32::try_from(N).unwrap(), Ordering::Release)
        });
    }
}
