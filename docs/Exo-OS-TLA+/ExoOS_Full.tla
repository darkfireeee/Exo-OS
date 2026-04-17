--------------------------- MODULE ExoOS_Full ---------------------------
EXTENDS Naturals, Integers, Sequences, FiniteSets, TLC

\* =========================================================================
\* GLOBAL CONSTANTS
\* =========================================================================
CONSTANTS 
    CORES, CORES_A, IRQS, MAX_PENDING_ACKS, MAX_OVERFLOWS, MAX_GEN, PIDS,
    MAX_TIMER, TIMEOUT_TICKS, MAX_TICKS, N_CYCLES

\* =========================================================================
\* GLOBAL SYSTEM VARIABLES
\* =========================================================================
VARIABLES
    \* Mod 1: Phoenix Handoff
    CoreState, FpuActive, FpuSaved, TlbShootdownActive, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer,
    \* Mod 3: IRQ Routing
    PendingAcks, MaskedSince, OverflowCount, DispatchGen, LapicIsrBit, EoiSent,
    \* Mod 12: Adversarial
    Attacker, AttackSuccessful, SystemIntegrity, RecoveryInProgress, CryptoServerState, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount, Ticks,
    \* SHARED VARIABLES
    HandoffFlag,     
    BlacklistedIrqs  

vars_phoenix == <<CoreState, FpuActive, FpuSaved, TlbShootdownActive, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer>>
vars_irq == <<PendingAcks, MaskedSince, OverflowCount, DispatchGen, LapicIsrBit, EoiSent>>
vars_adv == <<Attacker, AttackSuccessful, SystemIntegrity, RecoveryInProgress, CryptoServerState, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount, Ticks>>

\* =========================================================================
\* SUBSYSTEM INSTANTIATION & MAPPING
\* =========================================================================
Phoenix == INSTANCE ExoPhoenixHandoff
    WITH CoreState <- CoreState, FpuActive <- FpuActive, FpuSaved <- FpuSaved, 
         TlbShootdownActive <- TlbShootdownActive, HandoffFlag <- HandoffFlag, 
         FreezeAckBitmap <- FreezeAckBitmap, NmiWatchdogStrikes <- NmiWatchdogStrikes, 
         KernelBState <- KernelBState, EpochID <- EpochID, NonceSeed <- NonceSeed, FreezeTimer <- FreezeTimer

IRQ == INSTANCE IrqRoutingStress
    WITH PendingAcks <- PendingAcks, MaskedSince <- MaskedSince, OverflowCount <- OverflowCount, 
         DispatchGen <- DispatchGen, BlacklistedIrqs <- BlacklistedIrqs, LapicIsrBit <- LapicIsrBit, EoiSent <- EoiSent

Adversary == INSTANCE Adversarial
    WITH Attacker <- Attacker, AttackSuccessful <- AttackSuccessful, SystemIntegrity <- SystemIntegrity, 
         RecoveryInProgress <- RecoveryInProgress, HandoffFlag <- HandoffFlag, CryptoServerState <- CryptoServerState, 
         BlacklistedIrqs <- BlacklistedIrqs, ExpiredTokens <- ExpiredTokens, LedgerP0Entries <- LedgerP0Entries, 
         PhoenixCycleCount <- PhoenixCycleCount, Ticks <- Ticks

\* =========================================================================
\* INITIALIZATION
\* =========================================================================
Init ==
    /\ Phoenix!Init
    /\ IRQ!Init
    /\ Adversary!Init

\* =========================================================================
\* ASYNCHRONOUS INTERLEAVING (The Core Engine)
\* =========================================================================
Next ==
    \* FIX: If Phoenix fires, lock down BlacklistedIrqs explicitly
    \/ (Phoenix!Next /\ UNCHANGED vars_irq /\ UNCHANGED vars_adv /\ UNCHANGED BlacklistedIrqs)
    \* FIX: If IRQ fires, lock down HandoffFlag explicitly
    \/ (IRQ!Next /\ UNCHANGED vars_phoenix /\ UNCHANGED vars_adv /\ UNCHANGED HandoffFlag)
    \* Adversary handles both shared variables natively, so no extra UNCHANGED needed here
    \/ (Adversary!Next /\ UNCHANGED vars_phoenix /\ UNCHANGED vars_irq)

Spec == Init /\ [][Next]_<<vars_phoenix, vars_irq, vars_adv, HandoffFlag, BlacklistedIrqs>>

\* =========================================================================
\* GLOBAL INVARIANTS & PROPERTIES
\* =========================================================================
S_GLOBAL_1 == Phoenix!S1_NoSimultaneousExecution
S_GLOBAL_2 == Phoenix!S2_AllCoresHaltedBeforeAckAll
S_GLOBAL_3 == IRQ!S9_STRESS_SimultaneousLevelStorm
S_GLOBAL_4 == Adversary!S_ADV1
S_GLOBAL_5 == Adversary!S_ADV3

=============================================================================
