// Concern: bounds one closed form's magnitude from an instant on, off its atoms or formula | Non-concern: solvers, filters, loops, gain to the output | IO: (NodeId) -> a bound from each instant, or none

mod range;

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{NodeId, Var};
use sva_samples::collapse::plan::summed_bounds;
use sva_samples::{Audible, truncate_spectral_sum, truncate_written};

use crate::render::RenderConfig;
use crate::typing::{Typing, Value};
use range::{OP, Range, TRANSFORM_OPS};

pub(crate) struct Tail(Form);

enum Form {
    /// Each direct sum over the atoms' lines as its rounding bound and the factor it is under.
    Atoms(Vec<SpectralAtom>, Vec<(Option<SpectralAtom>, f64)>),
    /// Each node the formula reads, bounded by its own form.
    Written(Range, BTreeMap<NodeId, Tail>),
}

impl Tail {
    pub(crate) fn of(tys: &Typing, config: &RenderConfig, id: NodeId) -> Option<Tail> {
        Tail::within(tys, config, id, &mut BTreeSet::new())
    }

    fn within(
        tys: &Typing,
        config: &RenderConfig,
        id: NodeId,
        open: &mut BTreeSet<NodeId>,
    ) -> Option<Tail> {
        let band = Audible::of(&config.profile, config.rate);
        // A series no line reaches falls to the written form, as the collapse does.
        if tys.ty(id).is_closed_form()
            && let Ok(whole) = crate::refs::spectral_sum_of(tys, id, Var::T)
            && let Ok(sum) = truncate_spectral_sum(&whole, band)
        {
            let summed = summed_bounds(&whole, &config.profile, config.rate).ok()?;
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
        let Value::ClosedForm(form) = tys.value(id) else {
            return None;
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
            .map(|n| Some((n, Tail::within(tys, config, n, open)?)))
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
        }
    }
}
