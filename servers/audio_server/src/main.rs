#![no_std]
#![no_main]

//! # audio_server — Serveur Audio Système Ring 1 (PID 16, Vague 4)
//!
//! Conforme à `SPEC-AUDIO-STRATA.md` :
//! - Chime de démarrage ("Je suis prêt.")
//! - Terminal Bell ("Ce n'est pas valide.")
//! - Alerte sécurité ("Je surveille, et quelque chose se passe.")
//! - Fallback silencieux (sound-optional) si aucun contrôleur audio physique n'est détecté.

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use exo_audio_drivers::{
    dummy::DummyAudioDevice,
    hda::HdaDriver,
    synth,
    virtio_sound::VirtioSoundDriver,
    AudioDevice, DeviceHandle, DeviceInfo, ExoDriver,
};
use exo_syscall_abi as syscall;

// Buffer de synthèse PCM (65536 échantillons = ~743 ms de stéréo 44.1 kHz)
const PCM_BUFFER_LEN: usize = 65536;

struct AudioServerState {
    hda: HdaDriver,
    virtio: VirtioSoundDriver,
    dummy: DummyAudioDevice,
    active_backend: AudioBackend,
    pcm_buffer: [i16; PCM_BUFFER_LEN],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioBackend {
    Hda,
    Virtio,
    Dummy,
}

struct StateCell(UnsafeCell<AudioServerState>);
unsafe impl Sync for StateCell {}

static STATE: StateCell = StateCell(UnsafeCell::new(AudioServerState {
    hda: HdaDriver::new(),
    virtio: VirtioSoundDriver::new(),
    dummy: DummyAudioDevice::new(),
    active_backend: AudioBackend::Dummy,
    pcm_buffer: [0i16; PCM_BUFFER_LEN],
}));

#[inline(always)]
fn state_mut() -> &'static mut AudioServerState {
    unsafe { &mut *STATE.0.get() }
}

fn boot_log(msg: &[u8]) {
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            msg.as_ptr() as u64,
            msg.len() as u64,
            1,
        );
    }
}

/// Tente de détecter et d'attacher un périphérique audio PCI (Intel HDA ou VirtIO Sound).
fn probe_audio_hardware() {
    let s = state_mut();

    // 1. Recherche d'un contrôleur Intel HDA (Class 0x04, Subclass 0x03)
    let mut dev_info = syscall::PciDeviceInfo::default();
    let rc = unsafe {
        syscall::syscall5(
            syscall::SYS_PCI_FIND_DEVICE,
            0, // vendor (0 = wildcard)
            0, // device (0 = wildcard)
            0x04, // class Multimedia
            0x03, // subclass Audio Controller
            &mut dev_info as *mut syscall::PciDeviceInfo as u64,
        )
    };

    if rc >= 0 && dev_info.bar0_phys != 0 {
        boot_log(b"audio_server: Intel HDA controller discovered\n");
        let bdf = ((dev_info.bus as u32) << 8)
            | ((dev_info.device as u32) << 3)
            | (dev_info.function as u32);
        let info = DeviceInfo {
            vendor_id: dev_info.vendor_id,
            device_id: dev_info.device_id,
            class: dev_info.class_code,
            subclass: dev_info.subclass,
            bdf_raw: bdf,
            bar0_phys: dev_info.bar0_phys,
            bar0_size: dev_info.bar0_size,
            irq_line: dev_info.irq_line,
        };

        if let Ok(handle) = DeviceHandle::claim_and_map(info) {
            if s.hda.attach(handle).is_ok() {
                boot_log(b"audio_server: Intel HDA attached and ready\n");
                s.active_backend = AudioBackend::Hda;
                return;
            }
        }
    }

    // 2. Recherche VirtIO Sound (Device ID 0x1059)
    let mut virtio_info = syscall::PciDeviceInfo::default();
    let v_rc = unsafe {
        syscall::syscall5(
            syscall::SYS_PCI_FIND_DEVICE,
            exo_audio_drivers::virtio_sound::VIRTIO_VENDOR_ID as u64,
            exo_audio_drivers::virtio_sound::VIRTIO_SOUND_DEVICE_ID as u64,
            0,
            0,
            &mut virtio_info as *mut syscall::PciDeviceInfo as u64,
        )
    };

    if v_rc >= 0 && virtio_info.bar0_phys != 0 {
        boot_log(b"audio_server: VirtIO Sound controller discovered\n");
        let bdf = ((virtio_info.bus as u32) << 8)
            | ((virtio_info.device as u32) << 3)
            | (virtio_info.function as u32);
        let info = DeviceInfo {
            vendor_id: virtio_info.vendor_id,
            device_id: virtio_info.device_id,
            class: virtio_info.class_code,
            subclass: virtio_info.subclass,
            bdf_raw: bdf,
            bar0_phys: virtio_info.bar0_phys,
            bar0_size: virtio_info.bar0_size,
            irq_line: virtio_info.irq_line,
        };

        if let Ok(handle) = DeviceHandle::claim_and_map(info) {
            if s.virtio.attach(handle).is_ok() {
                boot_log(b"audio_server: VirtIO Sound attached and ready\n");
                s.active_backend = AudioBackend::Virtio;
                return;
            }
        }
    }

    boot_log(b"audio_server: no hardware sound detected, using sound-optional fallback\n");
    s.active_backend = AudioBackend::Dummy;
}

