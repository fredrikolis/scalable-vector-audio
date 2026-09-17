// Concern: states what a rhaouti_chaigne_joly solver must hold over the buffer it steps out | Non-concern: the site's own FD math (src/physics/rhaouti_chaigne_joly.rs) | IO: (Params) -> asserted peaks

mod fd;

use sva_samples::Params;
use sva_samples::physics::rhaouti_chaigne_joly::{
    NODE_COUNT_CEILING, RhaoutiChaigneJolyParams, node_count,
};

/// `sqrt(2)` avoids the degeneracy Bilbao warns of at rational aspect ratios.
#[test]
fn a_stepped_spectrum_matches_the_rectangular_membrane_modal_formula() {
    let (f0, aspect_ratio) = (200.0, std::f64::consts::SQRT_2);
    let params = Params::RhaoutiChaigneJoly(RhaoutiChaigneJolyParams {
        aspect_ratio,
        ..RhaoutiChaigneJolyParams::at(f0)
    });
    let buffer = fd::render(&params, 48_000, 0.2);
    fd::sounds(&buffer);
    let peaks = fd::peaks(&buffer, 24, None);

    // c = sqrt(3000/0.3) m/s, the module's T/sigma.
    let c = 100.0f64;
    let l0 = c / (f0 * std::f64::consts::SQRT_2);
    let lx = l0 * aspect_ratio.sqrt();
    let ly = l0 / aspect_ratio.sqrt();

    for (p, q) in [(1u32, 1u32), (2, 1), (1, 2)] {
        let predicted =
            (c / 2.0) * ((f64::from(p) / lx).powi(2) + (f64::from(q) / ly).powi(2)).sqrt();
        let found = fd::nearest(&peaks, predicted);
        let error = (found - predicted).abs() / predicted;
        assert!(
            error < 0.05,
            "mode ({p},{q}): measured {found} predicted {predicted} error {error}"
        );
    }
}

#[test]
fn a_rhaouti_chaigne_joly_site_steps_the_same_buffer_every_time() {
    let params = Params::RhaoutiChaigneJoly(RhaoutiChaigneJolyParams {
        aspect_ratio: std::f64::consts::SQRT_2,
        ..RhaoutiChaigneJolyParams::at(200.0)
    });
    fd::is_deterministic(&params, 48_000, 0.05);
}

#[test]
fn a_parameter_out_of_range_is_refused() {
    let base = || RhaoutiChaigneJolyParams::at(200.0);
    fd::refuses(
        &Params::RhaoutiChaigneJoly(base()),
        [
            RhaoutiChaigneJolyParams { f0: 0.0, ..base() },
            RhaoutiChaigneJolyParams {
                aspect_ratio: 0.0,
                ..base()
            },
            RhaoutiChaigneJolyParams {
                strike_x: 0.0,
                ..base()
            },
            RhaoutiChaigneJolyParams {
                strike_y: 1.0,
                ..base()
            },
            RhaoutiChaigneJolyParams { vel: 0.0, ..base() },
            RhaoutiChaigneJolyParams {
                hammer_mass: -1.0,
                ..base()
            },
            RhaoutiChaigneJolyParams {
                hammer_k: 0.0,
                ..base()
            },
            RhaoutiChaigneJolyParams {
                hammer_p: 0.0,
                ..base()
            },
            RhaoutiChaigneJolyParams {
                damp_dc: -0.1,
                ..base()
            },
            RhaoutiChaigneJolyParams {
                f0: f64::NAN,
                ..base()
            },
        ]
        .into_iter()
        .map(Params::RhaoutiChaigneJoly)
        .collect(),
    );
}

#[test]
fn an_undamped_run_stays_bounded() {
    fd::stays_bounded(
        &Params::RhaoutiChaigneJoly(RhaoutiChaigneJolyParams {
            aspect_ratio: std::f64::consts::SQRT_2,
            damp_dc: 0.0,
            damp_freq: 0.0,
            ..RhaoutiChaigneJolyParams::at(200.0)
        }),
        48_000,
        0.2,
    );
}

/// The sides grow as `1/f0`, so a low but otherwise valid fundamental asks for a grid no
/// machine holds: two guards `valid()` cannot state, both needing the rate.
#[test]
fn a_fundamental_too_low_to_size_a_grid_refuses_before_it_allocates() {
    let sr = 44_100.0;
    assert!(node_count(&RhaoutiChaigneJolyParams::at(5.0), sr) > NODE_COUNT_CEILING as f64);
    assert!(node_count(&RhaoutiChaigneJolyParams::at(200.0), sr) < NODE_COUNT_CEILING as f64);

    let asked = Params::RhaoutiChaigneJoly(RhaoutiChaigneJolyParams::at(5.0));
    assert!(asked.valid(), "the parameters themselves are well formed");
    let Err(refused) = sva_samples::physics::site(&asked, 44_100) else {
        panic!("a grid that large is refused, not opened")
    };
    assert_eq!(refused.code(), "samples.grid_too_large", "{refused:?}");
    assert!(
        sva_samples::physics::site(
            &Params::RhaoutiChaigneJoly(RhaoutiChaigneJolyParams::at(200.0)),
            44_100,
        )
        .is_ok(),
        "a drumhead at a musical fundamental still opens"
    );
}
