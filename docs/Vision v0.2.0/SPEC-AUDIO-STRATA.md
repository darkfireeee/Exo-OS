# SPEC-AUDIO-STRATA — Son Système ExoOS v0.2.0
## Chime Boot · Terminal Bell · Alertes Sécurité

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** NOUVEAU

---

## 1. Philosophie

Un vrai ordinateur a une voix. Pas pour jouer de la musique — pour confirmer
qu'il est vivant, qu'il surveille, qu'il réagit. L'audio v0.2.0 est le langage
sonore d'ExoOS : trois phrases, trois contextes, trois significations claires.

```
Son de démarrage  →  "Je suis prêt."
Terminal bell     →  "Ce n'est pas valide."
Alerte sécurité   →  "Je surveille, et quelque chose se passe."
```

L'audio multimédia, les fichiers son, le mixer : v0.3.0.

---

## 2. Architecture

```
                    ┌──────────────────────────────┐
                    │  audio_server (Ring1, Vague 4) │
                    │                              │
                    │  Sons embarqués (static PCM) │
                    │  Synthèse beep en mémoire    │
                    │  IPC : PlaySound / Beep / Stop│
                    └──────────┬───────────────────┘
                               │ Pilote
               ┌───────────────┴────────────────┐
               │                                │
       ┌───────▼──────┐              ┌──────────▼─────┐
       │  hda driver  │              │ virtio_sound   │
       │  (hardware)  │              │  (VM/QEMU)     │
       └──────────────┘              └────────────────┘

Émetteurs autorisés :
  init_server → PLAY_SYSTEM_SOUND(BOOT_COMPLETE)
  tty_server  → BEEP(800, 100)          [BEL 0x07 reçu]
  exo_shield  → PLAY_SYSTEM_SOUND(SECURITY_ALERT) ou BEEP(300/200, 150/1000)
```

---

## 3. IPC Protocol audio_server

```rust
#[repr(u32)]
pub enum AudioMsgType {
    PlaySystemSound = 0,
    Beep            = 1,
    Stop            = 2,
}

#[repr(u32)]
pub enum SoundId {
    BootComplete    = 0,
    SecurityAlert   = 1,
}

// PlaySystemSound payload (8 octets) :
// [0..4] : sound_id (SoundId as u32)
// [4..8] : reserved

// Beep payload (8 octets) :
// [0..4] : freq_hz (u32)
// [4..8] : duration_ms (u32)

// Stop payload : vide
```

**Capabilities requises :**
- `CAP_AUDIO_PLAY_SYSTEM` : init_server, exo_shield (sons système)
- `CAP_AUDIO_BELL` : tty_server (bell uniquement)
- **Aucun processus Ring3 n'a accès à audio_server en v0.2.0**

---

## 4. Sons Embarqués — Spécification PCM

### 4.1 — Boot Chime

- Format : PCM 44100 Hz, 16-bit LE, stéréo
- Durée : ~500ms (22050 échantillons stéréo = 88200 octets)
- Caractère : deux notes courtes ascendantes, ton doux et net
- Notes : G4 (392 Hz) 200ms → C5 (523 Hz) 300ms avec fade-out
- Synthèse à la compilation :

```rust
// tools/gen_sounds.rs (exécuté au build)
fn gen_boot_chime() -> Vec<i16> {
    const SAMPLE_RATE: f32 = 44100.0;
    let mut samples = Vec::new();

    // Note G4 : 392 Hz, 200ms, envelope ADSR simple
    for i in 0..(SAMPLE_RATE * 0.2) as usize {
        let t = i as f32 / SAMPLE_RATE;
        let env = if t < 0.01 { t / 0.01 }
                  else { 1.0 - (t - 0.01) / 0.19 };
        let s = (f32::sin(2.0 * PI * 392.0 * t) * 14000.0 * env) as i16;
        samples.push(s); samples.push(s); // L + R
    }

    // Gap 50ms silencieux
    samples.extend(vec![0i16; (SAMPLE_RATE * 0.05) as usize * 2]);

    // Note C5 : 523 Hz, 300ms, fade-out progressif
    for i in 0..(SAMPLE_RATE * 0.3) as usize {
        let t = i as f32 / SAMPLE_RATE;
        let env = 1.0 - (t / 0.3).powi(2);
        let s = (f32::sin(2.0 * PI * 523.0 * t) * 14000.0 * env) as i16;
        samples.push(s); samples.push(s);
    }

    samples
}
```

### 4.2 — Security Alert (pour exo_shield)

Le son est synthétisé à l'exécution par audio_server depuis les paramètres BEEP.

```rust
// HIGH threat : 3 bips courts (exo_shield appelle Beep × 3)
// CRITICAL threat : 1 bip long (exo_shield appelle PlaySystemSound(SecurityAlert))
// SECURITY_ALERT embarqué : 200Hz, 1000ms, légère distorsion (sinus saturé)
fn gen_security_alert() -> Vec<i16> {
    const SAMPLE_RATE: f32 = 44100.0;
    (0..(SAMPLE_RATE * 1.0) as usize).flat_map(|i| {
        let t = i as f32 / SAMPLE_RATE;
        let raw = f32::sin(2.0 * PI * 200.0 * t) * 1.4; // saturation légère
        let s = (raw.clamp(-1.0, 1.0) * 12000.0) as i16;
        [s, s] // L + R
    }).collect()
}
```

---

## 5. Synthèse Beep (Terminal Bell)

```rust
// audio_server/src/beep.rs
pub fn synthesize_and_play(driver: &dyn AudioDevice, freq_hz: u32, dur_ms: u32) {
    const SAMPLE_RATE: u32 = 44100;
    let n_samples = (SAMPLE_RATE * dur_ms / 1000) as usize;

    let pcm: Vec<i16> = (0..n_samples).flat_map(|i| {
        let t = i as f32 / SAMPLE_RATE as f32;
        // Fade in 5ms + fade out 20ms pour éviter les clicks
        let env = {
            let fade_in  = (t / 0.005).min(1.0);
            let fade_out = ((dur_ms as f32 / 1000.0 - t) / 0.020).min(1.0).max(0.0);
            fade_in * fade_out
        };
        let s = (f32::sin(2.0 * PI * freq_hz as f32 * t) * 12000.0 * env) as i16;
        [s, s]
    }).collect();

    let _ = driver.play_pcm(&pcm, SAMPLE_RATE, 2);
}
```

---

## 6. Fallback Silencieux

Si `audio_server` est absent ou en erreur, **aucun composant ne panique**.

```rust
// Dans init_server, tty_server, exo_shield :
fn play_sound(sound: AudioRequest) {
    match ipc_send_timeout(PID_AUDIO, &sound, timeout_ms: 50) {
        Ok(_) => {}
        Err(_) => { /* silent fallback — log DEBUG uniquement */ }
    }
}
```

Le système est **sound-optional** : il fonctionne silencieusement si aucun hardware audio n'est détecté (serveur sans carte son).

---

## 7. Tests Requis

```
audio_test::boot_chime_played_on_ring1_complete  PASS
audio_test::bell_triggered_by_bel_char           PASS
audio_test::shield_high_three_beeps              PASS
audio_test::shield_critical_long_tone            PASS
audio_test::no_ring3_access_to_audio             PASS
audio_test::silent_fallback_on_no_hardware       PASS
audio_test::beep_fade_no_click                   PASS  ← qualité audio
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-AUDIO-STRATA.md*
