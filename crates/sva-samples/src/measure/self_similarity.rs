// Concern: how a target's stretches resemble one another: chroma and band-level matrices, stripes, novelty | Non-concern: picking partials, band filters, verdicts | IO: (&[f64], Asked) -> SelfSimilarity

use super::bands;
use super::harmony::{above, measured};
use super::pitch::{HIGHEST_HZ, LOWEST_HZ, midi_of};
use super::sliding::{self, Span};

/// The most cells one side of the matrix holds: its lower triangle is 524 800 entries.
pub const MAX_CELLS: usize = 1024;
const CLASSES: usize = 12;

pub const MODEL: &str = "self-similarity matrix of Foote, J. (1999), Visualizing music and \
                         audio using self-similarity, Proc. ACM Multimedia 77-80, read for \
                         repetition as Bartsch, M. A. and Wakefield, G. H. (2001), To catch a \
                         chorus: using chroma-based representations for audio thumbnailing, \
                         Proc. IEEE WASPAA 15-18, and for boundaries as Foote, J. (2000), \
                         Automatic audio segmentation using a measure of audio novelty, Proc. \
                         IEEE ICME 452-455, each step as Mueller, M. (2015), Fundamentals of \
                         Music Processing, Springer, ch. 4 (FMP) writes it. Frames: a Hann \
                         frame of `frame_window_secs` of the first component centred every \
                         `hop_secs` from the target's start, its partials picked and refined \
                         as the harmonic readings pick them (see their method), \
                         `resolution_hz` apart before refinement, so two partials within about \
                         that of each other, as two notes a semitone apart under some 17 \
                         times `resolution_hz`, are not resolved. \
                         Chroma view: per frame each partial at or above `floor_db` from \
                         `min_hz` to `max_hz` adds its squared amplitude to the pitch class \
                         of its nearest equal-tempered note (A4 = 440 Hz), C first; \
                         harmonics are not removed, so a tone's third harmonic adds to its \
                         fifth's class, as Bartsch and Wakefield's chroma does. Timbre view: \
                         the RMS of each ERB band of `bands` (`centres_hz`), each band's level \
                         in dB above `floor_db`, zero under it. Cells: `frames_per_cell` \
                         frames, `cell_secs`, centred every `cell_secs` from the start as \
                         every framed reading steps, `from`/`to` the first frame centre in it \
                         and the next cell's, the first clipped at the target's start, and \
                         frames past the last cell's `to`, under half a cell, in none; a \
                         cell's chroma is its frames' mean, its band levels those of its \
                         bands' mean energy over that span. similarity(i, j): the cosine of \
                         cells i and j's vectors, in [0, 1] as no entry is negative, and 0 \
                         where either vector is all zero (silence, or nothing over the floor), \
                         itself included. matrix, where asked: row i lists \
                         similarity(i, i - lag) for lag 0 to i, the lower triangle in \
                         time-lag form; it is symmetric. Stripes: a repeat is a stripe \
                         parallel to the diagonal. Each diagonal is first averaged along its \
                         length over `smooth_cells` cells centred on each, over the cells it \
                         holds (FMP 4.2.2's path enhancement), so a held chord's square block \
                         does not hide which lag repeats; a cell is on a stripe where that \
                         average exceeds the one at one cell less lag and is no lower than \
                         the one at one cell more, in its row (a missing neighbour counts as \
                         lower); a stripe is a run of such cells at one lag, `cells` long, \
                         `later` repeating `earlier` `lag_secs` before it. Lags from one \
                         cell; a repeat under two cells away merges with the diagonal and is \
                         not read. A stripe's ends stand within half `smooth_cells` of where \
                         the repeat's do. `similarity_mean`/`_min` are the raw similarity along \
                         it, unaveraged. Transposition, chroma view: the same search runs on \
                         the matrix of each cell against each earlier cell's chroma rotated up \
                         `shift_semitones` (Goto, M. (2003), A chorus-section detecting \
                         method for musical audio signals, Proc. IEEE ICASSP 437-440; FMP \
                         4.2.2), shifts -5 to 6, so an exact repeat reads at shift 0 and one \
                         transposed up two semitones at shift 2; a shift is mod 12, so a \
                         repeat an octave up reads at 0, and a sustained glide reads as \
                         transposed repeats of itself at every shift. A symmetric chord (an \
                         augmented triad at 4, a diminished seventh at 3) reads at more than \
                         one shift. The matrix lists shift 0 alone; the timbre view has no \
                         shift. No threshold: every stripe is counted in `stripes_found`, \
                         the `listed` with the largest summed similarity listed, largest \
                         first, so a short weak stripe from noise reads with its small sum, \
                         never dropped unasked. Novelty: per boundary between two adjacent \
                         cells, sum over a and b of K(a, b) similarity(i + a, i + b), a and b \
                         from -L to L - 1 cells, L = `kernel_cells`, K Foote's checkerboard \
                         sign(u_a) sign(u_b) tapered by a Gaussian exp(-u^2 / (2 L^2)) at \
                         u = a + 1/2 (FMP's default variance), normalized so sum |K| = 1, \
                         cells past the target zero (so a boundary near either end reads \
                         against silence); in [-0.5, 0.5], 0.5 for a cut between two \
                         blocks each wholly alike within and wholly unlike across. `values` \
                         from `from_secs` by `cell_secs`; peaks: those over the one before \
                         and no lower than the one after, the `listed` highest, highest \
                         first, every one counted in `peaks_found`. Exact path: none; a \
                         closed form is read off its rendered samples, because a frame's \
                         partials over a floor and a band's energy over a span are stated by \
                         no closed form";

