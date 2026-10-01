// Concern: bounds one node's magnitude from an instant on, off its atoms, formula or fixed filter | Non-concern: solvers, loops, gain to the output | IO: (NodeId) -> a bound from each instant, or none

mod filter;
mod range;

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{NodeId, Var};
use sva_samples::collapse::plan::summed_bounds;
use sva_samples::{Audible, Extent, Grid, Profile, truncate_spectral_sum, truncate_written};

use crate::cast::Cast;
use crate::lower::number_of;
use crate::typing::{Typing, Value};
use filter::Ringing;
use range::{OP, Range, TRANSFORM_OPS};

pub(crate) struct Tail(Form);

enum Form {
    /// Each direct sum over the atoms' lines as its rounding bound and the factor it is under.
    Atoms(Vec<SpectralAtom>, Vec<(Option<SpectralAtom>, f64)>),
    /// Each node the formula reads, bounded by its own form.
    Written(Range, BTreeMap<NodeId, Tail>),
    /// A fixed filter on its grid's samples.
    Filter(Ringing, Grid),
}

/// Where a node is nonzero, its prune included: past it a reader reads zero.
pub(crate) type Ends<'a> = &'a dyn Fn(NodeId) -> Extent;

impl Tail {
    pub(crate) fn of(
        tys: &Typing,
        (profile, rate): (&Profile, u32),
        id: NodeId,
        ends: Ends,
    ) -> Option<Tail> {
        Tail::within(tys, (profile, rate), id, &mut BTreeSet::new(), ends)
    }

    fn within(
        tys: &Typing,
        (profile, rate): (&Profile, u32),
        id: NodeId,
        open: &mut BTreeSet<NodeId>,
        ends: Ends,
    ) -> Option<Tail> {
        let band = Audible::of(profile, rate);
        // A series no line reaches falls to the written form, as the collapse does.
        if tys.ty(id).is_closed_form()
            && let Ok(whole) = crate::refs::spectral_sum_of(tys, id, Var::T)
            && let Ok(sum) = truncate_spectral_sum(&whole, band)
        {
            let summed = summed_bounds(&whole, profile, rate).ok()?;
            let atoms = sum
                .lanes
                .iter()
                .flat_map(|lane| {
                    let modal = lane.modal.iter().flat_map(|bank| {
                        sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN)
                    });
                    lane.atoms.iter().copied().chain(modal)
                })
                .collect();
            return Some(Tail(Form::Atoms(atoms, summed)));
        }
        let form = match tys.value(id) {
            Value::ClosedForm(form) => form,
            Value::Filter {
                shape,
                x,
                cutoff,
                q,
                gain,
            } => {
                let [cutoff, q, gain] = [cutoff, q, gain].map(|p| number_of(tys, *p));
                let grid = tys.grid(id);
                if tys.grid(*x) != grid {
                    return None;
                }
                let (coeffs, _) =
                    sva_samples::filters::coefficients(*shape, cutoff?, q?, gain?, grid.sr());
                // A retired term's past is in the filter's state but in no bound here.
                if tys.retires() {
                    return None;
                }
                let input = match tys.value(*x) {
                    Value::Cast(Cast::Sample, source) => *source,
                    _ => *x,
                };
                let input = Tail::within(tys, (profile, rate), input, open, ends)?;
                let span = ends(*x);
                let first = match span.start {
                    i64::MIN => f64::NEG_INFINITY,
                    start => grid.instant(start),
                };
                let ringing = Ringing::of(&coeffs, input.from(first), span.end)?;
                return Some(Tail(Form::Filter(ringing, grid)));
            }
            _ => return None,
        };
        if form.var != Var::T {
            return None;
        }
        let range = Range::of(&truncate_written(&form.body, band).ok()?).ok()?;
        let mut nodes = Vec::new();
        range.nodes(&mut nodes);
        open.insert(id).then_some(())?;
        let reads = nodes
            .into_iter()
            .map(|n| Some((n, Tail::within(tys, (profile, rate), n, open, ends)?)))
            .collect::<Option<BTreeMap<_, _>>>();
        open.remove(&id);
        Some(Tail(Form::Written(range, reads?)))
    }

    /// Bounds `|x(s)|` for every `s >= t`, its rounding included; infinite where none holds.
    pub(crate) fn from(&self, t: f64) -> f64 {
        match &self.0 {
            Form::Atoms(atoms, summed) => {
                let rounded = 1.0 + OP * (atoms.len() as f64 + TRANSFORM_OPS);
                let mut sum = 0.0;
                for atom in atoms {
                    match sup_from(atom, t) {
                        Some(sup) => sum += sup,
                        None => return f64::INFINITY,
                    }
                }
                let direct = summed.iter().try_fold(0.0, |held, (factor, err)| {
                    let under = factor.as_ref().map_or(Some(1.0), |f| sup_from(f, t))?;
                    Some(held + err * under)
                });
                direct.map_or(f64::INFINITY, |direct| sum * rounded + direct)
            }
            Form::Written(range, reads) => range
                .from(t, &|n, t| reads[&n].from(t))
                .map_or(f64::INFINITY, |s| s.reach() + s.err),
            Form::Filter(ringing, grid) => {
                ringing.from(grid.count(t).floor().clamp(-9e18, 9e18) as i64)
            }
        }
    }
}
