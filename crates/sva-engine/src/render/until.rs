// Concern: the condition a render stops at, where it provably holds and the first sample it does | Non-concern: parsing it, the tail proof (silent/) | IO: (Until, samples, a bound) -> a sample

use std::fmt;

use crate::query::DEFAULT_FRAME_SECS;

/// Comparisons over `t`, the measured level and its extremes, joined by `and` and `or`.
#[derive(Clone, Debug, PartialEq)]
pub enum Until {
    Holds(Term, Cmp, Term),
    All(Box<Until>, Box<Until>),
    Any(Box<Until>, Box<Until>),
}

/// `t` and a number are seconds; a level is linear amplitude, the RMS of the `envelope`
/// representation's frames.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Time,
    Number(f64),
    Envelope(At),
    Max(At, At),
    Min(At, At),
}

/// An instant: `t` plus seconds, a fixed time, or no end at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum At {
    Now(f64),
    Secs(f64),
    Inf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn flipped(self) -> Cmp {
        match self {
            Cmp::Lt => Cmp::Gt,
            Cmp::Le => Cmp::Ge,
            Cmp::Gt => Cmp::Lt,
            Cmp::Ge => Cmp::Le,
        }
    }

    /// Holds for every value either interval may take.
    fn surely(self, a: (f64, f64), b: (f64, f64)) -> bool {
        match self {
            Cmp::Lt => a.1 < b.0,
            Cmp::Le => a.1 <= b.0,
            Cmp::Gt => a.0 > b.1,
            Cmp::Ge => a.0 >= b.1,
        }
    }
}

impl Term {
    fn is_level(&self) -> bool {
        matches!(self, Term::Envelope(_) | Term::Max(..) | Term::Min(..))
    }

    fn offsets(&self, out: &mut Vec<f64>) {
        let mut at = |a: &At| {
            if let At::Now(o) = a {
                out.push(*o);
            }
        };
        match self {
            Term::Envelope(a) => at(a),
            Term::Max(a, b) | Term::Min(a, b) => {
                at(a);
                at(b);
            }
            Term::Time | Term::Number(_) => {}
        }
    }

    /// Whether the term's value moves with `t` only by the frames it reads.
    fn anchored_on_now(&self) -> bool {
        match self {
            Term::Envelope(a) => matches!(a, At::Now(_)),
            Term::Max(a, b) | Term::Min(a, b) => {
                matches!(a, At::Now(_)) && matches!(b, At::Now(_) | At::Inf)
            }
            Term::Time | Term::Number(_) => true,
        }
    }
}

impl Until {
    /// `max(envelope([t, inf))) < level`: every frame from here on under `level`.
    pub fn quiet(level: f64) -> Until {
        Until::Holds(
            Term::Max(At::Now(0.0), At::Inf),
            Cmp::Lt,
            Term::Number(level),
        )
    }

    /// A time compares with a number of seconds and a level with a level or a number; an
    /// unbounded instant only ends an interval.
    pub fn checked(&self) -> Result<(), String> {
        match self {
            Until::All(a, b) | Until::Any(a, b) => {
                a.checked()?;
                b.checked()
            }
            Until::Holds(a, _, b) => {
                for term in [a, b] {
                    let open = match term {
                        Term::Envelope(at) | Term::Max(at, _) | Term::Min(at, _) => *at == At::Inf,
                        _ => false,
                    };
                    if open {
                        return Err(format!("`{term}` reads an instant at inf"));
                    }
                }
                match (a, b) {
                    (Term::Time, Term::Time) => Err("`t` compares with a number".to_string()),
                    (Term::Time, other) | (other, Term::Time) if other.is_level() => {
                        Err(format!("`t` is a time and `{other}` a level"))
                    }
                    _ => Ok(()),
                }
            }
        }
    }

