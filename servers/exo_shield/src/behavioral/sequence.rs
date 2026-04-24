//! # Sequence Analysis — Détection de séquences comportementales
//!
//! Analyse de séquences ordonnées de comportements via machines à états.
//! Détecte les patterns d'attaque qui ne sont visibles que dans l'ordre
//! chronologique des événements (ex : open → write → exec = shellcode).
//!
//! ## Fonctionnalités
//! - Détection de séquences ordonnées (max 16 séquences simultanées)
//! - Machine à états par séquence (max 8 états par séquence)
//! - Timeout TSC par étape
//! - Génération d'alertes quand une séquence complète est détectée
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de séquences détectées simultanément.
pub const MAX_SEQUENCES: usize = 16;

/// Nombre maximum d'états par séquence.
pub const MAX_SEQUENCE_STEPS: usize = 8;

/// Taille max du nom de séquence.
pub const SEQUENCE_NAME_SIZE: usize = 24;

/// Nombre maximum d'alertes de séquence.
pub const MAX_SEQUENCE_ALERTS: usize = 16;

/// Timeout par défaut pour une étape (en cycles TSC, ~1 seconde à 3 GHz).
pub const DEFAULT_STEP_TIMEOUT_TSC: u64 = 3_000_000_000;

/// Timeout global pour une séquence complète.
pub const DEFAULT_SEQUENCE_TIMEOUT_TSC: u64 = 30_000_000_000;

// ── Type d'événement ─────────────────────────────────────────────────────────

/// Type d'événement comportemental.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum EventType {
    /// Appel système.
    Syscall = 0,
    /// Accès mémoire.
    MemAccess = 1,
    /// Activité réseau.
    NetConnect = 2,
    /// Appel IPC.
    IpcCall = 3,
    /// Accès fichier.
    FileAccess = 4,
    /// Changement de privilège.
    PrivChange = 5,
    /// Création de processus.
    ProcessCreate = 6,
    /// Signal.
    Signal = 7,
    /// Personnalisé.
    Custom = 8,
}

impl EventType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(EventType::Syscall),
            1 => Some(EventType::MemAccess),
            2 => Some(EventType::NetConnect),
            3 => Some(EventType::IpcCall),
            4 => Some(EventType::FileAccess),
            5 => Some(EventType::PrivChange),
            6 => Some(EventType::ProcessCreate),
            7 => Some(EventType::Signal),
            8 => Some(EventType::Custom),
            _ => None,
        }
    }
}

// ── Événement ────────────────────────────────────────────────────────────────

/// Un événement comportemental observé.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BehaviorEvent {
    /// Type d'événement.
    pub event_type: EventType,
    /// PID du processus source.
    pub pid: u32,
    /// Code de l'événement (numéro de syscall, type d'accès, etc.).
    pub event_code: u64,
    /// Paramètre additionnel (adresse, port, etc.).
    pub param: u64,
    /// Horodatage TSC.
    pub timestamp_tsc: u64,
}

impl BehaviorEvent {
    pub const fn empty() -> Self {
        Self {
            event_type: EventType::Custom,
            pid: 0,
            event_code: 0,
            param: 0,
            timestamp_tsc: 0,
        }
    }

    pub fn new(event_type: EventType, pid: u32, event_code: u64, param: u64) -> Self {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!(
                "rdtsc",
                out("eax") lo,
                out("edx") hi,
                options(nostack, nomem),
            );
        }
        Self {
            event_type,
            pid,
            event_code,
            param,
            timestamp_tsc: ((hi as u64) << 32) | lo as u64,
        }
    }
}

// ── Condition de transition ──────────────────────────────────────────────────

/// Condition de transition entre deux états d'une séquence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum TransitionCondition {
    /// Événement exact (type + code).
    ExactMatch = 0,
    /// Type d'événement uniquement.
    TypeMatch = 1,
    /// Plage de codes (min..=max).
    CodeRange = 2,
    /// Paramètre dans une plage.
    ParamRange = 3,
    /// N'importe quel événement (toujours vrai).
    Any = 4,
}

