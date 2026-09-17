// Concern: holds one buffer's short-time spectra as (frame,bin) complex planes | Non-concern: the transform itself (stft.rs), reading one (measure/spectrum.rs) | IO: (c, frame, bin) -> Complex

/// `edited` is set by any `write`, and is what makes a later `istft` label measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub rate: u32,
    pub window: usize,
    pub hop: usize,
    pub width: usize,
    pub frames: usize,
    pub bins: usize,
    pub origin_secs: f64,
    pub samples: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    pub edited: bool,
}

impl Frames {
    pub fn silence(
        rate: u32,
        window: usize,
        hop: usize,
        width: usize,
        frames: usize,
        samples: usize,
    ) -> Frames {
        let bins = window / 2 + 1;
        Frames {
            rate,
            window,
            hop,
            width,
            frames,
            bins,
            origin_secs: 0.0,
            samples,
            re: vec![0.0; width * frames * bins],
            im: vec![0.0; width * frames * bins],
            edited: false,
        }
    }

    fn slot(&self, c: usize, frame: usize, bin: usize) -> usize {
        (c * self.frames + frame) * self.bins + bin
    }

    pub fn at(&self, c: usize, frame: usize, bin: usize) -> (f64, f64) {
        let k = self.slot(c, frame, bin);
        (self.re[k], self.im[k])
    }

    pub fn write(&mut self, c: usize, frame: usize, bin: usize, re: f64, im: f64) {
        let k = self.slot(c, frame, bin);
        self.re[k] = re;
        self.im[k] = im;
        self.edited = true;
    }

    pub(crate) fn place(&mut self, c: usize, frame: usize, bin: usize, re: f64, im: f64) {
        let k = self.slot(c, frame, bin);
        self.re[k] = re;
        self.im[k] = im;
    }
}
