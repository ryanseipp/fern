//! Benchmarks `ring_buffer::producer::RingBufferConsumer`

use std::sync::atomic::{AtomicU32, Ordering};

use divan::{Bencher, counter::ItemsCount};
use fern_uring::RingBufferConsumer;

fn main() {
    divan::main();
}

const LENGTHS: &[usize] = &[64, 128, 1024, 2048];

#[divan::bench(consts = LENGTHS)]
fn producer<const N: usize>(bencher: Bencher) {
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
