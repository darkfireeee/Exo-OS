------------------------- MODULE PhoenixState -------------------------
EXTENDS Naturals, FiniteSets, TLC

(***************************************************************************)
(* ExoPhoenix — 3-Core SMP "Evil AI" Adversary Model                       *)
(* Symmetry Disabled for Brute-Force State Checking                        *)
(* Incorporating CORR-12, CORR-13, CORR-14, CORR-15                        *)
(***************************************************************************)

CONSTANTS Cores

VARIABLES
    mode, 
    ack, 
    frozen,
    systemCompromised,
    (* Defenses / Subsystems *)
    busMasterActive,  \* CORR-14: Must be false before snapshot
    fpuSaved,         \* CORR-15: FPU state per core
    nonceReseeded,    \* CORR-12: Must be true upon restore
    vfsSynced,        \* CORR-13: Must be true before ACK
    coreStatus

vars == << mode, ack, frozen, systemCompromised, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

AllAcked == \A c \in Cores : ack[c]

(* ========================================================================= *)
(* 1. SAFETY INVARIANTS                                                      *)
(* ========================================================================= *)
TypeOK ==
    /\ mode \in {"Boot", "Normal", "PrepareIsolation", "Snapshot", "Restore", "Degraded"}
    /\ ack \in [Cores -> BOOLEAN]
    /\ frozen \in [Cores -> BOOLEAN]
    /\ fpuSaved \in [Cores -> BOOLEAN]
    /\ coreStatus \in [Cores -> {"Alive", "Dead"}]
    /\ systemCompromised \in BOOLEAN
    /\ busMasterActive \in BOOLEAN
    /\ nonceReseeded \in BOOLEAN
    /\ vfsSynced \in BOOLEAN

(* The Ultimate Safety Check: The Evil AI must never succeed *)
NoCompromise == ~systemCompromised

(* ========================================================================= *)
(* 2. THE EVIL AI ADVERSARY (Hunting for vulnerabilities)                    *)
(* ========================================================================= *)

AdversaryDMAExploit ==
    \* AI attempts memory corruption via malicious device during SSR Snapshot
    /\ mode = "Snapshot"
    /\ busMasterActive  \* If device_server didn't disable Bus Mastering (CORR-14)
    /\ systemCompromised' = TRUE
    /\ UNCHANGED << mode, ack, frozen, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

AdversaryCryptoReplay ==
    \* AI attempts to reuse ChaCha20 nonces after a RAM restore
    /\ mode = "Normal"
    /\ ~nonceReseeded  \* If crypto_server didn't get PhoenixWakeEntropy (CORR-12)
    /\ systemCompromised' = TRUE
    /\ UNCHANGED << mode, ack, frozen, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

AdversaryFPUCorruption(c) ==
    \* AI triggers complex AVX/SSE math after restore to check for FPU loss
    /\ mode = "Restore"
    /\ coreStatus[c] = "Alive"
    /\ ~fpuSaved[c]    \* If IDT 0xF3 didn't force XSAVE (CORR-15)
    /\ systemCompromised' = TRUE
    /\ UNCHANGED << mode, ack, frozen, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

AdversaryCorruptSnapshot ==
    \* AI wins ONLY if it forces a snapshot before VFS is synced or while DMA is active
    /\ mode = "Snapshot"
    /\ (~vfsSynced \/ busMasterActive)
    /\ systemCompromised' = TRUE
    /\ UNCHANGED << mode, ack, frozen, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

AdversaryKillsCore(c) ==
    \* Basic AI hardware attack
    /\ coreStatus[c] = "Alive"
    /\ coreStatus' = [coreStatus EXCEPT ![c] = "Dead"]
    /\ UNCHANGED << mode, ack, frozen, systemCompromised, busMasterActive, fpuSaved, nonceReseeded, vfsSynced >>

(* ========================================================================= *)
(* 3. SYSTEM STATE MACHINE & DEFENSES                                        *)
(* ========================================================================= *)

Init ==
    /\ mode = "Boot"
    /\ ack = [c \in Cores |-> FALSE]
    /\ frozen = [c \in Cores |-> FALSE]
    /\ fpuSaved = [c \in Cores |-> TRUE] \* Clean state at boot
    /\ coreStatus = [c \in Cores |-> "Alive"]
    /\ systemCompromised = FALSE
    /\ busMasterActive = TRUE
    /\ nonceReseeded = TRUE
    /\ vfsSynced = TRUE

