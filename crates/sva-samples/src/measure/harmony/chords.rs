// Concern: the chords a curve's moments hold, a run of moments naming one note set each | Non-concern: what a chord moves to (siblings) | IO: (moments, hop, floor) -> chords

use super::{Moment, Partial, above};
use crate::measure::pitch::{Note, midi_of, name_peaks};
use crate::measure::spectrum::{Peak, db};

pub const METHOD: &str = "per moment: the partials at or above the floor, named as `pitch` names \
                          peaks (25 Hz to 5 kHz, a peak at a near-whole multiple of another in \
                          the series the greatest subharmonic sum heads dropped as its \
                          harmonic), each rounded to its nearest equal-tempered \
                          note; consecutive moments naming one set are one chord, and a moment \
                          naming none ends one; a note whose partial's frequency bounds round \
                          to another note in any of its moments is listed as unresolved";

/// `notes` are MIDI numbers, ascending, each once; `end_secs` is the next moment's instant;
/// `unresolved` are those whose frequency bounds round to another note in some moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub start_secs: f64,
    pub end_secs: f64,
    pub notes: Vec<i64>,
    pub unresolved: Vec<i64>,
}

fn nearest(hz: f64) -> i64 {
    midi_of(hz).round() as i64
}

/// The partials at or above the floor that `pitch` names, loudest first, each with its name:
/// a note, or a harmonic in a series. Partials outside its band are left out.
pub fn named(moment: &Moment, floor_db: f64) -> Vec<(Partial, Note)> {
    let mut kept = above(moment, floor_db);
    kept.sort_by(|a, b| b.amplitude.total_cmp(&a.amplitude));
    let found: Vec<Peak> = kept
        .iter()
        .map(|p| Peak {
            hz: p.hz,
            db: db(p.amplitude),
        })
        .collect();
    name_peaks(&found, usize::MAX)
        .into_iter()
        .map(|note| {
            let partial = *kept
                .iter()
                .find(|p| p.hz == note.hz)
                .expect("each name is one kept partial's");
            (partial, note)
        })
        .collect()
}

pub fn notes_at(moment: &Moment, floor_db: f64) -> (Vec<i64>, Vec<i64>) {
    let (mut notes, mut unresolved) = (Vec::new(), Vec::new());
    for (partial, named) in named(moment, floor_db) {
        if named.harmonic_of.is_some() {
            continue;
        }
        let note = nearest(named.hz);
        notes.push(note);
        let bounds = partial.hz_bounds;
        if nearest(bounds.0) != note || nearest(bounds.1) != note {
            unresolved.push(note);
        }
    }
    for held in [&mut notes, &mut unresolved] {
        held.sort_unstable();
        held.dedup();
    }
    (notes, unresolved)
}

pub fn segment(moments: &[Moment], hop_secs: f64, floor_db: f64) -> Vec<Chord> {
    let mut chords: Vec<Chord> = Vec::new();
    let mut open = false;
    for moment in moments {
        let (notes, unresolved) = notes_at(moment, floor_db);
        let end_secs = moment.t_secs + hop_secs;
        match chords.last_mut() {
            Some(last) if open && last.notes == notes => {
                last.end_secs = end_secs;
                last.unresolved.extend(unresolved);
                last.unresolved.sort_unstable();
                last.unresolved.dedup();
            }
            _ if notes.is_empty() => {}
            _ => chords.push(Chord {
                start_secs: moment.t_secs,
                end_secs,
                notes: notes.clone(),
                unresolved,
            }),
        }
        open = !notes.is_empty();
    }
    chords
}
