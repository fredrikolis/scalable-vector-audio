// Concern: states that the sub-stepped felt converges and releases once | Non-concern: what an anvil is (each model) | IO: (displacements) -> asserted forces

use sva_samples::physics::hammer::{Hammer, SUBSTEPS};

/// The recurrence against a rigid anvil pinned at 0, at a sub-step count the shipped
/// `SUBSTEPS` fixes — which is why the sweep cannot drive `Hammer` directly.
fn isolated_contact_time(mass: f64, k: f64, p: f64, vel: f64, sr: f64, substeps: usize) -> f64 {
    let m_dt = (1.0 / sr) / substeps as f64;
    let (mut hp, mut hpv) = (vel * m_dt * 0.5, -vel * m_dt * 0.5);
    let mut count = 0usize;
    loop {
        let compression = hp;
        if compression <= 0.0 {
            break;
        }
        let force = k * compression.powf(p);
        let next = 2.0 * hp - hpv - (m_dt * m_dt / mass) * force;
        hpv = hp;
        hp = next;
        count += 1;
    }
    count as f64 * m_dt
}

/// The same contact, stepped by the shipped `Hammer`. `substeps` sets the block `dt` so its
/// own `SUBSTEPS` sub-steps are `(1/sr)/substeps` each; the crossing is read between blocks.
fn shipped_contact_time(mass: f64, k: f64, p: f64, vel: f64, sr: f64, substeps: usize) -> f64 {
    let dt = (1.0 / sr) * SUBSTEPS as f64 / substeps as f64;
    let mut hammer = Hammer::new(mass, k, p, vel, dt);
    let mut detached = [false; 1];
    let mut forces = [0.0f64; 1];
    let mut elapsed = 0.0;
    let mut previous = hammer.pos();
    for _ in 0..1_000_000 {
        hammer.substeps(dt, &[0.0], &mut detached, &mut forces);
        elapsed += dt;
        let now = hammer.pos();
        if now <= 0.0 {
            return elapsed + dt * now / (previous - now);
        }
        previous = now;
    }
    panic!("the felt never left the anvil")
}

/// The copy above is worth something only while it answers what the shipped hammer does.
#[test]
fn the_sweeps_recurrence_answers_what_the_shipped_hammer_does() {
    let (mass, k, p, vel, sr) = (2.9e-3f64, 2.6646e8f64, 2.5f64, 3.2f64, 44_100.0f64);
    let fine = 4096;
    let local = isolated_contact_time(mass, k, p, vel, sr, fine);
    let shipped = shipped_contact_time(mass, k, p, vel, sr, fine);
    assert!(
        (local - shipped).abs() / local < 2e-3,
        "one recurrence: local={local} shipped={shipped}"
    );
}

#[test]
fn the_isolated_hammer_recurrence_converges_to_the_closed_form_p1_period() {
    let (mass, k): (f64, f64) = (4.9e-3, 1e8);
    let closed_form = std::f64::consts::PI * (mass / k).sqrt();
    for sr in [44_100.0f64, 48_000.0] {
        for (substeps, tolerance) in [(16, 0.05), (256, 0.02), (16384, 0.001)] {
            let t = isolated_contact_time(mass, k, 1.0, 3.0, sr, substeps);
            assert!(
                (t - closed_form).abs() / closed_form < tolerance,
                "sr={sr} substeps={substeps} t={t} closed={closed_form}"
            );
        }
    }
}

#[test]
fn sixteen_substeps_already_agree_with_a_much_finer_count_at_the_shipped_exponent() {
    let (mass, k, p, vel, sr): (f64, f64, f64, f64, f64) = (2.9e-3, 2.6646e8, 2.5, 3.2, 44_100.0);
    let shipped = isolated_contact_time(mass, k, p, vel, sr, SUBSTEPS);
    let fine = isolated_contact_time(mass, k, p, vel, sr, 4096);
    assert!(
        (shipped - fine).abs() / fine < 0.02,
        "t{SUBSTEPS}={shipped} t4096={fine}"
    );
}

#[test]
fn a_single_anvil_and_a_unison_set_of_one_move_the_hammer_identically() {
    let dt = 1.0 / 44_100.0;
    let mut one = Hammer::new(4.9e-3, 1e8, 2.5, 3.0, dt);
    let mut many = Hammer::new(4.9e-3, 1e8, 2.5, 3.0, dt);
    let (mut d1, mut d3) = ([false; 1], [false; 3]);
    let (mut f1, mut f3) = ([0.0f64; 1], [0.0f64; 3]);
    for _ in 0..64 {
        one.substeps(dt, &[0.0], &mut d1, &mut f1);
        many.substeps(dt, &[0.0], &mut d3[..1], &mut f3[..1]);
    }
    assert_eq!(one.pos().to_bits(), many.pos().to_bits());
    assert_eq!(f1[0].to_bits(), f3[0].to_bits());
}

#[test]
fn a_released_anvil_never_re_engages_even_once_the_hammer_returns() {
    let dt = 1.0 / 44_100.0;
    let mut hammer = Hammer::new(4.9e-3, 1e8, 2.5, 3.0, dt);
    let mut detached = [false; 1];
    let mut forces = [0.0f64; 1];
    hammer.substeps(dt, &[1.0], &mut detached, &mut forces);
    assert!(detached[0], "an anvil out of reach releases the felt");
    assert_eq!(forces[0], 0.0);
    hammer.substeps(dt, &[-1.0], &mut detached, &mut forces);
    assert_eq!(forces[0], 0.0, "and never bears load again");
}
