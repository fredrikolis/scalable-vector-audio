// Concern: shapes every analysis result as JSON | Non-concern: computing any of them (stable/, experimental/) | IO: (a result) -> a JSON fragment

use sva_core::json::{NONE, list, num};

use crate::experimental::gain_reduction::GainReduction;
use crate::experimental::masking::Masking;
use crate::stable::onsets::Onsets;
use crate::stable::trajectory::{Direction, Trajectory, Verdict};

fn direction_str(d: Direction) -> &'static str {
    match d {
        Direction::Rising => "rising",
        Direction::Falling => "falling",
        Direction::Flat => "flat",
    }
}

fn verdict_json(v: &Verdict) -> String {
    format!(
        "{{ \"monotonic\": {}, \"direction\": \"{}\", \"violations\": {} }}",
        v.monotonic,
        direction_str(v.direction),
        list(&v.violations, |t| num(*t))
    )
}

fn maybe_verdict(v: Option<&Verdict>) -> String {
    v.map_or_else(|| NONE.to_string(), verdict_json)
}

pub fn onsets(o: &Onsets) -> String {
    format!(
        "{{ \"onsets\": {}, \"resolution_secs\": {}, \"ioi_histogram\": {}, \
         \"onsets_per_bar\": {}, \"syncopation_index\": {} }}",
        list(&o.onsets, |t| format!(
            "{{ \"t_secs\": {}, \"strength\": {} }}",
            num(t.t_secs),
            num(t.strength)
        )),
        num(o.resolution_secs),
        list(&o.ioi_histogram, |b| format!(
            "{{ \"lo_secs\": {}, \"hi_secs\": {}, \"count\": {} }}",
            num(b.lo_secs),
            num(b.hi_secs),
            b.count
        )),
        o.onsets_per_bar
            .as_ref()
            .map_or_else(|| NONE.to_string(), |v| list(v, |n| n.to_string())),
        o.syncopation_index.map_or_else(|| NONE.to_string(), num)
    )
}

pub fn trajectory(t: &Trajectory) -> String {
    format!(
        "{{ \"frames\": {}, \"level\": {}, \"width\": {}, \"centroid\": {} }}",
        list(&t.frames, |f| format!(
            "\n    {{ \"t_secs\": {}, \"rms_db\": {}, \"peak_db\": {}, \"width\": {}, \
             \"centroid_hz\": {}, \"flatness\": {} }}",
            num(f.t_secs),
            num(f.rms_db),
            num(f.peak_db),
            f.width.map_or_else(|| NONE.to_string(), num),
            num(f.centroid_hz),
            num(f.flatness)
        )),
        maybe_verdict(t.level.as_ref()),
        maybe_verdict(t.width.as_ref()),
        maybe_verdict(t.centroid.as_ref())
    )
}

pub fn masking(m: &Masking) -> String {
    format!(
        "{{ \"gated\": {}, \"frames_scored\": {}, \"bands\": {} }}",
        m.gated,
        m.frames_scored,
        list(&m.bands, |b| format!(
            "\n    {{ \"lo_hz\": {}, \"hi_hz\": {}, \"target_db\": {}, \"against_db\": {}, \
             \"smr_db\": {} }}",
            num(b.lo_hz),
            num(b.hi_hz),
            num(b.target_db),
            num(b.against_db),
            num(b.smr_db)
        ))
    )
}

pub fn gain_reduction(g: &GainReduction) -> String {
    format!(
        "{{ \"frames\": {} }}",
        list(&g.frames, |f| format!(
            "\n    {{ \"t_secs\": {}, \"gain_db\": {}, \"input_db\": {}, \"output_db\": {} }}",
            num(f.t_secs),
            num(f.gain_db),
            f.input_db.map_or_else(|| NONE.to_string(), num),
            f.output_db.map_or_else(|| NONE.to_string(), num)
        ))
    )
}
