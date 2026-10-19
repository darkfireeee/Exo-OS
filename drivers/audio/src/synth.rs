//! exo-audio-drivers — Synthèse PCM purement entière (#![no_std], SRV-02)
//!
//! Spécification conforme à `SPEC-AUDIO-STRATA.md` :
//! - Format : PCM 44100 Hz, 16-bit signé, Stéréo (2 canaux L+R).
//! - Aucune instruction flottante ni SIMD : arithmétique entière et table sinus 256 points.

pub const SAMPLE_RATE: u32 = 44100;

/// Table pré-calculée d'un cycle sinus complet (256 points, amplitude nominale 14000).
pub const SINE_TABLE_256: [i16; 256] = [
    0, 343, 686, 1029, 1372, 1713, 2054, 2393, 2731, 3067, 3401, 3733, 4063, 4391, 4716, 5038,
    5357, 5673, 5985, 6294, 6599, 6900, 7197, 7489, 7777, 8061, 8339, 8613, 8881, 9144, 9401, 9653,
    9899, 10139, 10373, 10600, 10822, 11036, 11244, 11446, 11640, 11827, 12008, 12181, 12346, 12505,
    12655, 12798, 12934, 13061, 13181, 13293, 13397, 13492, 13580, 13659, 13730, 13793, 13848, 13894,
    13932, 13962, 13983, 13995, 14000, 13995, 13983, 13962, 13932, 13894, 13848, 13793, 13730, 13659,
    13580, 13492, 13397, 13293, 13181, 13061, 12934, 12798, 12655, 12505, 12346, 12181, 12008, 11827,
    11640, 11446, 11244, 11036, 10822, 10600, 10373, 10139, 9899, 9653, 9401, 9144, 8881, 8613,
    8339, 8061, 7777, 7489, 7197, 6900, 6599, 6294, 5985, 5673, 5357, 5038, 4716, 4391, 4063,
    3733, 3401, 3067, 2731, 2393, 2054, 1713, 1372, 1029, 686, 343, 0, -343, -686, -1029,
    -1372, -1713, -2054, -2393, -2731, -3067, -3401, -3733, -4063, -4391, -4716, -5038, -5357, -5673,
    -5985, -6294, -6599, -6900, -7197, -7489, -7777, -8061, -8339, -8613, -8881, -9144, -9401, -9653,
    -9899, -10139, -10373, -10600, -10822, -11036, -11244, -11446, -11640, -11827, -12008, -12181, -12346,
    -12505, -12655, -12798, -12934, -13061, -13181, -13293, -13397, -13492, -13580, -13659, -13730, -13793,
    -13848, -13894, -13932, -13962, -13983, -13995, -14000, -13995, -13983, -13962, -13932, -13894, -13848,
    -13793, -13730, -13659, -13580, -13492, -13397, -13293, -13181, -13061, -12934, -12798, -12655, -12505,
    -12346, -12181, -12008, -11827, -11640, -11446, -11244, -11036, -10822, -10600, -10373, -10139, -9899,
    -9653, -9401, -9144, -8881, -8613, -8339, -8061, -7777, -7489, -7197, -6900, -6599, -6294,
    -5985, -5673, -5357, -5038, -4716, -4391, -4063, -3733, -3401, -3067, -2731, -2393, -2054,
    -1713, -1372, -1029, -686, -343,
];

/// Échantillonne le sinus par interpolation entière directe (phase u32 : [0..2^32 - 1]).
#[inline(always)]
pub fn sample_sine(phase: u32) -> i16 {
    let idx = (phase >> 24) as usize;
    SINE_TABLE_256[idx & 0xFF]
}

/// Synthétise un son Beep (Terminal Bell) avec enveloppe anti-click (fade-in 5 ms, fade-out 20 ms).
///
/// Retourne le nombre total d'échantillons `i16` écrits (frames × 2 canaux).
pub fn synthesize_beep(freq_hz: u32, dur_ms: u32, out: &mut [i16]) -> usize {
    let n_frames = ((SAMPLE_RATE as u64 * dur_ms as u64) / 1000) as usize;
    let _total_samples = n_frames.saturating_mul(2);
    let max_frames = out.len() / 2;
    let frames = n_frames.min(max_frames);

    if frames == 0 {
        return 0;
    }

    let phase_step = (((freq_hz as u64) << 32) / SAMPLE_RATE as u64) as u32;
    let fade_in_frames = ((SAMPLE_RATE as u64 * 5) / 1000) as usize; // 5ms
    let fade_out_frames = ((SAMPLE_RATE as u64 * 20) / 1000) as usize; // 20ms

    let mut phase = 0u32;
    for i in 0..frames {
        let raw = sample_sine(phase) as i32;
        phase = phase.wrapping_add(phase_step);

        // Facteur d'enveloppe entière Q15 (0..32768)
        let env_in = if i < fade_in_frames && fade_in_frames > 0 {
            ((i as u64 * 32768) / fade_in_frames as u64) as i32
        } else {
            32768
        };

        let env_out = if i >= frames.saturating_sub(fade_out_frames) && fade_out_frames > 0 {
            let rem = frames.saturating_sub(i);
            ((rem as u64 * 32768) / fade_out_frames as u64) as i32
        } else {
            32768
        };

        let env = (env_in * env_out) >> 15;
        let s = ((raw * env) >> 15) as i16;

        out[i * 2] = s;     // L
        out[i * 2 + 1] = s; // R
    }

    frames * 2
}