impl TransitionCondition {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(TransitionCondition::ExactMatch),
            1 => Some(TransitionCondition::TypeMatch),
            2 => Some(TransitionCondition::CodeRange),
            3 => Some(TransitionCondition::ParamRange),
            4 => Some(TransitionCondition::Any),
            _ => None,
        }
    }
}

// ── Étape de séquence ────────────────────────────────────────────────────────

/// Une étape (transition) dans une séquence comportementale.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SequenceStep {
    /// Type d'événement attendu.
    pub expected_type: EventType,
    /// Condition de transition.
    pub condition: TransitionCondition,
    /// Code d'événement attendu (pour ExactMatch).
    pub expected_code: u64,
    /// Borne inférieure du code (pour CodeRange).
    pub code_min: u64,
    /// Borne supérieure du code (pour CodeRange).
    pub code_max: u64,
    /// Paramètre minimum (pour ParamRange).
    pub param_min: u64,
    /// Paramètre maximum (pour ParamRange).
    pub param_max: u64,
    /// Timeout pour cette étape (en cycles TSC).
    pub timeout_tsc: u64,
    /// La transition doit-elle venir du même PID ?
    pub same_pid: bool,
    /// Réservé.
    _reserved: [u8; 7],
}

impl SequenceStep {
    pub const fn empty() -> Self {
        Self {
            expected_type: EventType::Custom,
            condition: TransitionCondition::Any,
            expected_code: 0,
            code_min: 0,
            code_max: 0,
            param_min: 0,
            param_max: 0,
            timeout_tsc: DEFAULT_STEP_TIMEOUT_TSC,
            same_pid: true,
            _reserved: [0; 7],
        }
    }

    /// Crée une étape avec correspondance exacte.
    pub fn exact(event_type: EventType, code: u64) -> Self {
        Self {
            expected_type: event_type,
            condition: TransitionCondition::ExactMatch,
            expected_code: code,
            timeout_tsc: DEFAULT_STEP_TIMEOUT_TSC,
            same_pid: true,
            ..SequenceStep::empty()
        }
    }

    /// Crée une étape avec correspondance de type uniquement.
    pub fn type_only(event_type: EventType) -> Self {
        Self {
            expected_type: event_type,
            condition: TransitionCondition::TypeMatch,
            timeout_tsc: DEFAULT_STEP_TIMEOUT_TSC,
            same_pid: true,
            ..SequenceStep::empty()
        }
    }

    /// Crée une étape avec plage de codes.
    pub fn code_range(event_type: EventType, min: u64, max: u64) -> Self {
        Self {
            expected_type: event_type,
            condition: TransitionCondition::CodeRange,
            code_min: min,
            code_max: max,
            timeout_tsc: DEFAULT_STEP_TIMEOUT_TSC,
            same_pid: true,
            ..SequenceStep::empty()
        }
    }

    /// Crée une étape "n'importe quel événement".
    pub fn any_event() -> Self {
        Self {
            condition: TransitionCondition::Any,
            timeout_tsc: DEFAULT_STEP_TIMEOUT_TSC,
            same_pid: false,
            ..SequenceStep::empty()
        }
    }

    /// Définit le timeout de cette étape.
    pub fn with_timeout(mut self, timeout_tsc: u64) -> Self {
        self.timeout_tsc = timeout_tsc;
        self
    }

    /// Autorise les événements d'autres PIDs.
    pub fn allow_cross_pid(mut self) -> Self {
        self.same_pid = false;
        self
    }

    /// Vérifie si un événement satisfait la condition de cette étape.
    pub fn matches(&self, event: &BehaviorEvent, last_pid: u32) -> bool {
        // Vérifier le PID si nécessaire
        if self.same_pid && event.pid != last_pid && last_pid != 0 {
            return false;
        }

        match self.condition {
            TransitionCondition::ExactMatch => {
                event.event_type == self.expected_type && event.event_code == self.expected_code
            }
            TransitionCondition::TypeMatch => event.event_type == self.expected_type,
            TransitionCondition::CodeRange => {
                event.event_type == self.expected_type
                    && event.event_code >= self.code_min
                    && event.event_code <= self.code_max
            }
            TransitionCondition::ParamRange => {
                event.event_type == self.expected_type
                    && event.param >= self.param_min
                    && event.param <= self.param_max
            }
            TransitionCondition::Any => true,
        }
    }
}

