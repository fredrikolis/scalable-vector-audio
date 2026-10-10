// Concern: the smallest voice leading between each two successive chords, in pitch space | Non-concern: finding the chords (chords.rs) | IO: (chords, norm) -> VoiceLeading

use super::Framing;
use super::chords::Chord;

pub const MODEL: &str = "Tymoczko, D. (2006), The Geometry of Musical Chords, Science 313 \
                         72-74, and A Geometry of Music (2011) ch. 2: the smallest voice \
                         leading in pitch space, a note doubled where sizes differ so every \
                         note of both chords moves to or from one of the other, its size the \
                         L1 or L2 norm of the moves in semitones";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm {
    L1,
    L2,
}

impl Norm {
    pub fn key(self) -> &'static str {
        match self {
            Norm::L1 => "l1",
            Norm::L2 => "l2",
        }
    }

    fn cost(self, from: i64, to: i64) -> f64 {
        let d = (from - to).abs() as f64;
        match self {
            Norm::L1 => d,
            Norm::L2 => d * d,
        }
    }

    fn size(self, summed: f64) -> f64 {
        match self {
            Norm::L1 => summed,
            Norm::L2 => summed.sqrt(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub at_secs: f64,
    pub from: Vec<i64>,
    pub to: Vec<i64>,
    pub distance: f64,
    pub mapping: Vec<(i64, i64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceLeading {
    pub framing: Framing,
    pub floor_db: f64,
    pub norm: Norm,
    pub chords: Vec<Chord>,
    pub transitions: Vec<Transition>,
    pub total: f64,
}

/// An edge cover of two ascending chords: crossing moves uncross at no greater convex cost
/// with the same notes covered (the cost is Monge), so a smallest cover runs in order along
/// both, and this dynamic program over them finds one exactly.
pub fn lead(from: &[i64], to: &[i64], norm: Norm) -> (f64, Vec<(i64, i64)>) {
    let (m, n) = (from.len(), to.len());
    let mut best = vec![vec![f64::INFINITY; n]; m];
    for i in 0..m {
        for j in 0..n {
            let before = match (i, j) {
                (0, 0) => 0.0,
                (0, _) => best[0][j - 1],
                (_, 0) => best[i - 1][0],
                _ => best[i - 1][j - 1].min(best[i - 1][j]).min(best[i][j - 1]),
            };
            best[i][j] = before + norm.cost(from[i], to[j]);
        }
    }
    let (mut i, mut j) = (m - 1, n - 1);
    let mut mapping = vec![(from[i], to[j])];
    while (i, j) != (0, 0) {
        (i, j) = match (i, j) {
            (0, _) => (0, j - 1),
            (_, 0) => (i - 1, 0),
            _ => {
                let diagonal = best[i - 1][j - 1];
                if diagonal <= best[i - 1][j] && diagonal <= best[i][j - 1] {
                    (i - 1, j - 1)
                } else if best[i - 1][j] <= best[i][j - 1] {
                    (i - 1, j)
                } else {
                    (i, j - 1)
                }
            }
        };
        mapping.push((from[i], to[j]));
    }
    mapping.reverse();
    (norm.size(best[m - 1][n - 1]), mapping)
}

pub fn analyze(chords: Vec<Chord>, framing: Framing, floor_db: f64, norm: Norm) -> VoiceLeading {
    let transitions: Vec<Transition> = chords
        .windows(2)
        .map(|pair| {
            let (distance, mapping) = lead(&pair[0].notes, &pair[1].notes, norm);
            Transition {
                at_secs: pair[1].start_secs,
                from: pair[0].notes.clone(),
                to: pair[1].notes.clone(),
                distance,
                mapping,
            }
        })
        .collect();
    VoiceLeading {
        framing,
        floor_db,
        norm,
        total: transitions.iter().map(|t| t.distance).sum(),
        chords,
        transitions,
    }
}
