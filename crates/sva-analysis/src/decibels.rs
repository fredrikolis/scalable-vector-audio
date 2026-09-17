// Concern: amplitude to dB for every analysis that reports one | Non-concern: the floored dB a spectrum's own scale needs (sva_samples::db) | IO: (amplitude) -> dB

/// Silence is `-inf`, not a floor: an analysis states what it measured and leaves the floor
/// to whoever reads it. `sva_samples::db` clamps instead, because a spectrum is drawn.
pub fn to_db(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * amplitude.log10()
    }
}