/// Each reading's arguments: the frame `hop_secs`, the matrix's `cell_secs`, the stripe
/// average's `smooth_secs`, the novelty kernel's whole `window_secs`, the floor, how many
/// stripes and peaks are listed, and whether the cells and matrices are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Asked {
    pub hop_secs: f64,
    pub cell_secs: f64,
    pub smooth_secs: f64,
    pub window_secs: f64,
    pub floor_db: f64,
    pub listed: usize,
    pub curve: bool,
}

/// `least_cell_secs` is the finest cell that fits the cap; `most_secs` the longest target
/// the cell asked fits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooManyCells {
    pub cells: usize,
    pub cap: usize,
    pub cell_secs: f64,
    pub held_secs: f64,
    pub least_cell_secs: f64,
    pub most_secs: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Chroma,
    Timbre,
}

impl View {
    pub fn key(self) -> &'static str {
        match self {
            View::Chroma => "chroma",
            View::Timbre => "timbre",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub t_secs: f64,
    pub from_secs: f64,
    pub to_secs: f64,
    /// Mean squared amplitude per frame, C to B.
    pub chroma: [f64; CLASSES],
    /// dB against full scale, one per band in `centres_hz`.
    pub levels_db: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stripe {
    pub shift_semitones: i32,
    pub lag_secs: f64,
    pub earlier: (f64, f64),
    pub later: (f64, f64),
    pub cells: usize,
    pub similarity_mean: f64,
    pub similarity_min: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub t_secs: f64,
    pub novelty: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub view: View,
    pub stripes_found: usize,
    /// Largest summed similarity first.
    pub stripes: Vec<Stripe>,
    /// One per boundary, the first at `novelty_from_secs`, `cell_secs` apart.
    pub novelty: Vec<f64>,
    pub peaks_found: usize,
    /// Highest first.
    pub peaks: Vec<Peak>,
    /// Row i holds lags 0 to i; listed where asked.
    pub matrix: Option<Vec<Vec<f64>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelfSimilarity {
    pub hop_secs: f64,
    pub frame_window_secs: f64,
    pub resolution_hz: f64,
    pub cell_secs: f64,
    pub frames_per_cell: usize,
    pub cell_count: usize,
    pub smooth_cells: usize,
    pub kernel_cells: usize,
    pub floor_db: f64,
    pub listed: usize,
    pub min_hz: f64,
    pub max_hz: f64,
    pub centres_hz: Vec<f64>,
    pub novelty_from_secs: f64,
    /// Listed where asked.
    pub cells: Option<Vec<Cell>>,
    pub views: Vec<Reading>,
}

pub fn measure(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    asked: Asked,
) -> Result<SelfSimilarity, TooManyCells> {
    let frame_hop = ((asked.hop_secs * sample_rate).round() as usize).max(1);
    let per_cell = ((asked.cell_secs * sample_rate / frame_hop as f64).round() as usize).max(1);
    let cell_hop = per_cell * frame_hop;
    let cell_secs = cell_hop as f64 / sample_rate;
    let count = samples.len().div_ceil(cell_hop);
    if count > MAX_CELLS {
        let least = samples.len().div_ceil(MAX_CELLS * frame_hop) * frame_hop;
        return Err(TooManyCells {
            cells: count,
            cap: MAX_CELLS,
            cell_secs,
            held_secs: samples.len() as f64 / sample_rate,
            least_cell_secs: least as f64 / sample_rate,
            most_secs: (MAX_CELLS * cell_hop) as f64 / sample_rate,
        });
    }
    let (moments, framing) = measured::moments(
        samples,
        sample_rate,
        start_secs,
        frame_hop as f64 / sample_rate,
    );
    let window = framing.window.expect("measured frames have a window");
    let chroma: Vec<[f64; CLASSES]> = moments
        .iter()
        .map(|m| {
            let mut held = [0.0; CLASSES];
            for p in above(m, asked.floor_db) {
                if (LOWEST_HZ..=HIGHEST_HZ).contains(&p.hz) {
                    held[(midi_of(p.hz).round() as i64).rem_euclid(12) as usize] +=
                        p.amplitude * p.amplitude;
                }
            }
            held
        })
        .collect();
    let split = bands::analyze(samples, sample_rate, start_secs);
    let grid = |rate: f64, len: usize| {
        sliding::spans(
            samples.len(),
            sample_rate,
            start_secs,
            cell_secs,
            rate,
            cell_secs,
            len,
        )
        .spans
    };
    let framed = grid(sample_rate / frame_hop as f64, chroma.len());
    let banded = grid(
        split.rate_hz,
        split.bands.first().map_or(0, |b| b.rms.len()),
    );
    let cells: Vec<Cell> = framed
        .iter()
        .zip(&banded)
        .map(|(f, b)| cell(f, b, &chroma, &split))
        .collect();
    let smooth_cells = ((asked.smooth_secs / cell_secs).round() as usize).max(1);
    let kernel_cells = ((asked.window_secs / (2.0 * cell_secs)).round() as usize).max(1);
    let read = Read {
        cells: &cells,
        cell_secs,
        smooth_cells,
        kernel_cells,
        listed: asked.listed,
        curve: asked.curve,
    };
    let chroma_of = |c: &Cell| c.chroma.to_vec();
    let timbre_of = |c: &Cell| {
        c.levels_db
            .iter()
            .map(|l| (l - asked.floor_db).max(0.0))
            .collect()
    };
    Ok(SelfSimilarity {
        hop_secs: framing.hop_secs,
        frame_window_secs: window.secs,
        resolution_hz: window.resolution_hz,
        cell_secs,
        frames_per_cell: per_cell,
        cell_count: cells.len(),
        smooth_cells,
        kernel_cells,
        floor_db: asked.floor_db,
        listed: asked.listed,
        min_hz: LOWEST_HZ,
        max_hz: HIGHEST_HZ,
        centres_hz: split.bands.iter().map(|b| b.centre_hz).collect(),
        novelty_from_secs: start_secs + 0.5 * cell_secs,
        views: vec![
            read.view(View::Chroma, chroma_of, -5..=6),
            read.view(View::Timbre, timbre_of, 0..=0),
        ],
        cells: asked.curve.then_some(cells),
    })
}

fn cell(f: &Span, b: &Span, chroma: &[[f64; CLASSES]], split: &bands::Bands) -> Cell {
    let frames = &chroma[f.from..f.to];
    let mut mean = [0.0; CLASSES];
    for frame in frames {
        for (m, e) in mean.iter_mut().zip(frame) {
            *m += e / frames.len() as f64;
        }
    }
    let levels_db = split
        .bands
        .iter()
        .map(|band| {
            let held = &band.rms[b.from..b.to];
            let energy = held.iter().map(|r| r * r).sum::<f64>() / held.len() as f64;
            10.0 * energy.log10()
        })
        .collect();
    Cell {
        t_secs: f.t_secs,
        from_secs: f.from_secs,
        to_secs: f.to_secs,
        chroma: mean,
        levels_db,
    }
}

struct Read<'a> {
    cells: &'a [Cell],
    cell_secs: f64,
    smooth_cells: usize,
    kernel_cells: usize,
    listed: usize,
    curve: bool,
}

impl Read<'_> {
    /// `shifts` rotate each earlier cell's vector, which only chroma's twelve classes mean.
    fn view(
        &self,
        view: View,
        vector: impl Fn(&Cell) -> Vec<f64>,
        shifts: std::ops::RangeInclusive<i32>,
    ) -> Reading {
        let vectors: Vec<Vec<f64>> = self.cells.iter().map(vector).collect();
        let norms: Vec<f64> = vectors
            .iter()
            .map(|v| v.iter().map(|x| x * x).sum::<f64>().sqrt())
            .collect();
        let mut raw = Vec::new();
        let mut stripes = Vec::new();
        for shift in shifts {
            let k = shift.rem_euclid(CLASSES as i32) as usize;
            let rotated = |j: usize, p: usize| {
                let v = &vectors[j];
                v[(p + v.len() - k % v.len()) % v.len()]
            };
            let held = diagonals(self.cells.len(), |i, j| {
                let dot: f64 = (0..vectors[i].len())
                    .map(|p| vectors[i][p] * rotated(j, p))
                    .sum();
                match norms[i] > 0.0 && norms[j] > 0.0 {
                    true => dot / (norms[i] * norms[j]),
                    false => 0.0,
                }
            });
            stripes.extend(self.stripes(&held, shift));
            if shift == 0 {
                raw = held;
            }
        }
        stripes.sort_by(|a, b| {
            let sum = |s: &Stripe| s.similarity_mean * s.cells as f64;
            sum(b)
                .total_cmp(&sum(a))
                .then(a.later.0.total_cmp(&b.later.0))
        });
        let novelty = novelty(&raw, self.kernel_cells);
        let mut peaks: Vec<Peak> = (0..novelty.len())
            .filter(|&i| {
                (i == 0 || novelty[i] > novelty[i - 1])
                    && novelty.get(i + 1).is_none_or(|next| novelty[i] >= *next)
            })
            .map(|i| Peak {
                t_secs: self.cells[0].t_secs + (i as f64 + 0.5) * self.cell_secs,
                novelty: novelty[i],
            })
            .collect();
        peaks.sort_by(|a, b| b.novelty.total_cmp(&a.novelty));
        let (stripes_found, peaks_found) = (stripes.len(), peaks.len());
        stripes.truncate(self.listed);
        peaks.truncate(self.listed);
        let matrix = self.curve.then(|| {
            (0..raw.len())
                .map(|i| (0..=i).map(|lag| raw[lag][i - lag]).collect())
                .collect()
        });
        Reading {
            view,
            stripes_found,
            stripes,
            novelty,
            peaks_found,
            peaks,
            matrix,
        }
    }

    /// Ridges of the averaged diagonals, as MODEL states, run along each lag.
    fn stripes(&self, raw: &[Vec<f64>], shift: i32) -> Vec<Stripe> {
        let smooth: Vec<Vec<f64>> = raw.iter().map(|d| averaged(d, self.smooth_cells)).collect();
        let mut found = Vec::new();
        for lag in 1..smooth.len() {
            let on = |j: usize| {
                let v = smooth[lag][j];
                v > smooth[lag - 1][j + 1] && (j == 0 || v >= smooth[lag + 1][j - 1])
            };
            let mut run: Option<usize> = None;
            for j in 0..=smooth[lag].len() {
                match (run, j < smooth[lag].len() && on(j)) {
                    (None, true) => run = Some(j),
                    (Some(from), false) => {
                        found.push(self.stripe(&raw[lag][from..j], shift, lag, from));
                        run = None;
                    }
                    _ => {}
                }
            }
        }
        found
    }

    fn stripe(&self, along: &[f64], shift: i32, lag: usize, from: usize) -> Stripe {
        let to = from + along.len() - 1;
        let span = |a: usize, b: usize| (self.cells[a].from_secs, self.cells[b].to_secs);
        Stripe {
            shift_semitones: shift,
            lag_secs: lag as f64 * self.cell_secs,
            earlier: span(from, to),
            later: span(from + lag, to + lag),
            cells: along.len(),
            similarity_mean: along.iter().sum::<f64>() / along.len() as f64,
            similarity_min: along.iter().copied().fold(f64::INFINITY, f64::min),
        }
    }
}

/// `held[lag][j]` is `similarity(j + lag, j)`.
fn diagonals(n: usize, similarity: impl Fn(usize, usize) -> f64) -> Vec<Vec<f64>> {
    (0..n)
        .map(|lag| (0..n - lag).map(|j| similarity(j + lag, j)).collect())
        .collect()
}

/// The mean over `len` entries centred on each, over those the diagonal holds.
fn averaged(d: &[f64], len: usize) -> Vec<f64> {
    let before = (len - 1) / 2;
    let after = len - 1 - before;
    let mut sums = vec![0.0; d.len() + 1];
    for (i, v) in d.iter().enumerate() {
        sums[i + 1] = sums[i] + v;
    }
    (0..d.len())
        .map(|j| {
            let lo = j.saturating_sub(before);
            let hi = (j + after + 1).min(d.len());
            (sums[hi] - sums[lo]) / (hi - lo) as f64
        })
        .collect()
}

/// Foote's tapered checkerboard over the boundary before each cell but the first.
fn novelty(raw: &[Vec<f64>], half: usize) -> Vec<f64> {
    let n = raw.len();
    let taper: Vec<f64> = (0..2 * half)
        .map(|a| {
            let u = a as f64 - half as f64 + 0.5;
            u.signum() * (-u * u / (2.0 * (half * half) as f64)).exp()
        })
        .collect();
    let total: f64 = taper.iter().map(|g| g.abs()).sum::<f64>().powi(2);
    let at = |p: usize, q: usize| raw[p.abs_diff(q)][p.min(q)];
    (1..n)
        .map(|i| {
            let first = i.saturating_sub(half);
            let last = (i + half).min(n);
            let mut sum = 0.0;
            for p in first..last {
                for q in first..last {
                    sum += taper[p + half - i] * taper[q + half - i] * at(p, q);
                }
            }
            sum / total
        })
        .collect()
}
