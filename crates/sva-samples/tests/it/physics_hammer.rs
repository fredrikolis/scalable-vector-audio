// Concern: states that the felt models one anvil and releases once | Non-concern: what an anvil is (each model), the sub-stepped recurrence's own convergence | IO: (displacements) -> asserted forces

use sva_samples::physics::hammer::Hammer;

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