    /// Each level a `level < number` comparison names, which the tail proof can bring about.
    pub(crate) fn levels(&self, out: &mut Vec<f64>) {
        match self {
            Until::All(a, b) | Until::Any(a, b) => {
                a.levels(out);
                b.levels(out);
            }
            Until::Holds(a, cmp, b) => {
                if let Some(level) = proved_below(a, *cmp, b) {
                    out.push(level);
                }
            }
        }
    }

    fn offsets(&self, out: &mut Vec<f64>) {
        match self {
            Until::All(a, b) | Until::Any(a, b) => {
                a.offsets(out);
                b.offsets(out);
            }
            Until::Holds(a, _, b) => {
                a.offsets(out);
                b.offsets(out);
            }
        }
    }

    fn times(&self, out: &mut Vec<f64>) {
        match self {
            Until::All(a, b) | Until::Any(a, b) => {
                a.times(out);
                b.times(out);
            }
            Until::Holds(Term::Time, _, Term::Number(v))
            | Until::Holds(Term::Number(v), _, Term::Time) => out.push(*v),
            Until::Holds(..) => {}
        }
    }

    /// A sample at which this surely holds, from `start` on, and whether it holds at every
    /// sample after. `proven` answers the first sample every later one is under a level.
    pub(crate) fn provable(
        &self,
        start: i64,
        rate: u32,
        proven: &dyn Fn(f64) -> Option<i64>,
    ) -> Option<(i64, bool)> {
        let sr = f64::from(rate);
        match self {
            Until::All(a, b) => match (
                a.provable(start, rate, proven)?,
                b.provable(start, rate, proven)?,
            ) {
                ((x, true), (y, true)) => Some((x.max(y), true)),
                _ => None,
            },
            Until::Any(a, b) => {
                let (x, y) = (
                    a.provable(start, rate, proven),
                    b.provable(start, rate, proven),
                );
                match (x, y) {
                    (Some(x), Some(y)) => Some(if x.0 <= y.0 { x } else { y }),
                    (x, y) => x.or(y),
                }
            }
            Until::Holds(Term::Number(v), cmp, Term::Time) => {
                Until::Holds(Term::Time, cmp.flipped(), Term::Number(*v))
                    .provable(start, rate, proven)
            }
            Until::Holds(Term::Time, cmp, Term::Number(v)) => match cmp {
                Cmp::Gt | Cmp::Ge => Some((((v * sr).ceil() as i64 + 1).max(start), true)),
                Cmp::Lt | Cmp::Le => cmp
                    .surely(instant(start, rate), (*v, *v))
                    .then_some((start, false)),
            },
            Until::Holds(Term::Number(a), cmp, Term::Number(b)) => {
                cmp.surely((*a, *a), (*b, *b)).then_some((start, true))
            }
            Until::Holds(a, cmp, b) => {
                let level = proved_below(a, *cmp, b)?;
                let term = if a.is_level() { a } else { b };
                if !term.anchored_on_now() {
                    return None;
                }
                let quiet = proven(level)?;
                let frame = frame_len(rate) as i64;
                let aligned = start + ((quiet - start).max(0) + frame - 1) / frame * frame;
                let mut offsets = Vec::new();
                term.offsets(&mut offsets);
                let back = offsets
                    .iter()
                    .map(|o| (o * sr).round() as i64)
                    .min()
                    .unwrap_or(0)
                    .min(0);
                Some((aligned + frame - back, true))
            }
        }
    }

    pub(crate) fn reach(&self, rate: u32) -> usize {
        let mut offsets = vec![0.0];
        self.offsets(&mut offsets);
        let back = offsets
            .iter()
            .map(|o| (-o * f64::from(rate)).ceil().max(0.0) as usize)
            .max()
            .unwrap_or(0);
        back + 2 * frame_len(rate)
    }

