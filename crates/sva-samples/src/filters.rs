// Concern: runs one filter call site and traces its parameters in time | Non-concern: coefficient math (biquad.rs), argument evaluation | IO: (x, cutoff, q, gain, i) -> a lane each, traces

use sva_formula::filter::Shape;

use crate::biquad::{Coeffs, clamp_cutoff, clamp_q, design, response};

pub const TRACE_HZ: f64 = 1000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomationFrame {
    pub t_secs: f64,
    pub cutoff: f64,
    pub q: f64,
    pub gain_db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterTrace {
    pub node: String,
    pub site: usize,
    pub channel: Option<usize>,
    pub shape: &'static str,
    pub clamped: bool,
    pub trace_secs: f64,
    pub frames: Vec<AutomationFrame>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Automation {
    pub node: String,
    pub site: usize,
    pub channel: Option<usize>,
    pub shape: &'static str,
    pub clamped: bool,
    pub frames: Vec<AutomationFrame>,
    pub coefficients: Coeffs,
    pub response_db: Vec<(f64, f64)>,
}

impl FilterTrace {
    pub fn over(&self, start_secs: f64, end_secs: f64, sample_rate: f64) -> Automation {
        let frames: Vec<AutomationFrame> = self
            .frames
            .iter()
            .copied()
            .filter(|f| f.t_secs >= start_secs && f.t_secs < end_secs)
            .collect();
        let midpoint = (start_secs + end_secs) / 2.0;
        let mid = nearest(&self.frames, midpoint).unwrap_or(AutomationFrame {
            t_secs: midpoint,
            cutoff: 0.0,
            q: 1.0,
            gain_db: 0.0,
        });
        let shape = Shape::from_name(self.shape).expect("shape came from Shape::name");
        let cutoff = clamp_cutoff(mid.cutoff, sample_rate).0;
        let coefficients = design(shape, cutoff, clamp_q(mid.q).0, mid.gain_db, sample_rate);
        Automation {
            node: self.node.clone(),
            site: self.site,
            channel: self.channel,
            shape: self.shape,
            clamped: self.clamped,
            response_db: response(&coefficients, cutoff, sample_rate),
            coefficients,
            frames,
        }
    }
}

/// Component `c` of a parameter `width` slots wide: a one-wide parameter answers every lane.
#[inline]
fn part<T: Copy>(v: &[T], c: usize) -> T {
    v[c.min(v.len() - 1)]
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn step(
    lane: &mut Lane,
    shape: Shape,
    stride: usize,
    clamped: &mut bool,
    x: f64,
    cutoff: f64,
    q: f64,
    gain_db: f64,
    sr: f64,
    i: usize,
) -> f64 {
    if (cutoff, q, gain_db) != lane.designed_for {
        let (cc, c_clamped) = clamp_cutoff(cutoff, sr);
        let (qq, q_clamped) = clamp_q(q);
        lane.coeffs = design(shape, cc, qq, gain_db, sr);
        lane.designed_for = (cutoff, q, gain_db);
        *clamped |= c_clamped || q_clamped;
    }
    if i.is_multiple_of(stride) {
        lane.frames.push(AutomationFrame {
            t_secs: i as f64 / sr,
            cutoff,
            q,
            gain_db,
        });
    }

    lane.state.step(&lane.coeffs, x)
}

/// Distance decides, never array position: a window off the 1 ms trace grid — or narrower
/// than it — still resolves the equation actually in force at its own midpoint.
fn nearest(frames: &[AutomationFrame], t_secs: f64) -> Option<AutomationFrame> {
    frames
        .iter()
        .min_by(|a, b| {
            (a.t_secs - t_secs)
                .abs()
                .total_cmp(&(b.t_secs - t_secs).abs())
        })
        .copied()
}

#[derive(Clone)]
struct Lane {
    state: crate::biquad::State,
    coeffs: Coeffs,
    designed_for: (f64, f64, f64),
    frames: Vec<AutomationFrame>,
}

impl Lane {
    fn new(shape: Shape, cutoff: f64, q: f64, gain_db: f64, sr: f64) -> (Lane, bool) {
        let (c, c_clamped) = clamp_cutoff(cutoff, sr);
        let (qq, q_clamped) = clamp_q(q);
        let lane = Lane {
            state: crate::biquad::State::default(),
            coeffs: design(shape, c, qq, gain_db, sr),
            designed_for: (cutoff, q, gain_db),
            frames: Vec::new(),
        };
        (lane, c_clamped || q_clamped)
    }
}

/// A call site, not a channel: one site carries as many lanes as its widest argument has
/// components, so widening a signal never multiplies the call sites an author writes.
#[derive(Clone)]
pub struct FilterSite {
    shape: Shape,
    lanes: Vec<Lane>,
    clamped: bool,
    stride: usize,
}

impl FilterSite {
    pub fn new(
        shape: Shape,
        width: usize,
        cutoff: &[f64],
        q: &[f64],
        gain_db: &[f64],
        sr: f64,
    ) -> FilterSite {
        let mut lanes = Vec::with_capacity(width);
        let mut clamped = false;
        for c in 0..width.max(1) {
            let (lane, hit) = Lane::new(shape, part(cutoff, c), part(q, c), part(gain_db, c), sr);
            lanes.push(lane);
            clamped |= hit;
        }
        FilterSite {
            shape,
            lanes,
            clamped,
            stride: ((sr / TRACE_HZ).round() as usize).max(1),
        }
    }

    /// Coefficients are redesigned only when a parameter actually moved, so a static filter
    /// costs five multiplies a sample and a modulated one pays the trig it asked for.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        x: &[f64],
        cutoff: &[f64],
        q: &[f64],
        gain_db: &[f64],
        out: &mut [f64],
        sr: f64,
        i: usize,
    ) {
        let (shape, stride) = (self.shape, self.stride);
        let mut clamped = self.clamped;
        for (c, lane) in self.lanes.iter_mut().enumerate() {
            out[c] = step(
                lane,
                shape,
                stride,
                &mut clamped,
                part(x, c),
                part(cutoff, c),
                part(q, c),
                part(gain_db, c),
                sr,
                i,
            );
        }
        self.clamped = clamped;
    }

    /// A stream reads no trace, so each block drops the last one's and keeps the room.
    pub fn forget_frames(&mut self) {
        for lane in &mut self.lanes {
            lane.frames.clear();
        }
    }

    pub fn trace(&self, node: &str, site: usize, sr: f64) -> Vec<FilterTrace> {
        let wide = self.lanes.len() > 1;
        self.lanes
            .iter()
            .enumerate()
            .map(|(c, lane)| FilterTrace {
                node: node.to_string(),
                site,
                channel: wide.then_some(c),
                shape: self.shape.name(),
                clamped: self.clamped,
                trace_secs: self.stride as f64 / sr,
                frames: lane.frames.clone(),
            })
            .collect()
    }
}
