// Concern: the fewest neo-Riemannian steps between successive triads, and each from the tonic | Non-concern: finding the chords (chords.rs) | IO: (chords) -> TonalDistance

use std::collections::VecDeque;

use super::Framing;
use super::chords::Chord;

pub const MODEL: &str = "Tonnetz (Euler 1739, Riemann): the 24 major and minor triads, two \
                         adjacent where one of P, L, R maps one to the other (Cohn, R. (1998), \
                         Introduction to Neo-Riemannian Theory, J. Music Theory 42(2) \
                         167-180): P keeps root and fifth (C-c), R the major triad's root and \
                         third (C-a), L its third and fifth (C-e); distance is the fewest steps. \
                         A chord whose pitch classes are no major or minor triad has no place on \
                         it, and its steps and tension are null";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Major,
    Minor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Triad {
    pub root: u8,
    pub quality: Quality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    P,
    L,
    R,
}

const STEPS: [Step; 3] = [Step::P, Step::L, Step::R];

impl Step {
    pub fn letter(self) -> char {
        match self {
            Step::P => 'P',
            Step::L => 'L',
            Step::R => 'R',
        }
    }

    pub fn of(self, t: Triad) -> Triad {
        let (shift, quality) = match (self, t.quality) {
            (Step::P, Quality::Major) => (0, Quality::Minor),
            (Step::P, Quality::Minor) => (0, Quality::Major),
            (Step::R, Quality::Major) => (9, Quality::Minor),
            (Step::R, Quality::Minor) => (3, Quality::Major),
            (Step::L, Quality::Major) => (4, Quality::Minor),
            (Step::L, Quality::Minor) => (8, Quality::Major),
        };
        Triad {
            root: (t.root + shift) % 12,
            quality,
        }
    }
}

impl Triad {
    fn index(self) -> usize {
        usize::from(self.root) * 2 + usize::from(self.quality == Quality::Minor)
    }

    pub fn of(notes: &[i64]) -> Option<Triad> {
        let mut classes: Vec<u8> = notes.iter().map(|n| n.rem_euclid(12) as u8).collect();
        classes.sort_unstable();
        classes.dedup();
        if classes.len() != 3 {
            return None;
        }
        let holds = |root: u8, third: u8| {
            let mut want = [root, (root + third) % 12, (root + 7) % 12];
            want.sort_unstable();
            want[..] == classes[..]
        };
        classes.iter().find_map(|&root| {
            [(4, Quality::Major), (3, Quality::Minor)]
                .into_iter()
                .find(|(third, _)| holds(root, *third))
                .map(|(_, quality)| Triad { root, quality })
        })
    }
}

/// Breadth first in P, L, R order.
pub fn path(from: Triad, to: Triad) -> Vec<Step> {
    let mut came: [Option<(Triad, Step)>; 24] = [None; 24];
    let mut seen = [false; 24];
    seen[from.index()] = true;
    let mut queue = VecDeque::from([from]);
    while let Some(at) = queue.pop_front() {
        if at == to {
            break;
        }
        for step in STEPS {
            let next = step.of(at);
            if !seen[next.index()] {
                seen[next.index()] = true;
                came[next.index()] = Some((at, step));
                queue.push_back(next);
            }
        }
    }
    let mut steps = Vec::new();
    let mut at = to;
    while let Some((before, step)) = came[at.index()] {
        steps.push(step);
        at = before;
    }
    steps.reverse();
    steps
}

#[derive(Clone, Debug, PartialEq)]
pub struct TonalChord {
    pub chord: Chord,
    pub triad: Option<Triad>,
    /// Steps from the tonic; `None` where this chord or the tonic is no triad.
    pub tension: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TonalTransition {
    pub at_secs: f64,
    pub from: Option<Triad>,
    pub to: Option<Triad>,
    pub path: Option<Vec<Step>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TonalDistance {
    pub framing: Framing,
    pub floor_db: f64,
    /// The first chord that is a triad, and its start.
    pub tonic: Option<(Triad, f64)>,
    pub chords: Vec<TonalChord>,
    pub transitions: Vec<TonalTransition>,
}

pub fn analyze(chords: Vec<Chord>, framing: Framing, floor_db: f64) -> TonalDistance {
    let triads: Vec<Option<Triad>> = chords.iter().map(|c| Triad::of(&c.notes)).collect();
    let tonic = chords
        .iter()
        .zip(&triads)
        .find_map(|(c, t)| t.map(|t| (t, c.start_secs)));
    let transitions = chords
        .windows(2)
        .zip(triads.windows(2))
        .map(|(c, t)| TonalTransition {
            at_secs: c[1].start_secs,
            from: t[0],
            to: t[1],
            path: t[0].zip(t[1]).map(|(a, b)| path(a, b)),
        })
        .collect();
    let chords = chords
        .into_iter()
        .zip(triads)
        .map(|(chord, triad)| TonalChord {
            tension: tonic.zip(triad).map(|((home, _), t)| path(home, t).len()),
            chord,
            triad,
        })
        .collect();
    TonalDistance {
        framing,
        floor_db,
        tonic,
        chords,
        transitions,
    }
}
