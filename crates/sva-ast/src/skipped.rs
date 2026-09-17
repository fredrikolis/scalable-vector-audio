// Concern: names a path a load found beside the nodes and read nothing from | Non-concern: finding one (dir.rs), reporting one (sva-cli's lint.rs) | IO: none

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub path: String,
    pub reason: Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// No ref can spell the path, so nothing can name what it holds.
    Unnameable,
    /// A socket, FIFO or device file, which holds no text to read at all.
    Special,
}
