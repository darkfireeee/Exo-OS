//! Pilote Intel High Definition Audio (HDA) Ring 3 pour Exo-OS
//!
//! Spécification Intel HDA 1.0a / PCI Class 0x04, Subclass 0x03.
//! Gère l'initialisation du contrôleur HDA, la configuration du flux de sortie (Stream 0)
//! et la lecture DMA de buffers PCM 44.1 kHz 16-bit stéréo via BDL (Buffer Descriptor List).

use exo_os_drivers::{AudioDevice, AudioError, DeviceHandle, DeviceInfo, DeviceMatch, DriverError, ExoDriver, IrqResult, ProbeResult};
use exo_syscall_abi as syscall;

// ── Registres généraux Intel HDA ─────────────────────────────────────────────
pub const HDA_REG_GCAP: usize     = 0x00; // Global Capabilities (u16)
pub const HDA_REG_VMIN: usize     = 0x02; // Minor Version (u8)
pub const HDA_REG_VMAJ: usize     = 0x03; // Major Version (u8)
pub const HDA_REG_GCTL: usize     = 0x08; // Global Control (u32)
pub const HDA_REG_WAKEEN: usize   = 0x0C; // Wake Enable (u16)
pub const HDA_REG_STATESTS: usize = 0x0E; // State Change Status (u16)
pub const HDA_REG_GSTS: usize     = 0x10; // Global Status (u16)

// ── Contrôle Global (GCTL) ───────────────────────────────────────────────────
pub const HDA_GCTL_CRST: u32 = 1 << 0; // Controller Reset (1 = opérationnel, 0 = reset)

// ── Registres d'un Stream Descriptor (SDn) ───────────────────────────────────
pub const HDA_SD_CTL: usize   = 0x00; // Control (u32/u8)
pub const HDA_SD_STS: usize   = 0x03; // Status (u8)
pub const HDA_SD_LPIB: usize  = 0x04; // Link Position in Buffer (u32)
pub const HDA_SD_CBL: usize   = 0x08; // Cyclic Buffer Length (u32)
pub const HDA_SD_LVI: usize   = 0x0C; // Last Valid Index (u16)
pub const HDA_SD_FIFOD: usize = 0x0E; // FIFO Size (u16)
pub const HDA_SD_FMT: usize   = 0x12; // Format (u16)
pub const HDA_SD_BDPL: usize  = 0x18; // BDL Pointer Lower (u32)
pub const HDA_SD_BDPU: usize  = 0x1C; // BDL Pointer Upper (u32)

// ── Immediate Command (ICW / IRR / ICS) ──────────────────────────────────────
pub const HDA_REG_ICW: usize     = 0x60; // Immediate Command Write (u32)
pub const HDA_REG_IRR: usize     = 0x64; // Immediate Response Read (u32)
pub const HDA_REG_ICS: usize     = 0x68; // Immediate Command Status (u16)
pub const HDA_ICS_ICB: u16       = 1 << 0; // Immediate Command Busy
pub const HDA_ICS_IRV: u16       = 1 << 1; // Immediate Result Valid

// SD_CTL bits
pub const HDA_SD_CTL_SRST: u32  = 1 << 0; // Stream Reset
pub const HDA_SD_CTL_RUN: u32   = 1 << 1; // Stream Run
pub const HDA_SD_CTL_IOCE: u32  = 1 << 2; // Interrupt on Completion Enable
pub const HDA_SD_CTL_FEIE: u32  = 1 << 3; // FIFO Error Interrupt Enable
pub const HDA_SD_CTL_DEIE: u32  = 1 << 4; // Descriptor Error Interrupt Enable

// Format PCM 44.1 kHz, 16-bit, 2 canaux (Stéréo) :
// Bit 14 : Base rate (1 = 44.1 kHz, 0 = 48.0 kHz) -> 1
// Bits 13..11 : Mult (000 = x1)
// Bits 10..8 : Div (000 = /1)
// Bits 6..4 : Bits per sample (001 = 16 bits)
// Bits 3..0 : Channels (0001 = 2 channels)
// 0x4000 | (1 << 4) | 1 = 0x4011
pub const HDA_FMT_44100_16BIT_STEREO: u16 = 0x4011;

