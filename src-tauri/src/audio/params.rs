//! Lock-free parameters shared between the UI and the audio threads.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    #[inline]
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    #[inline]
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
}

macro_rules! params {
    ($($name:ident = $default:expr),* $(,)?) => {
        /// Every user-tweakable value. Names are the wire names used by the UI.
        pub struct Params { $(pub $name: AtomicF32,)* }

        impl Default for Params {
            fn default() -> Self {
                Self { $($name: AtomicF32::new($default),)* }
            }
        }

        impl Params {
            pub fn set(&self, name: &str, value: f32) -> bool {
                if !value.is_finite() {
                    return false;
                }
                match name {
                    $(stringify!($name) => { self.$name.set(value); true })*
                    _ => false,
                }
            }
        }
    };
}

params! {
    // Autotune
    tune_enabled = 1.0,
    tune_key = 0.0,        // tonic pitch class 0..11
    tune_scale = 1.0,      // index into scale::SCALES
    tune_retune_ms = 40.0, // 0 = hard tune
    tune_amount = 1.0,     // 0..1 correction strength
    tune_humanize = 0.3,   // 0..1 slower retune on sustained notes
    tune_flex = 0.0,       // cents of natural deviation left alone
    tune_vibrato_depth = 0.0, // cents of synthetic vibrato
    tune_vibrato_rate = 5.5,  // Hz
    tune_transpose = 0.0,  // semitones, applied to the voice
    tune_formant = 0.0,    // semitones of formant shift
    tune_mix = 1.0,        // dry/wet
    tune_sensitivity = 0.8, // voicing clarity threshold

    // Harmony / doubling
    harmony_a = 0.0,       // diatonic degrees, 0 = off (±7 = octave)
    harmony_b = 0.0,
    harmony_level = 0.5,
    harmony_spread = 0.7,
    doubler = 0.0,         // 0..1

    // Channel strip
    gate_db = -50.0,
    hpf_hz = 90.0,
    eq_low_db = 0.0,
    eq_presence_db = 2.0,
    eq_air_db = 2.0,
    deesser = 0.4,
    comp = 0.5,
    delay_mix = 0.0,
    delay_ms = 320.0,
    delay_feedback = 0.3,
    reverb_mix = 0.18,
    reverb_size = 0.7,
    reverb_damp = 0.4,
    vocal_gain_db = 0.0,

    // Mix
    master = 0.9,
    instrumental = 0.85,
    guide = 0.0,
    vocal = 0.9,
    monitor = 1.0,
    record_latency_ms = -1.0, // <0 = automatic
    drone = 0.0,          // Voice Lab key drone level
}

pub struct Flags {
    pub playing: AtomicBool,
    pub recording: AtomicBool,
}

impl Default for Flags {
    fn default() -> Self {
        Self { playing: AtomicBool::new(false), recording: AtomicBool::new(false) }
    }
}