BootToNormal ==
    /\ mode = "Boot"
    /\ mode' = "Normal"
    /\ UNCHANGED << ack, frozen, systemCompromised, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

ThreatDetected ==
    /\ mode = "Normal"
    /\ mode' = "PrepareIsolation"
    /\ ack' = [c \in Cores |-> FALSE]
    /\ frozen' = [c \in Cores |-> FALSE]
    /\ fpuSaved' = [c \in Cores |-> FALSE] \* FPU state gets dirty
    /\ nonceReseeded' = FALSE              \* Nonce becomes stale due to impending snapshot
    /\ vfsSynced' = FALSE                  \* Page cache becomes dirty
    /\ UNCHANGED << systemCompromised, busMasterActive, coreStatus >>

(* --- THE DEFENSES --- *)

Defend_VfsServer_Corr13 ==
    /\ mode = "PrepareIsolation"
    /\ ~vfsSynced
    /\ vfsSynced' = TRUE
    /\ UNCHANGED << mode, ack, frozen, systemCompromised, busMasterActive, fpuSaved, nonceReseeded, coreStatus >>

Defend_DeviceServer_Corr14 ==
    /\ mode = "PrepareIsolation"
    /\ busMasterActive
    /\ busMasterActive' = FALSE
    /\ UNCHANGED << mode, ack, frozen, systemCompromised, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

Defend_HandleFreezeIPI_Corr15(c) ==
    /\ mode = "PrepareIsolation"
    /\ coreStatus[c] = "Alive"
    /\ ~ack[c]
    /\ fpuSaved' = [fpuSaved EXCEPT ![c] = TRUE] \* Force XSAVE64
    /\ ack' = [ack EXCEPT ![c] = TRUE]
    /\ frozen' = [frozen EXCEPT ![c] = TRUE]
    /\ UNCHANGED << mode, systemCompromised, busMasterActive, nonceReseeded, vfsSynced, coreStatus >>

(* ------------------------------------------------ *)

BeginSnapshot ==
    /\ mode = "PrepareIsolation"
    /\ AllAcked
    /\ vfsSynced
    /\ ~busMasterActive
    /\ mode' = "Snapshot"
    /\ UNCHANGED << ack, frozen, systemCompromised, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

SnapshotToRestore ==
    /\ mode = "Snapshot"
    /\ mode' = "Restore"
    /\ UNCHANGED << ack, frozen, systemCompromised, busMasterActive, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

Defend_PhoenixWakeSequence_Corr12 ==
    /\ mode = "Restore"
    /\ ~nonceReseeded
    /\ nonceReseeded' = TRUE  \* HKDF reseed with new epoch_id
    /\ UNCHANGED << mode, ack, frozen, systemCompromised, busMasterActive, fpuSaved, vfsSynced, coreStatus >>

WakeToNormal ==
    /\ mode = "Restore"
    /\ nonceReseeded
    /\ mode' = "Normal"
    /\ busMasterActive' = TRUE  \* Devices re-enabled post-wake
    /\ ack' = [c \in Cores |-> FALSE]
    /\ frozen' = [c \in Cores |-> FALSE]
    /\ UNCHANGED << systemCompromised, fpuSaved, nonceReseeded, vfsSynced, coreStatus >>

Next ==
    \/ BootToNormal
    \/ ThreatDetected
    \/ Defend_VfsServer_Corr13
    \/ Defend_DeviceServer_Corr14
    \/ (\E c \in Cores : Defend_HandleFreezeIPI_Corr15(c))
    \/ BeginSnapshot
    \/ SnapshotToRestore
    \/ Defend_PhoenixWakeSequence_Corr12
    \/ WakeToNormal
    \* Adversary Actions
    \/ AdversaryDMAExploit
    \/ AdversaryCryptoReplay
    \/ (\E c \in Cores : AdversaryFPUCorruption(c))
    \/ AdversaryCorruptSnapshot
    \/ (\E c \in Cores : AdversaryKillsCore(c))

Spec == Init /\ [][Next]_vars

=============================================================================
