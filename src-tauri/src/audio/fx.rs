//! Vocal channel-strip building blocks.

use std::f32::consts::PI;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[inline]
fn coef(ms: f32, fs: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * fs).max(1.0)).exp()
}

// ---------------------------------------------------------------- biquad --

#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Shape {
    HighPass,
    LowPass,
    LowShelf(f32),
    HighShelf(f32),
    Peak(f32),
}

impl Biquad {
    /// RBJ cookbook coefficients.
    pub fn set(&mut self, shape: Shape, freq: f32, q: f32, fs: f32) {
        let w0 = 2.0 * PI * (freq / fs).clamp(1e-4, 0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match shape {
            Shape::HighPass => ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Shape::LowPass => ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Shape::Peak(db) => {
                let a = 10f32.powf(db / 40.0);
                (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
            }
            Shape::LowShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            Shape::HighShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        // Transposed direct form II.
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Recomputes biquad coefficients only when a parameter actually moves.
#[derive(Default)]
pub struct SmartBiquad {
    pub f: Biquad,
    key: (f32, f32),
}

impl SmartBiquad {
    pub fn update(&mut self, shape: Shape, freq: f32, q: f32, fs: f32) {
        let gain = match shape {
            Shape::LowShelf(g) | Shape::HighShelf(g) | Shape::Peak(g) => g,
            _ => 0.0,
        };
        if self.key != (freq, gain) {
            self.key = (freq, gain);
            self.f.set(shape, freq, q, fs);
        }
    }
}

// ------------------------------------------------------------ dynamics --

/// Downward expander/gate with hold, so word tails are not chopped.
pub struct Gate {
    env: f32,
    gain: f32,
    hold: u32,
    hold_len: u32,
    att: f32,
    rel: f32,
    env_c: f32,
}

impl Gate {
    pub fn new(fs: f32) -> Self {
        Self {
            env: 0.0,
            gain: 0.0,
            hold: 0,
            hold_len: (0.12 * fs) as u32,
            att: coef(1.0, fs),
            rel: coef(150.0, fs),
            env_c: coef(5.0, fs),
        }
    }

    /// Returns the gain to apply (0..1).
    #[inline]
    pub fn run(&mut self, x: f32, threshold: f32) -> f32 {
        self.env += (x.abs() - self.env) * self.env_c;
        let open = self.env > threshold;
        if open {
            self.hold = self.hold_len;
        } else if self.hold > 0 {
            self.hold -= 1;
        }
        let target = if open || self.hold > 0 { 1.0 } else { 0.0 };
        let c = if target > self.gain { self.att } else { self.rel };
        self.gain += (target - self.gain) * c;
        self.gain
    }

    pub fn is_open(&self) -> bool {
        self.gain > 0.5
    }
}

/// RMS-detecting feed-forward compressor driven by a single "amount" knob.
pub struct Compressor {
    env: f32,
    att: f32,
    rel: f32,
}

impl Compressor {
    pub fn new(fs: f32) -> Self {
        Self { env: 0.0, att: coef(4.0, fs), rel: coef(90.0, fs) }
    }

    #[inline]
    pub fn gain(&mut self, x: f32, amount: f32) -> f32 {
        let p = x * x;
        let c = if p > self.env { self.att } else { self.rel };
        self.env += (p - self.env) * c;
        if amount <= 0.001 {
            return 1.0;
        }
        let level = lin_to_db(self.env.sqrt());
        let threshold = -12.0 - 18.0 * amount;
        let ratio = 1.5 + 4.5 * amount;
        let over = level - threshold;
        let reduction = if over > 0.0 { over * (1.0 - 1.0 / ratio) } else { 0.0 };
        let makeup = -threshold * (1.0 - 1.0 / ratio) * 0.45;
        db_to_lin(makeup - reduction)
    }
}

/// Split-band de-esser: only the sibilant band is ducked.
pub struct DeEsser {
    split: Biquad,
    env_hi: f32,
    env_all: f32,
    c: f32,
}

impl DeEsser {
    pub fn new(fs: f32) -> Self {
        let mut split = Biquad::default();
        split.set(Shape::HighPass, 5500.0, 0.7, fs);
        Self { split, env_hi: 0.0, env_all: 0.0, c: coef(3.0, fs) }
    }

    #[inline]
    pub fn run(&mut self, x: f32, amount: f32) -> f32 {
        let hi = self.split.run(x);
        self.env_hi += (hi.abs() - self.env_hi) * self.c;
        self.env_all += (x.abs() - self.env_all) * self.c;
        if amount <= 0.001 || self.env_all < 1e-5 {
            return x;
        }
        let share = self.env_hi / self.env_all;
        let threshold = 0.55 - 0.35 * amount;
        let duck = if share > threshold { ((share - threshold) * 3.0).min(0.9) * amount } else { 0.0 };
        x - hi * duck
    }
}

// --------------------------------------------------------- time based --

/// Stereo ping-pong delay with a darkening feedback path.
pub struct Delay {
    buf: [Vec<f32>; 2],
    pos: usize,
    tone: [Biquad; 2],
    time: f32,
    fs: f32,
}

impl Delay {
    pub fn new(fs: f32) -> Self {
        let len = (fs * 2.1) as usize;
        let mut tone = [Biquad::default(); 2];
        for t in tone.iter_mut() {
            t.set(Shape::LowPass, 4500.0, 0.6, fs);
        }
        Self { buf: [vec![0.0; len], vec![0.0; len]], pos: 0, tone, time: 0.3 * fs, fs }
    }

    #[inline]
    pub fn run(&mut self, l: f32, r: f32, ms: f32, feedback: f32) -> (f32, f32) {
        let len = self.buf[0].len();
        let target = (ms.clamp(20.0, 2000.0) * 0.001 * self.fs).min(len as f32 - 2.0);
        // Glide the read head instead of jumping, avoiding clicks on changes.
        self.time += (target - self.time) * 0.0005;
        let read = (self.pos as f32 - self.time + len as f32) % len as f32;
        let i0 = read as usize;
        let frac = read - i0 as f32;
        let i1 = (i0 + 1) % len;
        let dl = self.buf[0][i0] + (self.buf[0][i1] - self.buf[0][i0]) * frac;
        let dr = self.buf[1][i0] + (self.buf[1][i1] - self.buf[1][i0]) * frac;
        let fb = feedback.clamp(0.0, 0.92);
        // Ping-pong: each side feeds the other.
        self.buf[0][self.pos] = (l + r) * 0.5 + self.tone[0].run(dr) * fb;
        self.buf[1][self.pos] = self.tone[1].run(dl) * fb;
        self.pos = (self.pos + 1) % len;
        (dl, dr)
    }
}

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn run(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let y = self.buf[self.pos];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        let y = b - x;
        self.buf[self.pos] = x + b * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// Freeverb-style stereo reverb with pre-delay.
pub struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    pre: Vec<f32>,
    pre_pos: usize,
}

impl Reverb {
    pub fn new(fs: f32) -> Self {
        const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
        const SPREAD: usize = 23;
        let k = fs / 44_100.0;
        let mk_combs = |off: usize| {
            COMBS
                .iter()
                .map(|&n| Comb { buf: vec![0.0; ((n + off) as f32 * k) as usize], pos: 0, store: 0.0 })
                .collect::<Vec<_>>()
        };
        let mk_ap = |off: usize| {
            ALLPASSES
                .iter()
                .map(|&n| Allpass { buf: vec![0.0; ((n + off) as f32 * k) as usize], pos: 0 })
                .collect::<Vec<_>>()
        };
        Self {
            combs: [mk_combs(0), mk_combs(SPREAD)],
            allpasses: [mk_ap(0), mk_ap(SPREAD)],
            pre: vec![0.0; (fs * 0.025) as usize],
            pre_pos: 0,
        }
    }

    #[inline]
    pub fn run(&mut self, l: f32, r: f32, size: f32, damp: f32) -> (f32, f32) {
        let x = self.pre[self.pre_pos];
        self.pre[self.pre_pos] = (l + r) * 0.5 * 0.03;
        self.pre_pos = (self.pre_pos + 1) % self.pre.len();
        let feedback = 0.7 + 0.28 * size.clamp(0.0, 1.0);
        let damp = damp.clamp(0.0, 1.0) * 0.4;
        let mut out = [0.0f32; 2];
        for ch in 0..2 {
            let mut acc = 0.0;
            for c in self.combs[ch].iter_mut() {
                acc += c.run(x, feedback, damp);
            }
            for a in self.allpasses[ch].iter_mut() {
                acc = a.run(acc);
            }
            out[ch] = acc;
        }
        (out[0], out[1])
    }
}

/// Simple program-dependent peak limiter for the master bus.
pub struct Limiter {
    gain: f32,
    rel: f32,
}

impl Limiter {
    pub fn new(fs: f32) -> Self {
        Self { gain: 1.0, rel: coef(120.0, fs) }
    }

    #[inline]
    pub fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        let peak = l.abs().max(r.abs());
        let ceiling = 0.97;
        let needed = if peak * self.gain > ceiling { ceiling / peak } else { 1.0 };
        if needed < self.gain {
            self.gain = needed;
        } else {
            self.gain += (1.0 - self.gain) * self.rel;
        }
        ((l * self.gain).clamp(-1.0, 1.0), (r * self.gain).clamp(-1.0, 1.0))
    }
}