// ── Définition de séquence ───────────────────────────────────────────────────

/// Définition d'une séquence comportementale à détecter.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SequenceDefinition {
    /// Nom de la séquence.
    pub name: [u8; SEQUENCE_NAME_SIZE],
    /// Étapes de la séquence.
    pub steps: [SequenceStep; MAX_SEQUENCE_STEPS],
    /// Nombre d'étapes actives.
    pub step_count: u8,
    /// Sévérité de la séquence (0..=3).
    pub severity: u8,
    /// Timeout global de la séquence (en cycles TSC).
    pub global_timeout_tsc: u64,
    /// La séquence est-elle active ?
    pub enabled: bool,
    /// Réservé.
    _reserved: [u8; 2],
}

impl SequenceDefinition {
    pub const fn empty() -> Self {
        Self {
            name: [0u8; SEQUENCE_NAME_SIZE],
            steps: [SequenceStep::empty(); MAX_SEQUENCE_STEPS],
            step_count: 0,
            severity: 0,
            global_timeout_tsc: DEFAULT_SEQUENCE_TIMEOUT_TSC,
            enabled: false,
            _reserved: [0; 2],
        }
    }

    /// Crée une nouvelle définition de séquence.
    pub fn new(name: &[u8], severity: u8) -> Self {
        let mut name_buf = [0u8; SEQUENCE_NAME_SIZE];
        let len = name.len().min(SEQUENCE_NAME_SIZE - 1);
        name_buf[..len].copy_from_slice(&name[..len]);
        Self {
            name: name_buf,
            steps: [SequenceStep::empty(); MAX_SEQUENCE_STEPS],
            step_count: 0,
            severity: severity.min(3),
            global_timeout_tsc: DEFAULT_SEQUENCE_TIMEOUT_TSC,
            enabled: true,
            _reserved: [0; 2],
        }
    }

    /// Ajoute une étape à la séquence.
    pub fn add_step(&mut self, step: SequenceStep) -> bool {
        if self.step_count as usize >= MAX_SEQUENCE_STEPS {
            return false;
        }
        self.steps[self.step_count as usize] = step;
        self.step_count += 1;
        true
    }

    /// Retourne le nom comme slice.
    pub fn name_slice(&self) -> &[u8] {
        let mut len = 0usize;
        while len < SEQUENCE_NAME_SIZE && self.name[len] != 0 {
            len += 1;
        }
        &self.name[..len]
    }
}

// ── État d'une séquence en cours ─────────────────────────────────────────────

/// État d'une séquence en cours de détection.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SequenceState {
    /// Index de la définition de séquence.
    pub def_index: usize,
    /// PID associé (premier événement).
    pub pid: u32,
    /// Étape actuelle (0 = en attente du premier événement).
    pub current_step: u8,
    /// Nombre total d'étapes.
    pub total_steps: u8,
    /// Horodatage TSC du début de la séquence.
    pub start_tsc: u64,
    /// Horodatage TSC du dernier événement correspondant.
    pub last_event_tsc: u64,
    /// PID du dernier événement.
    pub last_pid: u32,
    /// L'état est-il actif ?
    pub active: bool,
    /// Nombre de fois que cette séquence a été complétée.
    pub completion_count: u32,
    /// Réservé.
    _reserved: [u8; 3],
}

impl SequenceState {
    pub const fn empty() -> Self {
        Self {
            def_index: 0,
            pid: 0,
            current_step: 0,
            total_steps: 0,
            start_tsc: 0,
            last_event_tsc: 0,
            last_pid: 0,
            active: false,
            completion_count: 0,
            _reserved: [0; 3],
        }
    }

