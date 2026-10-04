// Concern: the radix-2 complex transform, its inverse from half the bins, what a transform costs | Non-concern: what a bin means (stft.rs, measure/) | IO: (&mut [f64], &mut [f64]) -> (); n -> flops

use std::f64::consts::TAU;

/// One twiddle table serves every stage.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert_eq!(n, im.len(), "one real and one imaginary plane");
    assert!(n.is_power_of_two(), "radix-2 needs a power-of-two length");
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let twiddle: Vec<(f64, f64)> = (0..n / 2)
        .map(|k| {
            let angle = -TAU * k as f64 / n as f64;
            (angle.cos(), angle.sin())
        })
        .collect();

    let mut len = 2;
    while len <= n {
        let stride = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = twiddle[k * stride];
                let (i0, i1) = (start + k, start + k + len / 2);
                let tr = re[i1] * wr - im[i1] * wi;
                let ti = re[i1] * wi + im[i1] * wr;
                re[i1] = re[i0] - tr;
                im[i1] = im[i0] - ti;
                re[i0] += tr;
                im[i0] += ti;
            }
        }
        len <<= 1;
    }
}

pub fn ifft(re: &mut [f64], im: &mut [f64]) {
    for x in im.iter_mut() {
        *x = -*x;
    }
    fft(re, im);
    let scale = 1.0 / re.len() as f64;
    for x in re.iter_mut() {
        *x *= scale;
    }
    for x in im.iter_mut() {
        *x *= -scale;
    }
}

/// The caller fills `0..=n/2`; this mirrors the rest.
pub fn irfft(bins_re: &[f64], bins_im: &[f64], n: usize) -> Vec<f64> {
    assert_eq!(bins_re.len(), n / 2 + 1);
    assert_eq!(bins_im.len(), n / 2 + 1);
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for k in 0..=n / 2 {
        re[k] = bins_re[k];
        im[k] = bins_im[k];
        if k > 0 && k < n - k {
            re[n - k] = bins_re[k];
            im[n - k] = -bins_im[k];
        }
    }
    ifft(&mut re, &mut im);
    re
}

/// Other lengths price as the next power of two; the STFT refuses them.
pub fn transform_flops(n: usize) -> u128 {
    let m = n.next_power_of_two();
    m as u128 * u128::from(m.max(2).trailing_zeros())
}