    /// The first candidate in `[from, to]` at which this surely holds, over what `known` holds.
    pub(crate) fn first(&self, known: &Known, from: i64, to: i64) -> Option<i64> {
        let frame = known.frame as i64;
        let sr = f64::from(known.rate);
        let mut offsets = vec![0.0];
        self.offsets(&mut offsets);
        let mut candidates = vec![from];
        for o in offsets {
            let shift = (o * sr).round() as i64;
            let first = (from + shift - known.start).div_euclid(frame);
            let mut k = first;
            loop {
                let n = known.start + k * frame - shift;
                if n > to {
                    break;
                }
                if n >= from {
                    candidates.push(n);
                }
                k += 1;
            }
        }
        let mut times = Vec::new();
        self.times(&mut times);
        for v in times {
            let n = (v * sr).round() as i64;
            candidates.extend((n - 1..=n + 1).filter(|n| (from..=to).contains(n)));
        }
        candidates.sort_unstable();
        candidates.dedup();
        candidates.into_iter().find(|n| self.holds(known, *n))
    }

    fn holds(&self, known: &Known, n: i64) -> bool {
        match self {
            Until::All(a, b) => a.holds(known, n) && b.holds(known, n),
            Until::Any(a, b) => a.holds(known, n) || b.holds(known, n),
            Until::Holds(a, cmp, b) => cmp.surely(known.value(a, n), known.value(b, n)),
        }
    }
}

/// `level < number` or `number > level`: the number, which a proof under it satisfies.
fn proved_below(a: &Term, cmp: Cmp, b: &Term) -> Option<f64> {
    match (a, cmp, b) {
        (level, Cmp::Lt | Cmp::Le, Term::Number(v)) if level.is_level() => Some(*v),
        (Term::Number(v), Cmp::Gt | Cmp::Ge, level) if level.is_level() => Some(*v),
        _ => None,
    }
}

fn instant(n: i64, rate: u32) -> (f64, f64) {
    let t = n as f64 / f64::from(rate);
    (t, t)
}

pub(crate) fn frame_len(rate: u32) -> usize {
    ((DEFAULT_FRAME_SECS * f64::from(rate)).round() as usize).max(1)
}

/// The root's first channel from `start` on, framed from `start` as the `envelope`
/// representation frames it, and a bound on every sample past what it holds.
pub(crate) struct Known<'a> {
    pub(crate) plane: &'a [f64],
    pub(crate) base: i64,
    /// Where frames are counted from.
    pub(crate) start: i64,
    pub(crate) frame: usize,
    pub(crate) rate: u32,
    pub(crate) beyond: f64,
    /// Each frame's sum of squares and count held, summed frame by frame so a quiet frame
    /// after a loud one keeps its own digits.
    sums: Vec<(f64, usize)>,
    first_frame: i64,
}

impl<'a> Known<'a> {
    pub(crate) fn new(plane: &'a [f64], base: i64, start: i64, rate: u32, beyond: f64) -> Self {
        let frame = frame_len(rate) as i64;
        let first_frame = (base - start).div_euclid(frame);
        let mut sums: Vec<(f64, usize)> = Vec::new();
        for (i, v) in plane.iter().enumerate() {
            let f = (base + i as i64 - start).div_euclid(frame) - first_frame;
            if sums.len() <= f as usize {
                sums.resize(f as usize + 1, (0.0, 0));
            }
            let held = &mut sums[f as usize];
            held.0 += v * v;
            held.1 += 1;
        }
        Known {
            plane,
            base,
            start,
            frame: frame as usize,
            rate,
            beyond,
            sums,
            first_frame,
        }
    }

    fn end(&self) -> i64 {
        self.base + self.plane.len() as i64
    }