    /// Vérifie si la séquence est complète.
    pub fn is_complete(&self) -> bool {
        self.active && self.current_step >= self.total_steps
    }

    /// Vérifie si la séquence a expiré.
    pub fn is_timed_out(&self, now_tsc: u64, global_timeout: u64) -> bool {
        if !self.active {
            return false;
        }
        now_tsc.wrapping_sub(self.start_tsc) > global_timeout
    }
}

// ── Alerte de séquence ───────────────────────────────────────────────────────

/// Alerte générée quand une séquence est détectée.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SequenceAlert {
    /// Index de la définition de séquence.
    pub def_index: usize,
    /// PID du processus.
    pub pid: u32,
    /// Sévérité.
    pub severity: u8,
    /// Durée totale de la séquence (en cycles TSC).
    pub duration_tsc: u64,
    /// Horodatage de la détection.
    pub timestamp_tsc: u64,
    /// Nombre de fois que cette séquence a été vue.
    pub occurrence: u32,
    /// L'alerte est-elle valide ?
    pub valid: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl SequenceAlert {
    pub const fn empty() -> Self {
        Self {
            def_index: 0,
            pid: 0,
            severity: 0,
            duration_tsc: 0,
            timestamp_tsc: 0,
            occurrence: 0,
            valid: false,
            _reserved: [0; 3],
        }
    }
}

// ── Détecteur de séquences ───────────────────────────────────────────────────

static SEQUENCE_DETECTOR: Mutex<SequenceDetectorInner> = Mutex::new(SequenceDetectorInner::new());

static TOTAL_EVENTS_PROCESSED: AtomicU64 = AtomicU64::new(0);
static TOTAL_SEQUENCES_COMPLETED: AtomicU64 = AtomicU64::new(0);
static TOTAL_SEQUENCES_TIMEOUT: AtomicU64 = AtomicU64::new(0);

struct SequenceDetectorInner {
    definitions: [SequenceDefinition; MAX_SEQUENCES],
    def_count: usize,
    active_states: [SequenceState; MAX_SEQUENCES],
    state_count: usize,
    alerts: [SequenceAlert; MAX_SEQUENCE_ALERTS],
    alert_head: usize,
    alert_count: usize,
}

impl SequenceDetectorInner {
    const fn new() -> Self {
        Self {
            definitions: [SequenceDefinition::empty(); MAX_SEQUENCES],
            def_count: 0,
            active_states: [SequenceState::empty(); MAX_SEQUENCES],
            state_count: 0,
            alerts: [SequenceAlert::empty(); MAX_SEQUENCE_ALERTS],
            alert_head: 0,
            alert_count: 0,
        }
    }
}

// ── Lecture TSC ──────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Ajoute une définition de séquence.
///
/// # Retour
/// - L'index de la définition si succès, ou MAX_SEQUENCES si plein.
pub fn add_sequence(def: &SequenceDefinition) -> usize {
    let mut det = SEQUENCE_DETECTOR.lock();

    if det.def_count >= MAX_SEQUENCES {
        return MAX_SEQUENCES;
    }

    let idx = det.def_count;
    det.definitions[idx] = *def;
    det.def_count += 1;
    idx
}

/// Supprime une définition de séquence.
pub fn remove_sequence(index: usize) -> bool {
    let mut det = SEQUENCE_DETECTOR.lock();
    if index >= det.def_count {
        return false;
    }
    det.definitions[index].enabled = false;

    // Supprimer aussi les états actifs associés
    for i in 0..det.state_count {
        if det.active_states[i].active && det.active_states[i].def_index == index {
            det.active_states[i].active = false;
        }
    }
    true
}

/// Active une définition de séquence.
pub fn enable_sequence(index: usize) -> bool {
    let mut det = SEQUENCE_DETECTOR.lock();
    if index >= det.def_count {
        return false;
    }
    det.definitions[index].enabled = true;
    true
}

