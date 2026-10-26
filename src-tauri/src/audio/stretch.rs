//! Key and tempo changes for the backing track (Signalsmith Stretch).

use signalsmith_stretch::Stretch;

/// Render `stereo` transposed by `semitones` and played at `tempo` × speed.
pub fn render(stereo: &[f32], fs: u32, semitones: f32, tempo: f32) -> Vec<f32> {
    if semitones.abs() < 1e-3 && (tempo - 1.0).abs() < 1e-3 {
        return stereo.to_vec();
    }
    let in_frames = stereo.len() / 2;
    let out_frames = (in_frames as f64 / tempo.clamp(0.5, 2.0) as f64).round() as usize;
    let mut st = Stretch::preset_default(2, fs);
    st.set_transpose_factor_semitones(semitones, None);
    let mut out = vec![0f32; out_frames * 2];
    if !st.exact(stereo, &mut out) {
        // Fall back to streaming if the input is too short for `exact`.
        st.reset();
        st.process(stereo, &mut out);
    }
    out
}
