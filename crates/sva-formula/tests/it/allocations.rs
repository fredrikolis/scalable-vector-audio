// Concern: how many allocations a call on this thread makes, under a counting allocator | Non-concern: what allocates them | IO: (a call) -> its value, a count

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
}

fn noted() {
    let _ = MADE.try_with(|made| made.set(made.get() + 1));
}

// SAFETY: forwards each call to `System` unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        noted();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        noted();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        noted();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// `call`'s value and the allocations it made.
pub fn counted<T>(call: impl FnOnce() -> T) -> (T, u64) {
    let before = MADE.with(Cell::get);
    let out = call();
    (out, MADE.with(Cell::get) - before)
}
