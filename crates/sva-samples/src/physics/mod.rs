// Concern: names the six finite-difference models, opens one solver, and holds the ceiling and the drive each shared by two | Non-concern: a model's own grid (the siblings) | IO: (Params) -> a Solver

pub mod botteldooren;
pub mod bound;
pub mod chaigne_askenfelt;
pub mod chaigne_doutaut;
pub mod darabundit_scavone;
pub mod hammer;
pub mod rhaouti_chaigne_joly;
pub mod tonehole;
pub mod willemsen_bilbao_serafin;

use crate::error::SampleError;

use botteldooren::{BotteldoorenParams, BotteldoorenSite};
use chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use chaigne_doutaut::{ChaigneDoutautParams, ChaigneDoutautSite};
use darabundit_scavone::{BoreParams, BoreSite};
use rhaouti_chaigne_joly::{RhaoutiChaigneJolyParams, RhaoutiChaigneJolySite};
use willemsen_bilbao_serafin::{WillemsenBilbaoSerafinParams, WillemsenBilbaoSerafinSite};

/// One sample per call. The derivative half of the old pair died with the lanes.
pub trait Solver {
    fn step(&mut self) -> Result<f64, SampleError>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Params {
    ChaigneAskenfelt(ChaigneAskenfeltParams),
    WillemsenBilbaoSerafin(WillemsenBilbaoSerafinParams),
    DarabunditScavone(BoreParams),
    RhaoutiChaigneJoly(RhaoutiChaigneJolyParams),
    ChaigneDoutaut(ChaigneDoutautParams),
    Botteldooren(BotteldoorenParams),
}

impl Params {
    pub fn name(&self) -> &'static str {
        match self {
            Params::ChaigneAskenfelt(_) => "chaigne_askenfelt",
            Params::WillemsenBilbaoSerafin(_) => "willemsen_bilbao_serafin",
            Params::DarabunditScavone(_) => "darabundit_scavone",
            Params::RhaoutiChaigneJoly(_) => "rhaouti_chaigne_joly",
            Params::ChaigneDoutaut(_) => "chaigne_doutaut",
            Params::Botteldooren(_) => "botteldooren",
        }
    }

    pub fn valid(&self) -> bool {
        match self {
            Params::ChaigneAskenfelt(p) => p.valid(),
            Params::WillemsenBilbaoSerafin(p) => p.valid(),
            Params::DarabunditScavone(p) => p.valid(),
            Params::RhaoutiChaigneJoly(p) => p.valid(),
            Params::ChaigneDoutaut(p) => p.valid(),
            Params::Botteldooren(p) => p.valid(),
        }
    }
}

/// One grid, sized at the observation's rate; nothing here reads a sample back. A size only
/// the rate decides is checked here, where the rate is known — `valid()` never sees one.
pub fn site(p: &Params, rate: u32) -> Result<Box<dyn Solver>, SampleError> {
    let sr = f64::from(rate);
    Ok(match p {
        Params::ChaigneAskenfelt(p) => Box::new(ChaigneAskenfeltSite::new(p, sr)?),
        Params::WillemsenBilbaoSerafin(p) => Box::new(WillemsenBilbaoSerafinSite::new(p, sr)?),
        Params::DarabunditScavone(p) => Box::new(BoreSite::new(p, sr)),
        Params::RhaoutiChaigneJoly(p) => {
            under_ceiling(
                "rhaouti_chaigne_joly",
                rhaouti_chaigne_joly::node_count(p, sr),
                rhaouti_chaigne_joly::NODE_COUNT_CEILING,
            )?;
            Box::new(RhaoutiChaigneJolySite::new(p, sr))
        }
        Params::ChaigneDoutaut(p) => Box::new(ChaigneDoutautSite::new(p, sr)),
        Params::Botteldooren(p) => {
            under_ceiling(
                "botteldooren",
                botteldooren::node_count(p.f0, p.aspect_y, p.aspect_z, sr),
                botteldooren::NODE_COUNT_CEILING,
            )?;
            Box::new(BotteldoorenSite::new(p, sr))
        }
    })
}

fn under_ceiling(model: &'static str, nodes: f64, ceiling: usize) -> Result<(), SampleError> {
    match nodes > ceiling as f64 {
        true => Err(SampleError::GridTooLarge {
            model,
            nodes: nodes as usize,
            ceiling,
        }),
        false => Ok(()),
    }
}

/// The raised-cosine pressure pulse the room and the bore are driven by: one half-turn over
/// `width` seconds and silence after, so the drive starts and ends at rest.
pub(crate) fn raised_cosine_pulse(t: f64, amp: f64, width: f64) -> f64 {
    match t < width {
        true => amp * 0.5 * (1.0 - (std::f64::consts::TAU * t / width).cos()),
        false => 0.0,
    }
}
