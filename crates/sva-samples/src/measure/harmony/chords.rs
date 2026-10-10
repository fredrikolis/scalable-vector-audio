// Concern: the chords a curve's moments hold, a run of moments naming one note set each | Non-concern: what a chord moves to (siblings) | IO: (moments, hop, floor) -> chords

use super::{Moment, above};
use crate::measure::pitch::{midi_of, name_peaks};
use crate::measure::spectrum::{Peak, db};

pub const METHOD: &str = "per moment: the partials at or above the floor, named as `pitch` names \
                          peaks (25 Hz to 5 kHz, a peak at a near-whole multiple of a louder one \
                          dropped as its harmonic), each rounded to its nearest equal-tempered \
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

pub fn notes_at(moment: &Moment, floor_db: f64) -> (Vec<i64>, Vec<i64>) {
    let kept = above(moment, floor_db);
    let mut found: Vec<Peak> = kept
        .iter()
        .map(|p| Peak {
            hz: p.hz,
            db: db(p.amplitude),
        })
        .collect();
    found.sort_by(|a, b| b.db.total_cmp(&a.db));
    let (mut notes, mut unresolved) = (Vec::new(), Vec::new());
    for named in name_peaks(&found, usize::MAX) {
        if named.harmonic_of.is_some() {
            continue;
        }
        let note = nearest(named.hz);
        notes.push(note);
        let bounds = kept
            .iter()
            .find(|p| p.hz == named.hz)
            .expect("each name is one kept partial's")
            .hz_bounds;
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