/// Désactive une définition de séquence.
pub fn disable_sequence(index: usize) -> bool {
    let mut det = SEQUENCE_DETECTOR.lock();
    if index >= det.def_count {
        return false;
    }
    det.definitions[index].enabled = false;

    // Supprimer les états actifs associés
    for i in 0..det.state_count {
        if det.active_states[i].active && det.active_states[i].def_index == index {
            det.active_states[i].active = false;
        }
    }
    true
}

/// Trouve ou crée un slot d'état pour une séquence.
fn find_or_create_state(
    det: &mut SequenceDetectorInner,
    def_index: usize,
    pid: u32,
    now_tsc: u64,
) -> Option<usize> {
    // Chercher un état existant pour cette définition et ce PID
    for i in 0..det.state_count {
        if det.active_states[i].active
            && det.active_states[i].def_index == def_index
            && det.active_states[i].pid == pid
        {
            return Some(i);
        }
    }

    // Chercher un slot libre
    for i in 0..det.state_count {
        if !det.active_states[i].active {
            det.active_states[i] = SequenceState {
                def_index,
                pid,
                current_step: 0,
                total_steps: det.definitions[def_index].step_count,
                start_tsc: now_tsc,
                last_event_tsc: now_tsc,
                last_pid: pid,
                active: true,
                completion_count: 0,
                _reserved: [0; 3],
            };
            return Some(i);
        }
    }

    // Créer un nouveau slot
    if det.state_count < MAX_SEQUENCES {
        let idx = det.state_count;
        det.active_states[idx] = SequenceState {
            def_index,
            pid,
            current_step: 0,
            total_steps: det.definitions[def_index].step_count,
            start_tsc: now_tsc,
            last_event_tsc: now_tsc,
            last_pid: pid,
            active: true,
            completion_count: 0,
            _reserved: [0; 3],
        };
        det.state_count += 1;
        return Some(idx);
    }

    // Réutiliser le slot le plus ancien
    let mut oldest_tsc = u64::MAX;
    let mut oldest_idx = 0;
    for i in 0..det.state_count {
        if det.active_states[i].last_event_tsc < oldest_tsc {
            oldest_tsc = det.active_states[i].last_event_tsc;
            oldest_idx = i;
        }
    }
    det.active_states[oldest_idx] = SequenceState {
        def_index,
        pid,
        current_step: 0,
        total_steps: det.definitions[def_index].step_count,
        start_tsc: now_tsc,
        last_event_tsc: now_tsc,
        last_pid: pid,
        active: true,
        completion_count: 0,
        _reserved: [0; 3],
    };
    Some(oldest_idx)
}

