// Concern: the latest entries of a log that may run for hours, and how many it made in all | Non-concern: what an entry is | IO: (entry) -> its place; () -> the latest, the count

use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub(crate) struct Recent<T> {
    kept: usize,
    shed: usize,
    items: VecDeque<T>,
}

impl<T> Recent<T> {
    pub(crate) fn keeping(kept: usize) -> Recent<T> {
        Recent {
            kept,
            shed: 0,
            items: VecDeque::new(),
        }
    }

    /// Its place among every entry made.
    pub(crate) fn push(&mut self, item: T) -> usize {
        self.items.push_back(item);
        if self.items.len() > self.kept {
            self.items.pop_front();
            self.shed += 1;
        }
        self.made() - 1
    }

    /// `None` where the entry at `at` was shed.
    pub(crate) fn get_mut(&mut self, at: usize) -> Option<&mut T> {
        self.items.get_mut(at.checked_sub(self.shed)?)
    }

    pub(crate) fn made(&self) -> usize {
        self.shed + self.items.len()
    }

    pub(crate) fn shed(&self) -> usize {
        self.shed
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }
}