/// Entrée de la liste de descripteurs de buffer (BDL - Buffer Descriptor List)
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct HdaBdlEntry {
    pub addr_lo: u32,
    pub addr_hi: u32,
    pub length: u32,
    pub ioc: u32, // Bit 0 = Interrupt on Completion
}

pub struct HdaDriver {
    pub handle: Option<DeviceHandle>,
    pub num_iss: u8,
    pub num_oss: u8,
    pub out_sd_offset: usize,
    pub dma_buffer_phys: u64,
    pub dma_buffer_virt: *mut u8,
    pub dma_buffer_size: usize,
    pub bdl_phys: u64,
    pub bdl_virt: *mut HdaBdlEntry,
}

unsafe impl Send for HdaDriver {}
unsafe impl Sync for HdaDriver {}

impl HdaDriver {
    pub const fn new() -> Self {
        Self {
            handle: None,
            num_iss: 0,
            num_oss: 0,
            out_sd_offset: 0x80,
            dma_buffer_phys: 0,
            dma_buffer_virt: core::ptr::null_mut(),
            dma_buffer_size: 0,
            bdl_phys: 0,
            bdl_virt: core::ptr::null_mut(),
        }
    }

    /// Réinitialise le contrôleur Intel HDA via GCTL.CRST
    pub fn reset_controller(&mut self) -> Result<(), DriverError> {
        let Some(ref h) = self.handle else {
            return Err(DriverError::DeviceNotFound);
        };

        unsafe {
            // Mettre en reset
            let gctl = h.mmio.read32(HDA_REG_GCTL);
            h.mmio.write32(HDA_REG_GCTL, gctl & !HDA_GCTL_CRST);

            let mut timeout = 10_000usize;
            while (h.mmio.read32(HDA_REG_GCTL) & HDA_GCTL_CRST) != 0 && timeout > 0 {
                core::hint::spin_loop();
                timeout -= 1;
            }

            // Sortir du reset
            h.mmio.write32(HDA_REG_GCTL, HDA_GCTL_CRST);
            timeout = 100_000usize;
            while (h.mmio.read32(HDA_REG_GCTL) & HDA_GCTL_CRST) == 0 && timeout > 0 {
                core::hint::spin_loop();
                timeout -= 1;
            }
            if timeout == 0 {
                return Err(DriverError::HardwareTimeout);
            }

            // Attente stabilisation du bus codec
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        }

        Ok(())
    }

