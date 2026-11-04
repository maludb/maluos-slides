//! Test support for MaluOS Slides: a heap meter that counts, per thread, the bytes that thread
//! allocated and that are still live, and the process's resident size. The soak tests use both
//! for their memory budgets: the meter is exact and unaffected by the other tests running in the
//! same process, the resident size is what the operating system sees.
//!
//! Install the meter in a test binary with
//! `#[cfg(test)] #[global_allocator] static HEAP: maluos_slides_testkit::ThreadHeap = ThreadHeap;`
//! at the crate root. Only tests of crates that use it as a dev-dependency may do so
//! (`cargo xtask layers` enforces that).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

/// Threads the meter tells apart; the last slot is shared by any beyond that.
const SLOTS: usize = 4096;
const SHARED: usize = SLOTS - 1;
/// Bytes in front of every block: the allocating thread's slot, padded to the block's alignment.
const HEADER_MIN: usize = 16;

static LIVE: [AtomicIsize; SLOTS] = [const { AtomicIsize::new(0) }; SLOTS];
static PEAK: [AtomicIsize; SLOTS] = [const { AtomicIsize::new(0) }; SLOTS];
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static SLOT: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// The current thread's slot, assigned on first use.
fn slot() -> usize {
    // `try_with`: the thread-local may already be gone while the thread is torn down, and the
    // allocator must never panic.
    SLOT.try_with(|s| {
        if s.get() == usize::MAX {
            s.set(NEXT_SLOT.fetch_add(1, Ordering::Relaxed).min(SHARED));
        }
        s.get()
    })
    .unwrap_or(SHARED)
}

/// The system allocator, counting each thread's live bytes.
///
/// Every block carries the slot of the thread that allocated it, so a block freed on another
/// thread still comes off its allocator's count: a thread's figure is exactly the bytes it
/// allocated that are still live, whoever frees them.
pub struct ThreadHeap;

fn add(owner: usize, delta: isize) {
    let now = LIVE[owner].fetch_add(delta, Ordering::Relaxed) + delta;
    PEAK[owner].fetch_max(now, Ordering::Relaxed);
}

fn size(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

fn header(layout: Layout) -> usize {
    layout.align().max(HEADER_MIN)
}

/// The block really allocated for `layout`: the header plus the caller's bytes.
fn outer(layout: Layout) -> Option<Layout> {
    Layout::from_size_align(layout.size().checked_add(header(layout))?, header(layout)).ok()
}

#[allow(unsafe_code)]
// SAFETY: every block is allocated `header(layout)` bytes larger than asked, with an alignment
// of at least that header, so the slot written at its start is aligned and the pointer handed
// out keeps the caller's alignment; dealloc and realloc undo exactly that offset with the same
// layout the caller passes back. The bookkeeping neither allocates nor panics.
unsafe impl GlobalAlloc for ThreadHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(whole) = outer(layout) else { return std::ptr::null_mut() };
        let base = unsafe { System.alloc(whole) };
        if base.is_null() {
            return base;
        }
        let owner = slot();
        unsafe { base.cast::<usize>().write(owner) };
        add(owner, size(layout.size()));
        unsafe { base.add(header(layout)) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let Some(whole) = outer(layout) else { return std::ptr::null_mut() };
        let base = unsafe { System.alloc_zeroed(whole) };
        if base.is_null() {
            return base;
        }
        let owner = slot();
        unsafe { base.cast::<usize>().write(owner) };
        add(owner, size(layout.size()));
        unsafe { base.add(header(layout)) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let Some(whole) = outer(layout) else { return };
        let base = unsafe { ptr.sub(header(layout)) };
        let owner = unsafe { base.cast::<usize>().read() }.min(SHARED);
        add(owner, -size(layout.size()));
        unsafe { System.dealloc(base, whole) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some(whole) = outer(layout) else { return std::ptr::null_mut() };
        let Some(new_whole) = new_size.checked_add(header(layout)) else { return std::ptr::null_mut() };
        let base = unsafe { ptr.sub(header(layout)) };
        let owner = unsafe { base.cast::<usize>().read() }.min(SHARED);
        let moved = unsafe { System.realloc(base, whole, new_whole) };
        if moved.is_null() {
            return moved;
        }
        // The header travels with the block's bytes.
        add(owner, size(new_size) - size(layout.size()));
        unsafe { moved.add(header(layout)) }
    }
}

impl ThreadHeap {
    /// Bytes the current thread allocated that are still live.
    pub fn live() -> usize {
        usize::try_from(LIVE[slot()].load(Ordering::Relaxed)).unwrap_or(0)
    }

    /// The most bytes the current thread has had live since the thread started or
    /// [`ThreadHeap::reset_peak`].
    pub fn peak() -> usize {
        usize::try_from(PEAK[slot()].load(Ordering::Relaxed)).unwrap_or(0)
    }

    /// Starts the peak over from what is live now.
    pub fn reset_peak() {
        let s = slot();
        PEAK[s].store(LIVE[s].load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

/// The process's resident set size in bytes, from `/proc/self/statm` (Linux, 4 KiB pages);
/// `None` where that file does not exist.
pub fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

/// Bytes as a short figure for a log line: `12.3 MiB`.
pub fn mib(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

#[cfg(test)]
#[global_allocator]
static HEAP: ThreadHeap = ThreadHeap;

#[cfg(test)]
mod tests {
    use super::{ThreadHeap, rss_bytes};

    /// The meter follows this thread's own allocations, wherever they are freed, and not
    /// another thread's.
    #[test]
    fn counts_the_calling_threads_live_bytes() {
        let before = ThreadHeap::live();
        ThreadHeap::reset_peak();
        let block = vec![1u8; 4 << 20];
        let with = ThreadHeap::live();
        assert!(with >= before + (4 << 20), "live {with} after {before}");
        assert!(ThreadHeap::peak() >= with);
        // Freed on another thread: still off this thread's count.
        std::thread::spawn(move || drop(block)).join().unwrap_or(());
        assert!(ThreadHeap::live() < with, "freed elsewhere: {}", ThreadHeap::live());
        let mine = ThreadHeap::live();
        let other = std::thread::spawn(|| {
            let _held = vec![2u8; 8 << 20];
            ThreadHeap::live()
        })
        .join()
        .unwrap_or(0);
        assert!(other >= 8 << 20, "the other thread counts its own: {other}");
        assert!(ThreadHeap::live() <= mine, "and not here: {}", ThreadHeap::live());
        // Growing a block in place or by moving keeps the figure right.
        let mut v: Vec<u8> = Vec::with_capacity(1 << 20);
        let grown_from = ThreadHeap::live();
        v.resize(3 << 20, 0);
        assert!(ThreadHeap::live() >= grown_from + (2 << 20), "{}", ThreadHeap::live());
        drop(v);
        assert!(ThreadHeap::live() <= mine + 4096, "{}", ThreadHeap::live());
    }

    #[test]
    fn resident_size_is_reported_on_linux() {
        let rss = rss_bytes();
        if cfg!(target_os = "linux") {
            assert!(rss.is_some_and(|b| b > 1 << 20), "{rss:?}");
        }
    }
}
