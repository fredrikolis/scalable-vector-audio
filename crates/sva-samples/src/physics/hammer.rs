// Concern: one Hertzian felt hammer sub-stepped against its anvils | Non-concern: what an anvil is (each model's grid) | IO: (displacements) -> forces

pub const SUBSTEPS: usize = 16;

pub struct Hammer {
    pos: f64,
    prev: f64,
    mass: f64,
    k: f64,
    anvil_k: Vec<f64>,
    p: f64,
}

impl Hammer {
    /// Seeded at the substep timescale, so the hammer carries `vel` at t=0.
    pub fn new(mass: f64, k: f64, p: f64, vel: f64, dt: f64) -> Hammer {
        let m_dt = dt / SUBSTEPS as f64;
        Hammer {
            pos: vel * m_dt * 0.5,
            prev: -vel * m_dt * 0.5,
            mass,
            k,
            anvil_k: Vec::new(),
            p,
        }
    }

    /// Anvil `i` meets the felt at `k * ratios[i]`.
    pub fn with_anvil_ratios(mut self, ratios: &[f64]) -> Hammer {
        self.anvil_k = ratios.iter().map(|r| self.k * r).collect();
        self
    }

    pub fn pos(&self) -> f64 {
        self.pos
    }

    /// `detached` is sticky per anvil.
    pub fn substeps(&mut self, dt: f64, anvils: &[f64], detached: &mut [bool], forces: &mut [f64]) {
        let m_dt = dt / SUBSTEPS as f64;
        let (mut hp, mut hpv) = (self.pos, self.prev);
        forces.fill(0.0);
        for _ in 0..SUBSTEPS {
            let mut total_force = 0.0;
            for i in 0..anvils.len() {
                let compression = hp - anvils[i];
                let force = if !detached[i] && compression > 0.0 {
                    self.anvil_k.get(i).copied().unwrap_or(self.k) * compression.powf(self.p)
                } else {
                    detached[i] = true;
                    0.0
                };
                forces[i] += force;
                total_force += force;
            }
            let next = 2.0 * hp - hpv - (m_dt * m_dt / self.mass) * total_force;
            hpv = hp;
            hp = next;
        }
        self.pos = hp;
        self.prev = hpv;
        for f in forces.iter_mut() {
            *f /= SUBSTEPS as f64;
        }
    }
}
