//! End-to-end check of the app's separation chain on a real file:
//! decode → separate in segments → stream-resample → growing buffers →
//! stem cache → reload from cache.
//! `cargo run --release --example separation_flow -- model.onnx song.m4a out_dir`
use open_karaoke_lib::audio::decode::{decode_file, write_wav, StreamResampler};
use open_karaoke_lib::audio::separate::{Growing, Separator, MODEL, MODEL_RATE};
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (model, song, out) = (Path::new(&args[0]), Path::new(&args[1]), Path::new(&args[2]));
    let device_rate = 48_000u32;

    let mix = decode_file(song, MODEL_RATE).expect("decode");
    let n = mix.len() / 2;
    let frames = (n as f64 * device_rate as f64 / MODEL_RATE as f64).round() as usize;
    let (inst_buf, mut inst_w) = Growing::new(frames * 2);
    let (vox_buf, mut vox_w) = Growing::new(frames * 2);

    let mut sep = Separator::load(model, MODEL.config).expect("model");
    let mut rs_i = StreamResampler::new(MODEL_RATE, device_rate).unwrap();
    let mut rs_v = StreamResampler::new(MODEL_RATE, device_rate).unwrap();
    let (mut inst_all, mut vox_all, mut tmp, mut vox) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let t0 = Instant::now();
    let mut first_ready = None;
    sep.run(&mix, |seg, _| {
        let base = inst_all.len();
        vox.clear();
        vox.extend(seg.iter().enumerate().map(|(i, &v)| mix[base + i] - v));
        inst_all.extend_from_slice(seg);
        vox_all.extend_from_slice(&vox);
        rs_i.push(seg, &mut tmp).unwrap();
        inst_w.append(&tmp);
        rs_v.push(&vox, &mut tmp).unwrap();
        vox_w.append(&tmp);
        // What the audio thread would see right now.
        let ready_s = inst_buf.ready_slice().len() as f64 / 2.0 / device_rate as f64;
        if first_ready.is_none() && ready_s >= 30.0 {
            first_ready = Some(t0.elapsed());
        }
        true
    })
    .expect("separate");
    rs_i.flush(&mut tmp).unwrap();
    inst_w.append(&tmp);
    rs_v.flush(&mut tmp).unwrap();
    vox_w.append(&tmp);
    let before_finish = inst_buf.ready_slice().len();
    inst_w.finish();
    vox_w.finish();
    println!(
        "{:.1} s song separated in {:.1?}; 30 s playable after {:.1?}",
        n as f64 / MODEL_RATE as f64,
        t0.elapsed(),
        first_ready.unwrap_or_default()
    );
    println!(
        "growing buffer: {} of {} samples filled by the resampler ({:.4}% padding), done = {}",
        before_finish,
        inst_buf.capacity(),
        100.0 * (inst_buf.capacity() - before_finish) as f64 / inst_buf.capacity() as f64,
        inst_buf.is_done() && vox_buf.is_done()
    );

    // Instrumental + vocals must reconstruct the original exactly.
    let max_err = (0..mix.len()).map(|i| (inst_all[i] + vox_all[i] - mix[i]).abs()).fold(0f32, f32::max);
    println!("stems sum back to the mix: max error {max_err:.2e}");

    std::fs::create_dir_all(out).unwrap();
    let (iw, vw) = (out.join("t.instrumental.wav"), out.join("t.vocals.wav"));
    write_wav(&iw, &inst_all, MODEL_RATE).unwrap();
    write_wav(&vw, &vox_all, MODEL_RATE).unwrap();
    let t1 = Instant::now();
    let cached = decode_file(&iw, device_rate).unwrap();
    println!(
        "cache reload: {} frames at 48 kHz in {:.0?} (live buffer had {}; diff {} frames)",
        cached.len() / 2,
        t1.elapsed(),
        frames,
        (cached.len() / 2) as i64 - frames as i64
    );
}
