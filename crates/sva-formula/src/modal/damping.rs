// Concern: the per-mode decay closed form every geometry here shares | Non-concern: which modes it damps (the geometry files) | IO: (Hz) -> tau

/// `1/tau = dc + per_square_hz * f^2`; Chaigne & Askenfelt, JASA 95 (1994) 1112.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Damping {
    pub dc: f64,
    pub per_square_hz: f64,
}

impl Damping {
    pub fn tau(self, hz: f64) -> f64 {
        1.0 / (self.dc + self.per_square_hz * hz * hz)
    }
}
