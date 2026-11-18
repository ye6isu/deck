pub struct StudioReverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    allpass_l: Vec<Allpass>,
    allpass_r: Vec<Allpass>,
}

impl StudioReverb {
    pub fn new(sample_rate: usize) -> Self {
        let comb_tunings = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let allpass_tunings = [556, 441, 341, 225];
        let sr_factor = sample_rate as f32 / 44100.0;

        Self {
            combs_l: comb_tunings.iter().map(|&t| Comb::new((t as f32 * sr_factor) as usize)).collect(),
            combs_r: comb_tunings.iter().map(|&t| Comb::new(((t + 23) as f32 * sr_factor) as usize)).collect(),
            allpass_l: allpass_tunings.iter().map(|&t| Allpass::new((t as f32 * sr_factor) as usize)).collect(),
            allpass_r: allpass_tunings.iter().map(|&t| Allpass::new(((t + 23) as f32 * sr_factor) as usize)).collect(),
        }
    }

    pub fn reset(&mut self) {
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.buf.fill(0.0);
            comb.idx = 0;
            comb.store = 0.0;
        }
        for allpass in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            allpass.buf.fill(0.0);
            allpass.idx = 0;
        }
    }

    /// Process a stereo sample frame with a single curated wet dial (0.0 to 1.0)
    pub fn process(&mut self, in_l: f32, in_r: f32, wet_amount: f32) -> (f32, f32) {
        if wet_amount <= 0.001 {
            return (in_l, in_r);
        }

        let input = (in_l + in_r) * 0.015;
        // Warm hall tuning for sweet YouTube-style reverb
        let room_feedback = 0.82;
        let damping = 0.25;

        let mut out_l = 0.0;
        let mut out_r = 0.0;

        for (cl, cr) in self.combs_l.iter_mut().zip(self.combs_r.iter_mut()) {
            out_l += cl.process(input, room_feedback, damping);
            out_r += cr.process(input, room_feedback, damping);
        }

        for (al, ar) in self.allpass_l.iter_mut().zip(self.allpass_r.iter_mut()) {
            out_l = al.process(out_l);
            out_r = ar.process(out_r);
        }

        let dry = 1.0 - (wet_amount * 0.35);
        let wet = wet_amount * 0.65;

        (in_l * dry + out_l * wet, in_r * dry + out_r * wet)
    }
}

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    fn new(size: usize) -> Self {
        Self { buf: vec![0.0; size], idx: 0, store: 0.0 }
    }
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buf[self.idx];
        self.store = (output * (1.0 - damp)) + (self.store * damp);
        self.buf[self.idx] = input + (self.store * feedback);
        self.idx = (self.idx + 1) % self.buf.len();
        output
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(size: usize) -> Self {
        Self { buf: vec![0.0; size], idx: 0 }
    }
    fn process(&mut self, input: f32) -> f32 {
        let buf_out = self.buf[self.idx];
        let output = -input + buf_out;
        self.buf[self.idx] = input + (buf_out * 0.5);
        self.idx = (self.idx + 1) % self.buf.len();
        output
    }
}
