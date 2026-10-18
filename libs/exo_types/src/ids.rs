//! ids.rs — Newtypes stricts d'identifiants fondamentaux Exo-OS (#![no_std])
//!
//! Garantit la séparation de types pour éviter la confusion entre PID, TID, BDF et EndpointId.

/// Identifiant de processus (PID)
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Pid(pub u32);

impl Pid {
    /// PID inactif / init kernel idle
    pub const IDLE: Self = Self(0);
    /// Sentinelle d'erreur / PID invalide
    pub const INVALID: Self = Self(u32::MAX);

    /// Construit un nouveau `Pid`
    #[inline(always)]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Vérifie si le PID est valide (différent de 0 et u32::MAX)
    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 != u32::MAX && self.0 != 0
    }

    /// Valeur brute u32
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Identifiant de thread (TID)
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Tid(pub u32);

impl Tid {
    /// Sentinelle TID invalide
    pub const INVALID: Self = Self(u32::MAX);

    /// Construit un nouveau `Tid`
    #[inline(always)]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Vérifie si le TID est valide
    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }

    /// Valeur brute u32
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Adresse PCI Bus/Device/Function (BDF)
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Bdf(pub u32);

impl Bdf {
    /// Construit un nouveau `Bdf` à partir des composantes bus, device et function
    #[inline(always)]
    pub const fn new(bus: u8, dev: u8, func: u8) -> Self {
        Self(((bus as u32) << 8) | (((dev & 0x1F) as u32) << 3) | ((func & 0x07) as u32))
    }

    /// Numéro de bus PCI
    #[inline(always)]
    pub const fn bus(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    /// Numéro de device PCI
    #[inline(always)]
    pub const fn device(self) -> u8 {
        ((self.0 >> 3) & 0x1F) as u8
    }

    /// Numéro de fonction PCI
    #[inline(always)]
    pub const fn function(self) -> u8 {
        (self.0 & 0x07) as u8
    }

    /// Valeur brute encodée
    #[inline(always)]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Identifiant d'endpoint de communication IPC (64 bits)
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct EndpointId(pub u64);

impl EndpointId {
    /// Sentinelle d'endpoint invalide
    pub const INVALID: Self = Self(0);

    /// Construit un nouveau `EndpointId` à partir d'une valeur u64 brute
    #[inline(always)]
    pub const fn new(val: u64) -> Self {
        Self(val)
    }

    /// Construit un `EndpointId` canonique composé d'un PID et d'un canal
    #[inline(always)]
    pub const fn from_pid_channel(pid: u32, channel: u32) -> Self {
        Self(((pid as u64) << 32) | (channel as u64))
    }

    /// PID du propriétaire de l'endpoint
    #[inline(always)]
    pub const fn pid(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Canal de service associé
    #[inline(always)]
    pub const fn channel(self) -> u32 {
        (self.0 & 0xFFFF_FFFF) as u32
    }

    /// Valeur brute u64
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}
