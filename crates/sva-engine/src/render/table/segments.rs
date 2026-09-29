// Concern: a set of whole-sample intervals, kept disjoint, sorted and merged | Non-concern: what the samples hold or who asked for them | IO: (Extent) -> Segments

use sva_samples::Extent;

/// Disjoint, sorted intervals; two that touch are one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Segments(Vec<Extent>);

impl Segments {
    pub(crate) fn of(e: Extent) -> Segments {
        let mut out = Segments::default();
        out.add(e);
        out
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Extent> + '_ {
        self.0.iter().copied()
    }

    pub(crate) fn add(&mut self, e: Extent) {
        if e.is_empty() {
            return;
        }
        let (mut start, mut end) = (e.start, e.end);
        let first = self.0.partition_point(|held| held.end < start);
        let mut last = first;
        while last < self.0.len() && self.0[last].start <= end {
            start = start.min(self.0[last].start);
            end = end.max(self.0[last].end);
            last += 1;
        }
        self.0.splice(first..last, [Extent::new(start, end)]);
    }

    pub(crate) fn union(&mut self, other: &Segments) {
        for e in other.iter() {
            self.add(e);
        }
    }

    pub(crate) fn intersect(&self, e: Extent) -> Segments {
        Segments(
            self.0
                .iter()
                .map(|held| held.intersect(e))
                .filter(|held| !held.is_empty())
                .collect(),
        )
    }

    pub(crate) fn minus(&self, other: &Segments) -> Segments {
        let mut out = Vec::new();
        for held in self.iter() {
            let mut from = held.start;
            for gone in other
                .iter()
                .filter(|g| g.end > held.start && g.start < held.end)
            {
                if gone.start > from {
                    out.push(Extent::new(from, gone.start));
                }
                from = from.max(gone.end);
            }
            if from < held.end {
                out.push(Extent::new(from, held.end));
            }
        }
        Segments(out)
    }

    pub(crate) fn covers(&self, other: &Segments) -> bool {
        other.minus(self).is_empty()
    }

    pub(crate) fn hull(&self) -> Extent {
        match (self.0.first(), self.0.last()) {
            (Some(first), Some(last)) => Extent::new(first.start, last.end),
            _ => Extent::NOWHERE,
        }
    }

    /// Each sample moved into `[0, period)`: a periodic value is sample `k mod period`.
    pub(crate) fn folded(&self, period: i64) -> Segments {
        let mut out = Segments::default();
        for e in self.iter() {
            let whole = i128::from(e.end) - i128::from(e.start) >= i128::from(period);
            if whole || !e.is_bounded() {
                return Segments::of(Extent::new(0, period));
            }
            let (from, to) = (e.start.rem_euclid(period), e.end.rem_euclid(period));
            match from < to || to == 0 {
                true => out.add(Extent::new(from, if to == 0 { period } else { to })),
                false => {
                    out.add(Extent::new(from, period));
                    out.add(Extent::new(0, to));
                }
            }
        }
        out
    }
}