    /// Alloue les pages DMA nécessaires pour le buffer PCM et la table BDL
    pub fn setup_dma(&mut self) -> Result<(), DriverError> {
        // Allouer 32 pages (128 Kio) pour le buffer PCM de sortie
        const PCM_SIZE: usize = 32 * 4096;
        let mut pcm_virt = 0u64;
        let pcm_dma = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                PCM_SIZE as u64,
                0, // DMA_BIDIRECTIONAL
                &mut pcm_virt as *mut u64 as u64,
                0,
                0,
            )
        };
        if pcm_dma < 0 {
            return Err(DriverError::DmaAllocFailed(pcm_dma));
        }

        // Allouer 1 page (4 Kio) pour la table BDL (jusqu'à 256 descripteurs)
        const BDL_SIZE: usize = 4096;
        let mut bdl_virt = 0u64;
        let bdl_dma = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                BDL_SIZE as u64,
                0, // DMA_BIDIRECTIONAL
                &mut bdl_virt as *mut u64 as u64,
                0,
                0,
            )
        };
        if bdl_dma < 0 {
            return Err(DriverError::DmaAllocFailed(bdl_dma));
        }

        self.dma_buffer_phys = pcm_dma as u64;
        self.dma_buffer_virt = pcm_virt as *mut u8;
        self.dma_buffer_size = PCM_SIZE;

        self.bdl_phys = bdl_dma as u64;
        self.bdl_virt = bdl_virt as *mut HdaBdlEntry;

        Ok(())
    }

    /// Envoie un verbe 32-bit au codec via Immediate Command (ICW/IRR)
    pub fn send_verb(&self, verb: u32) -> Result<u32, DriverError> {
        let Some(ref h) = self.handle else {
            return Err(DriverError::DeviceNotFound);
        };

        unsafe {
            let mut timeout = 20_000usize;
            while (h.mmio.read16(HDA_REG_ICS) & HDA_ICS_ICB) != 0 && timeout > 0 {
                core::hint::spin_loop();
                timeout -= 1;
            }
            if timeout == 0 {
                return Err(DriverError::HardwareTimeout);
            }

            // Effacer le bit IRV précédent (W1C)
            h.mmio.write16(HDA_REG_ICS, HDA_ICS_IRV);

            // Écrire le verbe dans ICW
            h.mmio.write32(HDA_REG_ICW, verb);

            // Déclencher l'exécution par le matériel (ICB = 1)
            h.mmio.write16(HDA_REG_ICS, HDA_ICS_ICB);

            // Attendre la fin d'exécution (ICB = 0)
            timeout = 50_000usize;
            while (h.mmio.read16(HDA_REG_ICS) & HDA_ICS_ICB) != 0 && timeout > 0 {
                core::hint::spin_loop();
                timeout -= 1;
            }

            Ok(h.mmio.read32(HDA_REG_IRR))
        }
    }

    /// Configure les nœuds du codec HDA (DAC + Pin Out) pour router le flux PCM
    pub fn init_codecs(&mut self) -> Result<(), DriverError> {
        // QEMU hda-duplex standard :
        // Node 0x02 : Audio Output Converter (DAC)
        // Node 0x03 : Pin Complex (Line Out)
        // Node 0x04 : Pin Complex / Speaker (hda-micro ou autre)

        // 1. Allumer DAC 0x02, Pin 0x03 (Line Out) et Pin 0x04 (Power State D0 = 0)
        let _ = self.send_verb(0x0027_0500);
        let _ = self.send_verb(0x0037_0500);
        let _ = self.send_verb(0x0047_0500);

        // 2. Activer la sortie analogique (Pin Widget Control = Out Enable 0x40) sur Pin 0x03 et 0x04
        let _ = self.send_verb(0x0037_0740);
        let _ = self.send_verb(0x0047_0740);

        // 3. Dé-muter l'ampli de sortie avec gain nominal (0 dB) sur DAC 0x02, Pin 0x03 et Pin 0x04
        // Verbe 0x3000 (Set Amp Gain): 0xB000 (Output, Left+Right) | 0x7F (0 dB, unmuted)
        let _ = self.send_verb(0x0023_B07F);
        let _ = self.send_verb(0x0033_B07F);
        let _ = self.send_verb(0x0043_B07F);

        // 4. Assigner le stream tag 1 (canal 0) au DAC 0x02 (Verbe 0x706, param 0x10)
        let _ = self.send_verb(0x0027_0610);

        // 5. Configurer le format du DAC 0x02 pour correspondre au flux (44.1 kHz, 16b, stéréo)
        let _ = self.send_verb(0x0022_0000 | (HDA_FMT_44100_16BIT_STEREO as u32));

        Ok(())
    }
}

