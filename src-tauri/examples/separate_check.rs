//! Checks the separator on a synthetic song whose parts are known:
//! `cargo run --release --example separate_check -- model.onnx [seconds]`
use open_karaoke_lib::audio::separate::{MdxConfig, Separator, MODEL_RATE};
use std::f32::consts::TAU;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs: f32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let sr = MODEL_RATE as f32;
    let n = (secs * sr) as usize;
    let mut seed = 9u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    };

    // Instrumental: chords, bass, hats and kicks, panned.
    let chords = [[55, 59, 62], [60, 64, 67], [57, 60, 64], [62, 66, 69]];
    let hz = |m: i32| 440.0 * 2f32.powf((m as f32 - 69.0) / 12.0);
    let mut inst = vec![0f32; n * 2];
    for i in 0..n {
        let t = i as f32 / sr;
        let c = chords[(t / 2.0) as usize % 4];
        let pad: f32 = c.iter().map(|&m| (TAU * hz(m) * t).sin() + 0.3 * (TAU * 2.0 * hz(m) * t).sin()).sum::<f32>() * 0.05;
        let bass = (TAU * hz(c[0] - 24) * t).sin() * 0.15;
        let beat = t % 0.5;
        let kick = (TAU * (50.0 + 80.0 * (-beat * 30.0).exp()) * beat).sin() * (-beat * 12.0).exp() * 0.4;
        let hat = if (t % 0.25) < 0.03 { rnd() * 0.08 } else { 0.0 };
        inst[i * 2] = pad * 1.2 + bass + kick + hat;
        inst[i * 2 + 1] = pad * 0.8 + bass + kick + hat * 0.6;
    }

    // "Voice": a vibrato-rich glottal-like tone through vowel formants,
    // singing a melody in syllables, centred like a lead vocal.
    let melody = [67, 69, 71, 72, 71, 69, 67, 64];
    let mut voice = vec![0f32; n];
    let formants = [(700.0f32, 0.9f32), (1200.0, 0.5), (2600.0, 0.25)];
    let mut phase = 0f32;
    let mut res = [[0f32; 2]; 3];
    for i in 0..n {
        let t = i as f32 / sr;
        let note = melody[(t / 0.6) as usize % melody.len()];
        let f0 = hz(note) * (1.0 + 0.012 * (TAU * 5.5 * t).sin());
        phase = (phase + f0 / sr) % 1.0;
        let src = 1.0 - 2.0 * phase; // sawtooth ≈ glottal source
        let syl = (t % 0.6) / 0.6;
        let env = (syl * 12.0).min(1.0) * (1.0 - ((syl - 0.8).max(0.0) * 5.0));
        let mut y = 0.0;
        for (k, (fc, g)) in formants.iter().enumerate() {
            // Two-pole resonator per formant.
            let r = 0.995f32;
            let w = TAU * fc / sr;
            let out = src * (1.0 - r) + 2.0 * r * w.cos() * res[k][0] - r * r * res[k][1];
            res[k][1] = res[k][0];
            res[k][0] = out;
            y += out * g;
        }
        voice[i] = y * env * 0.5;
    }
    let peak = voice.iter().fold(0f32, |a, &v| a.max(v.abs()));
    voice.iter_mut().for_each(|v| *v *= 0.35 / peak);

    let mix: Vec<f32> = (0..n * 2).map(|j| inst[j] + voice[j / 2]).collect();

    let t0 = Instant::now();
    let rss = || {
        let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
    };
    println!("rss before load: {:.0} MB", rss());
    let mut sep = Separator::load(args[0].as_ref(), MdxConfig::INST_HQ_3).expect("load model");
    println!("rss after load: {:.0} MB", rss());
    println!("model loaded in {:?} ({:?})", t0.elapsed(), sep.config());
    let t1 = Instant::now();
    let mut est = Vec::with_capacity(n * 2);
    let mut first_segment = None;
    sep.run(&mix, |seg, _| {
        if first_segment.is_none() {
            first_segment = Some(t1.elapsed());
        }
        est.extend_from_slice(seg);
        true
    })
    .expect("separate");
    let took = t1.elapsed().as_secs_f32();
    println!(
        "separated {secs:.0} s of audio in {took:.1} s ({:.1}x real time); first {:.1} s ready after {:.2?}",
        secs / took,
        sep.config().step() as f32 / sr,
        first_segment.unwrap()
    );
    assert_eq!(est.len(), mix.len());

    let energy = |v: &mut dyn Iterator<Item = f32>| v.map(|x| x * x).sum::<f32>();
    let sig = energy(&mut inst.iter().copied());
    let before = energy(&mut (0..n * 2).map(|j| mix[j] - inst[j]));
    let after = energy(&mut (0..n * 2).map(|j| est[j] - inst[j]));
    println!(
        "instrumental SDR: mix {:.1} dB → separated {:.1} dB (higher is cleaner)",
        10.0 * (sig / before).log10(),
        10.0 * (sig / after).log10()
    );
}
