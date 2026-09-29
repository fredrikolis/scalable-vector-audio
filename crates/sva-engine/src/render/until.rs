// Concern: the condition a render stops at, and the first sample it holds at | Non-concern: parsing it, where a render's extent ends (reach.rs) | IO: (Until, the root's samples) -> a sample

use std::fmt;

/// Comparisons over `t` and the measured level, joined by `and` and `or`.
#[derive(Clone, Debug, PartialEq)]
pub enum Until {
    Holds(Term, Cmp, Term),
    All(Box<Until>, Box<Until>),
    Any(Box<Until>, Box<Until>),
}

/// `t` and a number are seconds; `envelope(t)` is the RMS of the `envelope` frame holding `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Term {
    Time,
    Number(f64),
    Envelope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn holds(self, a: f64, b: f64) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }
}

impl Until {
    pub fn checked(&self) -> Result<(), String> {
        match self {
            Until::All(a, b) | Until::Any(a, b) => {
                a.checked()?;
                b.checked()
            }
            Until::Holds(Term::Time, _, Term::Time) => Err("`t` compares with a number".into()),
            Until::Holds(Term::Time, _, Term::Envelope)
            | Until::Holds(Term::Envelope, _, Term::Time) => {
                Err("`t` is a time and `envelope(t)` a level".into())
            }
            Until::Holds(..) => Ok(()),
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

    /// A level is known only once its whole frame is.
    pub(crate) fn first(&self, known: &Known, from: i64, to: i64) -> Option<i64> {
        let frame = known.frame as i64;
        let first_frame = (from - known.start).div_euclid(frame);
        let mut candidates: Vec<i64> = (first_frame..)
            .map(|k| known.start + k * frame)
            .take_while(|n| *n < to)
            .map(|n| n.max(from))
            .collect();
        let mut times = Vec::new();
        self.times(&mut times);
        let sr = f64::from(known.rate);
        for v in times {
            let n = (v * sr).round() as i64;
            candidates.extend((n - 1..=n + 1).filter(|n| (from..to).contains(n)));
        }
        candidates.sort_unstable();
        candidates.dedup();
        candidates
            .into_iter()
            .find(|n| self.holds(known, *n) == Some(true))
    }

    fn holds(&self, known: &Known, n: i64) -> Option<bool> {
        match self {
            Until::All(a, b) => match (a.holds(known, n), b.holds(known, n)) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            Until::Any(a, b) => match (a.holds(known, n), b.holds(known, n)) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            Until::Holds(a, cmp, b) => Some(cmp.holds(known.value(*a, n)?, known.value(*b, n)?)),
        }
    }
}

/// The root's channels from `base` on, framed from `start` as the `envelope`
/// representation frames it; `ended` says no sample follows the last one held.
pub(crate) struct Known<'a> {
    planes: Vec<&'a [f64]>,
    base: i64,
    start: i64,
    frame: usize,
    rate: u32,
    ended: bool,
}

impl<'a> Known<'a> {
    pub(crate) fn new(
        planes: Vec<&'a [f64]>,
        base: i64,
        start: i64,
        frame: usize,
        rate: u32,
        ended: bool,
    ) -> Self {
        Known {
            planes,
            base,
            start,
            frame: frame.max(1),
            rate,
            ended,
        }
    }

    /// A frame the render ends inside is as long as what it holds, as in `envelope`.
    fn frame_level(&self, n: i64) -> Option<f64> {
        let frame = self.frame as i64;
        let from = self.start + (n - self.start).div_euclid(frame) * frame;
        let end = self.base + self.planes.iter().map(|p| p.len()).min().unwrap_or(0) as i64;
        if from < self.base {
            return None;
        }
        let to = match from + frame <= end {
            true => from + frame,
            false if self.ended => end,
            false => return None,
        };
        let held: Vec<&[f64]> = self
            .planes
            .iter()
            .map(|p| &p[(from - self.base) as usize..(to - self.base) as usize])
            .collect();
        Some(sva_samples::measure::envelope::level(&held))
    }

    fn value(&self, term: Term, n: i64) -> Option<f64> {
        match term {
            Term::Time => Some(n as f64 / f64::from(self.rate)),
            Term::Number(v) => Some(v),
            Term::Envelope => self.frame_level(n),
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Time => write!(f, "t"),
            Term::Number(v) => write!(f, "{v}"),
            Term::Envelope => write!(f, "envelope(t)"),
        }
    }
}

/// A level beside a number prints the number in dB, a time beside one in seconds.
impl fmt::Display for Until {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |term: &Term, other: &Term| match (term, other) {
            (Term::Number(v), Term::Envelope) => format!("{:.1}db", 20.0 * v.log10()),
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
