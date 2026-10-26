//! Offline check of the song pipeline on real files:
//! `cargo run --release --example pipeline -- instrumental.m4a original.m4a`
use croonly_lib::audio::{analysis, decode, stretch};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sr = 48_000;
    let t = Instant::now();
    let inst = decode::decode_file(args[0].as_ref(), sr).expect("decode instrumental");
    println!("decoded {} s in {:?}", inst.len() / 2 / sr as usize, t.elapsed());
    let key = analysis::detect_key(&analysis::mono(&inst), sr);
    println!("key {key:?}");
    if let Some(orig) = args.get(1) {
        let orig = decode::decode_file(orig.as_ref(), sr).expect("decode original");
        let t = Instant::now();
        let a = analysis::align(&analysis::mono(&inst), &analysis::mono(&orig), sr, 45.0);
        println!("alignment {a:?} in {:?}", t.elapsed());
    }
    let t = Instant::now();
    let shifted = stretch::render(&inst, sr, 2.0, 1.1);
    println!("+2 st @110%: {} s → {} s in {:?}", inst.len() / 2 / sr as usize, shifted.len() / 2 / sr as usize, t.elapsed());
}
