//! Musical scales shared with the UI (indices must match `src/lib/music.ts`).

pub const SCALES: &[&[u8]] = &[
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], // Chromatic
    &[0, 2, 4, 5, 7, 9, 11],                 // Major
    &[0, 2, 3, 5, 7, 8, 10],                 // Natural minor
    &[0, 2, 3, 5, 7, 8, 11],                 // Harmonic minor
    &[0, 2, 3, 5, 7, 9, 11],                 // Melodic minor
    &[0, 2, 3, 5, 7, 9, 10],                 // Dorian
    &[0, 1, 3, 5, 7, 8, 10],                 // Phrygian
    &[0, 2, 4, 6, 7, 9, 11],                 // Lydian
    &[0, 2, 4, 5, 7, 9, 10],                 // Mixolydian
    &[0, 2, 4, 7, 9],                        // Major pentatonic
    &[0, 3, 5, 7, 10],                       // Minor pentatonic
    &[0, 3, 5, 6, 7, 10],                    // Blues
];

pub fn scale(idx: usize) -> &'static [u8] {
    SCALES.get(idx).copied().unwrap_or(SCALES[0])
}

pub fn in_scale(note: i32, tonic: i32, scale: &[u8]) -> bool {
    let pc = (note - tonic).rem_euclid(12) as u8;
    scale.contains(&pc)
}

/// Nearest scale note to the fractional MIDI pitch `m`.
pub fn nearest(m: f32, tonic: i32, scale: &[u8]) -> i32 {
    let base = m.round() as i32;
    let mut best = base;
    let mut best_d = f32::MAX;
    for n in base - 3..=base + 3 {
        if in_scale(n, tonic, scale) {
            let d = (m - n as f32).abs();
            if d < best_d {
                best_d = d;
                best = n;
            }
        }
    }
    best
}

/// Move `note` by `degrees` steps within the scale (diatonic harmony).
/// `±7` always means an octave, whatever the scale length.
pub fn shift_degrees(note: i32, degrees: i32, tonic: i32, scale: &[u8]) -> i32 {
    if degrees.abs() == 7 {
        return note + 12 * degrees.signum();
    }
    if scale.len() == 12 {
        // Chromatic has no diatonic steps; harmonise with major intervals.
        const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
        return note + 12 * degrees.div_euclid(7) + MAJOR[degrees.rem_euclid(7) as usize];
    }
    let len = scale.len() as i32;
    let rel = note - tonic;
    let oct = rel.div_euclid(12);
    let pc = rel.rem_euclid(12) as u8;
    let idx = scale
        .iter()
        .enumerate()
        .min_by_key(|(_, &s)| (s as i32 - pc as i32).abs())
        .map(|(i, _)| i as i32)
        .unwrap_or(0);
    let t = idx + degrees;
    tonic + (oct + t.div_euclid(len)) * 12 + scale[t.rem_euclid(len) as usize] as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_c_major() {
        let s = SCALES[1];
        assert_eq!(nearest(61.4, 0, s), 62); // C#+ → D
        assert_eq!(nearest(60.4, 0, s), 60);
        assert_eq!(nearest(65.6, 0, s), 65); // F#- → F (F nearer than G)
    }

    #[test]
    fn diatonic_harmony() {
        let s = SCALES[1];
        assert_eq!(shift_degrees(60, 2, 0, s), 64); // C → E
        assert_eq!(shift_degrees(62, 2, 0, s), 65); // D → F (minor third)
        assert_eq!(shift_degrees(71, 2, 0, s), 74); // B → D above
        assert_eq!(shift_degrees(60, -2, 0, s), 57); // C → A below
        assert_eq!(shift_degrees(60, 7, 0, s), 72);
    }
}
