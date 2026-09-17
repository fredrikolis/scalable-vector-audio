// Concern: the complex transform at any length, and the real one every caller reads bins off | Non-concern: what a bin means (stft.rs, collapse/) | IO: (&mut [f64], &mut [f64]) -> ()

use std::f64::consts::{PI, TAU};

/// Iterative radix-2 Cooley-Tukey, in place. One twiddle table serves every stage.
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

/// `exp(i*pi*k^2/n)`, `k^2` reduced mod `2n` so a large index keeps its digits.
fn chirp(k: usize, n: usize) -> (f64, f64) {
    let squared = (k as u128 * k as u128 % (2 * n as u128)) as f64;
    let theta = PI * squared / n as f64;
    (theta.cos(), theta.sin())
}

/// Bluestein's chirp-z: any length, so no horizon is padded to a power of two.
pub fn dft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    if n.is_power_of_two() {
        fft(re, im);
        return;
    }
    let m = (2 * n - 1).next_power_of_two();
    let (mut ar, mut ai) = (vec![0.0; m], vec![0.0; m]);
    let (mut br, mut bi) = (vec![0.0; m], vec![0.0; m]);
    for k in 0..n {
        let (c, s) = chirp(k, n);
        ar[k] = re[k] * c + im[k] * s;
        ai[k] = im[k] * c - re[k] * s;
        br[k] = c;
        bi[k] = s;
        if k > 0 {
            br[m - k] = c;
            bi[m - k] = s;
        }
    }
    fft(&mut ar, &mut ai);
    fft(&mut br, &mut bi);
    for i in 0..m {
        let product = (ar[i] * br[i] - ai[i] * bi[i], ar[i] * bi[i] + ai[i] * br[i]);
        (ar[i], ai[i]) = product;
    }
    ifft(&mut ar, &mut ai);
    for k in 0..n {
        let (c, s) = chirp(k, n);
        re[k] = ar[k] * c + ai[k] * s;
        im[k] = ai[k] * c - ar[k] * s;
    }
}

pub fn idft(re: &mut [f64], im: &mut [f64]) {
    for x in im.iter_mut() {
        *x = -*x;
    }
    dft(re, im);
    let scale = 1.0 / re.len() as f64;
    for x in re.iter_mut() {
        *x *= scale;
    }
    for x in im.iter_mut() {
        *x = -*x * scale;
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

pub fn rfft(x: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = x.len();
    let mut re = x.to_vec();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    re.truncate(n / 2 + 1);
    im.truncate(n / 2 + 1);
    (re, im)
}