    /// One frame's RMS, as low and as high as the samples not held allow.
    fn frame_level(&self, f: i64) -> (f64, f64) {
        let frame = self.frame as i64;
        if self.start + f * frame < self.base {
            return (0.0, f64::INFINITY);
        }
        let (sum, held) = usize::try_from(f - self.first_frame)
            .ok()
            .and_then(|at| self.sums.get(at))
            .copied()
            .unwrap_or((0.0, 0));
        let missing = (self.frame - held) as f64;
        let rest = match missing {
            0.0 => 0.0,
            m => m * self.beyond * self.beyond,
        };
        let n = self.frame as f64;
        ((sum / n).sqrt(), ((sum + rest) / n).sqrt())
    }

    fn frame_of(&self, n: i64) -> i64 {
        (n - self.start).div_euclid(self.frame as i64)
    }

    fn sample(&self, at: At, n: i64) -> Option<i64> {
        let sr = f64::from(self.rate);
        match at {
            At::Now(o) => Some(n + (o * sr).round() as i64),
            At::Secs(s) => Some((s * sr).round() as i64),
            At::Inf => None,
        }
    }

    fn value(&self, term: &Term, n: i64) -> (f64, f64) {
        match term {
            Term::Time => instant(n, self.rate),
            Term::Number(v) => (*v, *v),
            Term::Envelope(at) => match self.sample(*at, n) {
                Some(m) => self.frame_level(self.frame_of(m)),
                None => (0.0, f64::INFINITY),
            },
            Term::Max(a, b) | Term::Min(a, b) => {
                let max = matches!(term, Term::Max(..));
                let Some(first) = self.sample(*a, n).map(|m| self.frame_of(m)) else {
                    return (0.0, f64::INFINITY);
                };
                let last_held = self.frame_of(self.end() - 1);
                let last = self.sample(*b, n).map(|m| self.frame_of(m));
                let mut held: Option<(f64, f64)> = None;
                let mut join = |v: (f64, f64)| {
                    held = Some(match held {
                        None => v,
                        Some(h) if max => (h.0.max(v.0), h.1.max(v.1)),
                        Some(h) => (h.0.min(v.0), h.1.min(v.1)),
                    });
                };
                let upto = last.unwrap_or(last_held).min(last_held);
                for f in first..=upto {
                    join(self.frame_level(f));
                }
                if last.is_none_or(|l| l > last_held) {
                    join((0.0, self.beyond));
                }
                held.unwrap_or((0.0, 0.0))
            }
        }
    }
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            At::Now(o) if *o == 0.0 => write!(f, "t"),
            At::Now(o) if *o < 0.0 => write!(f, "t - {}s", -o),
            At::Now(o) => write!(f, "t + {o}s"),
            At::Secs(s) => write!(f, "{s}s"),
            At::Inf => write!(f, "inf"),
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let span = |a: &At, b: &At| match b {
            At::Inf => format!("[{a}, inf)"),
            b => format!("[{a}, {b}]"),
        };
        match self {
            Term::Time => write!(f, "t"),
            Term::Number(v) => write!(f, "{v}"),
            Term::Envelope(at) => write!(f, "envelope({at})"),
            Term::Max(a, b) => write!(f, "max(envelope({}))", span(a, b)),
            Term::Min(a, b) => write!(f, "min(envelope({}))", span(a, b)),
        }
    }
}

/// A level beside a number prints the number in dB, a time beside one in seconds.
impl fmt::Display for Until {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |term: &Term, other: &Term| match (term, other) {
            (Term::Number(v), level) if level.is_level() => {
                format!("{:.1}db", 20.0 * v.log10())
            }
            (Term::Number(v), Term::Time) => format!("{v}s"),
            (term, _) => term.to_string(),
        };
        match self {
            Until::All(a, b) => write!(f, "({a} and {b})"),
            Until::Any(a, b) => write!(f, "({a} or {b})"),
            Until::Holds(a, cmp, b) => {
                let op = match cmp {
                    Cmp::Lt => "<",
                    Cmp::Le => "<=",
                    Cmp::Gt => ">",
                    Cmp::Ge => ">=",
                };
                write!(f, "{} {op} {}", side(a, b), side(b, a))
            }
        }
    }
}