impl ExoDriver for HdaDriver {
    fn supported_ids() -> &'static [DeviceMatch] {
        static IDS: [DeviceMatch; 2] = [
            // Exact Intel HDA ICH6 (QEMU default)
            DeviceMatch::Exact { vendor: 0x8086, device: 0x2668 },
            // Generic PCI Audio Controller
            DeviceMatch::ClassSub { class: 0x04, sub: 0x03 },
        ];
        &IDS
    }

    fn probe(&self, info: &DeviceInfo) -> ProbeResult {
        if info.class == 0x04 && info.subclass == 0x03 {
            ProbeResult::Yes
        } else {
            ProbeResult::No
        }
    }

    fn attach(&mut self, handle: DeviceHandle) -> Result<(), DriverError> {
        self.handle = Some(handle);

        let h = self.handle.as_ref().unwrap();
        let gcap = unsafe { h.mmio.read16(HDA_REG_GCAP) };
        self.num_iss = ((gcap >> 8) & 0x0F) as u8;
        self.num_oss = ((gcap >> 12) & 0x0F) as u8;

        // Le premier stream descriptor de sortie (OS0) se trouve après tous les input streams
        self.out_sd_offset = 0x80 + (self.num_iss as usize) * 0x20;

        self.reset_controller()?;
        self.setup_dma()?;
        self.init_codecs()?;

        Ok(())
    }

    fn handle_irq(&mut self, _irq: u8) -> IrqResult {
        IrqResult::Handled
    }

    fn detach(&mut self) {
        self.stop();
        self.handle = None;
    }
}

impl AudioDevice for HdaDriver {
    fn play_pcm(&self, data: &[i16], _rate: u32, _channels: u8) -> Result<(), AudioError> {
        let Some(ref h) = self.handle else {
            return Err(AudioError::HwError);
        };

        if self.dma_buffer_virt.is_null() || self.bdl_virt.is_null() {
            return Err(AudioError::HwError);
        }

        let byte_len = data.len() * 2;
        let copy_len = byte_len.min(self.dma_buffer_size);

        unsafe {
            // 1. Stopper le stream s'il tournait
            let sd_base = self.out_sd_offset;
            h.mmio.write32(sd_base + HDA_SD_CTL, 0);

            // 2. Copier les données PCM dans le buffer DMA
            core::ptr::copy_nonoverlapping(
                data.as_ptr() as *const u8,
                self.dma_buffer_virt,
                copy_len,
            );

            // 3. Préparer les entrées BDL (Au moins 2 entrées obligatoires par la spec Intel HDA et QEMU)
            // L'échantillon stéréo 16-bit fait 4 octets : chaque moitié doit être alignée sur 4 octets.
            let half = (copy_len / 2) & !3;
            let bdl_entry0 = HdaBdlEntry {
                addr_lo: self.dma_buffer_phys as u32,
                addr_hi: (self.dma_buffer_phys >> 32) as u32,
                length: half as u32,
                ioc: 0,
            };
            let bdl_entry1 = HdaBdlEntry {
                addr_lo: (self.dma_buffer_phys + half as u64) as u32,
                addr_hi: ((self.dma_buffer_phys + half as u64) >> 32) as u32,
                length: (copy_len - half) as u32,
                ioc: 1, // Interruption de complétion
            };
            core::ptr::write(self.bdl_virt, bdl_entry0);
            core::ptr::write(self.bdl_virt.add(1), bdl_entry1);

            // 4. Configurer le Stream Descriptor
            h.mmio.write32(sd_base + HDA_SD_BDPL, self.bdl_phys as u32);
            h.mmio.write32(sd_base + HDA_SD_BDPU, (self.bdl_phys >> 32) as u32);
            h.mmio.write32(sd_base + HDA_SD_CBL, copy_len as u32);
            h.mmio.write16(sd_base + HDA_SD_LVI, 1); // 2 entrées valides (indices 0 et 1)

            // Format : 44.1 kHz, 16-bit, Stéréo (offset 0x12 est u16)
            h.mmio.write16(sd_base + HDA_SD_FMT, HDA_FMT_44100_16BIT_STEREO);

            // Stream tag = 1, lancer le flux (RUN)
            let ctl = (1 << 20) | HDA_SD_CTL_RUN | HDA_SD_CTL_IOCE;
            h.mmio.write32(sd_base + HDA_SD_CTL, ctl);
        }

        Ok(())
    }

    fn stop(&self) {
        if let Some(ref h) = self.handle {
            unsafe {
                let sd_base = self.out_sd_offset;
                h.mmio.write32(sd_base + HDA_SD_CTL, 0);
            }
        }
    }
}