/// Synthétise le carillon de démarrage (Boot Chime) :
/// - G4 (392 Hz) pendant 200 ms (fade-in 10ms, fade-out 190ms)
/// - Silence 50 ms
/// - C5 (523 Hz) pendant 300 ms (fade-out quadratique)
pub fn synthesize_boot_chime(out: &mut [i16]) -> usize {
    let g4_frames = ((SAMPLE_RATE as u64 * 200) / 1000) as usize; // 8820 frames
    let silence_frames = ((SAMPLE_RATE as u64 * 50) / 1000) as usize; // 2205 frames
    let c5_frames = ((SAMPLE_RATE as u64 * 300) / 1000) as usize; // 13230 frames

    let total_frames = g4_frames + silence_frames + c5_frames;
    let max_frames = out.len() / 2;
    let frames = total_frames.min(max_frames);

    let mut cursor = 0usize;

    // 1. Note G4 (392 Hz, 200 ms)
    let g4_phase_step = (((392u64) << 32) / SAMPLE_RATE as u64) as u32;
    let mut phase = 0u32;
    let g4_limit = g4_frames.min(frames);
    let attack_frames = ((SAMPLE_RATE as u64 * 10) / 1000) as usize; // 10ms
    for i in 0..g4_limit {
        let raw = sample_sine(phase) as i32;
        phase = phase.wrapping_add(g4_phase_step);

        let env = if i < attack_frames {
            ((i as u64 * 32768) / attack_frames as u64) as i32
        } else {
            let decay = g4_frames.saturating_sub(i);
            let rem_frames = g4_frames.saturating_sub(attack_frames);
            if rem_frames > 0 {
                ((decay as u64 * 32768) / rem_frames as u64) as i32
            } else {
                0
            }
        };

        let s = ((raw * env) >> 15) as i16;
        out[cursor * 2] = s;
        out[cursor * 2 + 1] = s;
        cursor += 1;
    }

    // 2. Silence (50 ms)
    let silence_limit = (g4_frames + silence_frames).min(frames);
    while cursor < silence_limit {
        out[cursor * 2] = 0;
        out[cursor * 2 + 1] = 0;
        cursor += 1;
    }

    // 3. Note C5 (523 Hz, 300 ms avec fade-out quadratique)
    let c5_phase_step = (((523u64) << 32) / SAMPLE_RATE as u64) as u32;
    phase = 0;
    let c5_limit = (g4_frames + silence_frames + c5_frames).min(frames);
    let c5_count = c5_frames;
    let mut i = 0usize;
    while cursor < c5_limit {
        let raw = sample_sine(phase) as i32;
        phase = phase.wrapping_add(c5_phase_step);

        // Fade-out quadratique en Q15 : env = 1.0 - (i/N)^2
        // frac_q15 = (i * 32768) / N
        // env_q15 = 32768 - (frac_q15 * frac_q15 >> 15)
        let frac_q15 = if c5_count > 0 {
            ((i as u64 * 32768) / c5_count as u64) as i32
        } else {
            32768
        };
        let frac_sq = (frac_q15 * frac_q15) >> 15;
        let env = (32768 - frac_sq).max(0);

        let s = ((raw * env) >> 15) as i16;
        out[cursor * 2] = s;
        out[cursor * 2 + 1] = s;
        cursor += 1;
        i += 1;
    }

    cursor * 2
}

/// Synthétise l'alerte de sécurité (Security Alert) :
/// - 200 Hz pendant 1000 ms avec saturation harmonique contrôlée (soft clipping à 12000).
pub fn synthesize_security_alert(out: &mut [i16]) -> usize {
    let n_frames = ((SAMPLE_RATE as u64 * 1000) / 1000) as usize; // 44100 frames
    let max_frames = out.len() / 2;
    let frames = n_frames.min(max_frames);

    let phase_step = (((200u64) << 32) / SAMPLE_RATE as u64) as u32;
    let mut phase = 0u32;

    for i in 0..frames {
        let raw = sample_sine(phase) as i32;
        phase = phase.wrapping_add(phase_step);

        // Saturation légère : amplification x 1.4 puis clamp à 12000
        let amplified = (raw * 14) / 10;
        let clamped = amplified.clamp(-12000, 12000) as i16;

        out[i * 2] = clamped;
        out[i * 2 + 1] = clamped;
    }

    frames * 2
}
