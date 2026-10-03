// Concern: the largest single allocation a call on this thread asks for, under a counting allocator | Non-concern: what allocates it | IO: (a call) -> its value, bytes

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn noted(size: usize) {
    let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
}

// SAFETY: forwards each call to `System` unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        noted(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        noted(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        noted(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// `call`'s value and its largest one allocation, in bytes.
pub fn largest<T>(call: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(0));
    let out = call();
    (out, LARGEST.with(Cell::get))
}