/// Soumet un événement au détecteur de séquences.
///
/// L'événement est comparé contre toutes les séquences actives.
/// Si une séquence est complétée, une alerte est générée.
///
/// # Retour
/// - Le nombre de séquences qui ont progressé.
pub fn submit_event(event: &BehaviorEvent) -> usize {
    let mut det = SEQUENCE_DETECTOR.lock();
    let now_tsc = event.timestamp_tsc;
    let mut progressed = 0usize;

    TOTAL_EVENTS_PROCESSED.fetch_add(1, Ordering::Relaxed);

    // 1. Vérifier les timeouts des séquences actives
    for i in 0..det.state_count {
        let mut state = det.active_states[i];
        if !state.active {
            continue;
        }
        let def = det.definitions[state.def_index];
        if state.is_timed_out(now_tsc, def.global_timeout_tsc) {
            state.active = false;
            det.active_states[i] = state;
            TOTAL_SEQUENCES_TIMEOUT.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        // Vérifier le timeout de l'étape actuelle
        if state.current_step > 0 && state.current_step < state.total_steps {
            let step = &def.steps[state.current_step as usize];
            if now_tsc.wrapping_sub(state.last_event_tsc) > step.timeout_tsc {
                state.active = false;
                det.active_states[i] = state;
                TOTAL_SEQUENCES_TIMEOUT.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        }

        det.active_states[i] = state;
    }

    // 2. Essayer de faire progresser les séquences actives
    for i in 0..det.state_count {
        let mut state = det.active_states[i];
        if !state.active {
            continue;
        }

        let def = det.definitions[state.def_index];
        if !def.enabled {
            continue;
        }

        if state.current_step >= state.total_steps {
            continue; // Séquence déjà complète
        }

        let step = &def.steps[state.current_step as usize];
        if step.matches(event, state.last_pid) {
            state.current_step += 1;
            state.last_event_tsc = now_tsc;
            state.last_pid = event.pid;
            progressed += 1;

            // Vérifier si la séquence est complète
            if state.current_step >= state.total_steps {
                let duration = now_tsc.wrapping_sub(state.start_tsc);
                let def_index = state.def_index;
                let pid = state.pid;
                let severity = def.severity;
                let completion_count = state.completion_count + 1;

                // Générer une alerte
                let alert = SequenceAlert {
                    def_index,
                    pid,
                    severity,
                    duration_tsc: duration,
                    timestamp_tsc: now_tsc,
                    occurrence: completion_count,
                    valid: true,
                    _reserved: [0; 3],
                };

                let alert_idx = det.alert_head;
                det.alerts[alert_idx] = alert;
                det.alert_head = (alert_idx + 1) % MAX_SEQUENCE_ALERTS;
                if det.alert_count < MAX_SEQUENCE_ALERTS {
                    det.alert_count += 1;
                }

                // Réinitialiser l'état pour les occurrences futures
                state.completion_count = completion_count;
                state.current_step = 0;
                state.start_tsc = now_tsc;

                TOTAL_SEQUENCES_COMPLETED.fetch_add(1, Ordering::Relaxed);
            }

            det.active_states[i] = state;
        }
    }

    // 3. Essayer de démarrer de nouvelles séquences avec cet événement
    for def_idx in 0..det.def_count {
        let def = det.definitions[def_idx];
        if !def.enabled || def.step_count == 0 {
            continue;
        }

        // Vérifier si on a déjà un état actif pour cette définition et ce PID
        let mut already_active = false;
        for i in 0..det.state_count {
            if det.active_states[i].active
                && det.active_states[i].def_index == def_idx
                && det.active_states[i].pid == event.pid
            {
                already_active = true;
                break;
            }
        }
        if already_active {
            continue;
        }

        // Vérifier si le premier step correspond
        let first_step = &def.steps[0];
        if first_step.matches(event, event.pid) {
            if let Some(state_idx) = find_or_create_state(&mut det, def_idx, event.pid, now_tsc) {
                let mut state = det.active_states[state_idx];
                state.current_step = 1;
                state.last_event_tsc = now_tsc;
                state.last_pid = event.pid;
                progressed += 1;

                // Vérifier si la séquence est déjà complète (1 seul step)
                if state.current_step >= state.total_steps {
                    let duration = now_tsc.wrapping_sub(state.start_tsc);
                    let alert = SequenceAlert {
                        def_index: def_idx,
                        pid: event.pid,
                        severity: def.severity,
                        duration_tsc: duration,
                        timestamp_tsc: now_tsc,
                        occurrence: 1,
                        valid: true,
                        _reserved: [0; 3],
                    };

                    let alert_idx = det.alert_head;
                    det.alerts[alert_idx] = alert;
                    det.alert_head = (alert_idx + 1) % MAX_SEQUENCE_ALERTS;
                    if det.alert_count < MAX_SEQUENCE_ALERTS {
                        det.alert_count += 1;
                    }

                    state.completion_count = 1;
                    state.current_step = 0;
                    state.start_tsc = now_tsc;

                    TOTAL_SEQUENCES_COMPLETED.fetch_add(1, Ordering::Relaxed);
                }

                det.active_states[state_idx] = state;
            }
        }
    }

    progressed
}

/// Récupère les alertes de séquence récentes.
///
/// # Retour
/// Le nombre d'alertes copiées.
pub fn get_alerts(buffer: &mut [SequenceAlert], max_count: usize) -> usize {
    let det = SEQUENCE_DETECTOR.lock();
    let limit = max_count.min(buffer.len()).min(det.alert_count);
    let mut copied = 0usize;

    for i in 0..limit {
        let idx = if det.alert_head >= i + 1 {
            det.alert_head - i - 1
        } else {
            MAX_SEQUENCE_ALERTS - (i + 1 - det.alert_head)
        };
        if det.alerts[idx].valid {
            buffer[copied] = det.alerts[idx];
            copied += 1;
            if copied >= buffer.len() || copied >= max_count {
                break;
            }
        }
    }

    copied
}

/// Retourne les états actifs des séquences.
///
/// # Retour
/// Le nombre d'états copiés.
pub fn get_active_states(buffer: &mut [SequenceState], max_count: usize) -> usize {
    let det = SEQUENCE_DETECTOR.lock();
    let limit = max_count.min(buffer.len());
    let mut copied = 0usize;

    for i in 0..det.state_count {
        if det.active_states[i].active && copied < limit {
            buffer[copied] = det.active_states[i];
            copied += 1;
        }
    }

    copied
}

/// Réinitialise toutes les séquences actives.
pub fn reset_active_sequences() {
    let mut det = SEQUENCE_DETECTOR.lock();
    for i in 0..det.state_count {
        det.active_states[i].active = false;
    }
}

/// Nettoie les séquences expirées (à appeler périodiquement).
pub fn cleanup_expired() -> u32 {
    let mut det = SEQUENCE_DETECTOR.lock();
    let now_tsc = read_tsc();
    let mut cleaned = 0u32;

    for i in 0..det.state_count {
        let mut state = det.active_states[i];
        if !state.active {
            continue;
        }

        let def = det.definitions[state.def_index];
        if state.is_timed_out(now_tsc, def.global_timeout_tsc) {
            state.active = false;
            det.active_states[i] = state;
            cleaned += 1;
        } else if state.current_step > 0 && state.current_step < state.total_steps {
            let step = &def.steps[state.current_step as usize];
            if now_tsc.wrapping_sub(state.last_event_tsc) > step.timeout_tsc {
                state.active = false;
                det.active_states[i] = state;
                cleaned += 1;
            }
        } else {
            det.active_states[i] = state;
        }
    }

    if cleaned > 0 {
        TOTAL_SEQUENCES_TIMEOUT.fetch_add(cleaned as u64, Ordering::Relaxed);
    }

    cleaned
}

/// Statistiques du détecteur de séquences.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SequenceStats {
    pub definition_count: usize,
    pub active_sequences: usize,
    pub total_events_processed: u64,
    pub total_sequences_completed: u64,
    pub total_sequences_timed_out: u64,
    pub alert_count: usize,
}

/// Retourne les statistiques du détecteur.
pub fn get_sequence_stats() -> SequenceStats {
    let det = SEQUENCE_DETECTOR.lock();
    let mut active = 0usize;
    for i in 0..det.state_count {
        if det.active_states[i].active {
            active += 1;
        }
    }
    SequenceStats {
        definition_count: det.def_count,
        active_sequences: active,
        total_events_processed: TOTAL_EVENTS_PROCESSED.load(Ordering::Relaxed),
        total_sequences_completed: TOTAL_SEQUENCES_COMPLETED.load(Ordering::Relaxed),
        total_sequences_timed_out: TOTAL_SEQUENCES_TIMEOUT.load(Ordering::Relaxed),
        alert_count: det.alert_count,
    }
}

/// Initialise le détecteur de séquences.
pub fn sequence_init() {
    let mut det = SEQUENCE_DETECTOR.lock();
    for i in 0..MAX_SEQUENCES {
        det.definitions[i] = SequenceDefinition::empty();
        det.active_states[i] = SequenceState::empty();
    }
    for i in 0..MAX_SEQUENCE_ALERTS {
        det.alerts[i] = SequenceAlert::empty();
    }
    det.def_count = 0;
    det.state_count = 0;
    det.alert_head = 0;
    det.alert_count = 0;

    TOTAL_EVENTS_PROCESSED.store(0, Ordering::Release);
    TOTAL_SEQUENCES_COMPLETED.store(0, Ordering::Release);
    TOTAL_SEQUENCES_TIMEOUT.store(0, Ordering::Release);
}