fn get_active_device() -> &'static dyn AudioDevice {
    let s = state_mut();
    match s.active_backend {
        AudioBackend::Hda => &s.hda,
        AudioBackend::Virtio => &s.virtio,
        AudioBackend::Dummy => &s.dummy,
    }
}

fn handle_play_sound(sound_id: u32) {
    let s = state_mut();
    let samples_count = match sound_id {
        syscall::AUDIO_SOUND_BOOT_COMPLETE => synth::synthesize_boot_chime(&mut s.pcm_buffer),
        syscall::AUDIO_SOUND_SECURITY_ALERT => synth::synthesize_security_alert(&mut s.pcm_buffer),
        _ => 0,
    };

    if samples_count > 0 {
        let dev = get_active_device();
        let _ = dev.play_pcm(&s.pcm_buffer[..samples_count], synth::SAMPLE_RATE, 2);
    }
}

fn handle_beep(freq_hz: u32, dur_ms: u32) {
    let s = state_mut();
    let dur = dur_ms.clamp(10, 3000);
    let freq = freq_hz.clamp(100, 10000);
    let samples_count = synth::synthesize_beep(freq, dur, &mut s.pcm_buffer);
    if samples_count > 0 {
        let dev = get_active_device();
        let _ = dev.play_pcm(&s.pcm_buffer[..samples_count], synth::SAMPLE_RATE, 2);
    }
}

fn handle_stop() {
    let dev = get_active_device();
    dev.stop();
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    boot_log(b"audio_server: starting (Ring 1, Vague 4)\n");

    // Enregistrement de l'endpoint public et du nom
    let name = b"audio_server";
    let reg_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            syscall::AUDIO_SERVER_ENDPOINT,
        )
    };
    if reg_rc < 0 && reg_rc != syscall::EEXIST {
        boot_log(b"audio_server: failed to register endpoint\n");
    }

    probe_audio_hardware();
    boot_log(b"audio_server: service online\n");

    let mut req = syscall::AudioRequest::default();

    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::AUDIO_SERVER_ENDPOINT,
                &mut req as *mut syscall::AudioRequest as u64,
                core::mem::size_of::<syscall::AudioRequest>() as u64,
                0, // Bloquant
            )
        };

        if rc < core::mem::size_of::<syscall::AudioRequest>() as i64 {
            continue;
        }

        match req.msg_type {
            syscall::AUDIO_MSG_PLAY_SYSTEM_SOUND => {
                handle_play_sound(req.a);
            }
            syscall::AUDIO_MSG_BEEP => {
                handle_beep(req.a, req.b);
            }
            syscall::AUDIO_MSG_STOP => {
                handle_stop();
            }
            _ => {}
        }

        if req.reply_endpoint != 0 {
            let reply = syscall::AudioReply {
                status: 0,
                _reserved: [0; 16],
            };
            unsafe {
                let _ = syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    req.reply_endpoint,
                    &reply as *const syscall::AudioReply as u64,
                    core::mem::size_of::<syscall::AudioReply>() as u64,
                    0,
                    0,
                    0,
                );
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    boot_log(b"audio_server: panic!\n");
    loop {
        core::hint::spin_loop();
    }
}
