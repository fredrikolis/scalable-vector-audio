// Concern: expands a rational's partial fractions into pole atoms by residues | Non-concern: choosing a rational's zeros and poles | IO: (Rational) -> Vec<SpectralAtom> or Left

use crate::closed_form::Rational;
use crate::complex::C64;
use crate::origin::Origin;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Factors, Pole, Singular, SpectralAtom};

/// Measured: 7.6e-9 dB of round-trip error at order 12, above -100 dB.
pub const MAX_POLE_ORDER: u16 = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Residue {
    pub at: C64,
    pub order: u16,
    pub weight: C64,
}

/// Ascending coefficients: `c[k]` multiplies `x^k`.
type Poly = Vec<C64>;

pub fn expand(r: &Rational, origin: Origin) -> Result<Vec<SpectralAtom>, Left> {
    let mut num = from_roots(&r.zeros);
    for c in &mut num {
        *c = *c * r.gain;
    }
    let (quotient, residues) = partial_fractions(&num, &r.poles)
        .map_err(|reason| Left::new(origin, AtomSketch::of(Factor::Pole), reason))?;

    let mut out: Vec<SpectralAtom> = quotient
        .into_iter()
        .enumerate()
        .filter(|(_, c)| !c.is_zero())
        .map(|(k, c)| {
            SpectralAtom::new(
                c,
                Factors::poly(u16::try_from(k).expect("a quotient degree fits a u16")),
                Singular::Regular,
                origin,
            )
        })
        .collect();

    out.extend(
        residues
            .into_iter()
            .filter(|x| !x.weight.is_zero())
            .map(|x| {
                SpectralAtom::new(
                    x.weight,
                    Factors {
                        pole: Some(Pole {
                            at: x.at,
                            order: x.order,
                            pv: false,
                        }),
                        ..Factors::NONE
                    },
                    Singular::Regular,
                    origin,
                )
            }),
    );
    Ok(out)
}

/// A repeated pole's residues are the truncated Taylor series of the rest over it.
pub fn partial_fractions(num: &[C64], poles: &[C64]) -> Result<(Poly, Vec<Residue>), LeftReason> {
    let total = u16::try_from(poles.len()).unwrap_or(u16::MAX);
    if total > MAX_POLE_ORDER {
        return Err(LeftReason::PoleOrder(total));
    }
    let den = from_roots(poles);
    let (quotient, remainder) = divide(num, &den);

    let mut grouped: Vec<(C64, u16)> = Vec::new();
    for p in poles {
        match grouped.iter_mut().find(|(q, _)| q == p) {
            Some((_, m)) => *m += 1,
            None => grouped.push((*p, 1)),
        }
    }

    let mut out = Vec::new();
    for (p, m) in &grouped {
        let others: Vec<C64> = grouped
            .iter()
            .filter(|(q, _)| q != p)
            .flat_map(|(q, n)| std::iter::repeat_n(*q, usize::from(*n)))
            .collect();
        let top = taylor_at(&remainder, *p, usize::from(*m));
        let bottom = taylor_at(&from_roots(&others), *p, usize::from(*m));
        let g = series_div(&top, &bottom);
        for (k, weight) in g.iter().enumerate() {
            out.push(Residue {
                at: *p,
                order: m - u16::try_from(k).expect("a pole order fits a u16"),
                weight: *weight,
            });
        }
    }
    Ok((quotient, out))
}

fn from_roots(roots: &[C64]) -> Poly {
    let mut out = vec![C64::ONE];
    for r in roots {
        let mut next = vec![C64::ZERO; out.len() + 1];
        for (k, c) in out.iter().enumerate() {
            next[k + 1] = next[k + 1] + *c;
            next[k] = next[k] - *c * *r;
        }
        out = next;
    }
    out
}

fn degree(p: &[C64]) -> Option<usize> {
    p.iter().rposition(|c| !c.is_zero())
}

fn divide(num: &[C64], den: &[C64]) -> (Poly, Poly) {
    let (Some(dn), Some(dd)) = (degree(num), degree(den)) else {
        return (Vec::new(), num.to_vec());
    };
    if dn < dd {
        return (Vec::new(), num.to_vec());
    }
    let mut rem = num.to_vec();
    let mut quotient = vec![C64::ZERO; dn - dd + 1];
    for shift in (0..=dn - dd).rev() {
        let factor = rem[shift + dd] / den[dd];
        quotient[shift] = factor;
        for (k, d) in den.iter().enumerate().take(dd + 1) {
            rem[shift + k] = rem[shift + k] - factor * *d;
        }
    }
    rem.truncate(dd);
    (quotient, rem)
}

/// The first `count` Taylor coefficients about `at`, by repeated synthetic division.
fn taylor_at(p: &[C64], at: C64, count: usize) -> Poly {
    let mut work = if p.is_empty() {
        vec![C64::ZERO]
    } else {
        p.to_vec()
    };
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let last = work.len() - 1;
        let mut quotient = vec![C64::ZERO; last];
        let mut carry = work[last];
        for k in (0..last).rev() {
            quotient[k] = carry;
            carry = work[k] + carry * at;
        }
        out.push(carry);
        work = if quotient.is_empty() {
            vec![C64::ZERO]
        } else {
            quotient
        };
    }
    out
}

/// Truncated power series; `b[0]` is nonzero because the other factors miss this pole.
fn series_div(a: &[C64], b: &[C64]) -> Poly {
    let n = a.len();
    let mut out = vec![C64::ZERO; n];
    for k in 0..n {
        let mut acc = a[k];
        for j in 1..=k {
            acc = acc - b[j] * out[k - j];
        }
        out[k] = acc / b[0];
    }
    out
}
