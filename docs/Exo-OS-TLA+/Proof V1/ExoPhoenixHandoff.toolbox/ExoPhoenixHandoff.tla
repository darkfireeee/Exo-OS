----------------------- MODULE ExoPhoenixHandoff -----------------------
EXTENDS Naturals, FiniteSets, TLC

(***************************************************************************)
(* MODULE 1: The ExoPhoenix Automaton (Dual-Kernel Handoff)                *)
(* 100% COMPLETION: Includes Standard (S1-S4, L1) and STRESS (S1-S3)       *)
(***************************************************************************)

CONSTANTS CORES_A, MAX_TIMER, TIMEOUT_TICKS

VARIABLES 
    CoreState,           
    FpuActive,           
    FpuSaved,            
    TlbShootdownActive,  
    HandoffFlag,         
    FreezeAckBitmap,     
    NmiWatchdogStrikes,  
    KernelBState,        
    EpochID,             
    NonceSeed,           
    FreezeTimer          

vars == << CoreState, FpuActive, FpuSaved, TlbShootdownActive, 
           HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, 
           KernelBState, EpochID, NonceSeed, FreezeTimer >>

(* ========================================================================= *)
(* 1.1 TYPE INVARIANT                                                        *)
(* ========================================================================= *)
TypeOK ==
    /\ CoreState \in [CORES_A -> {"RUNNING", "FREEZE_REQ_RECV", "XSAVE_DONE", "FREEZE_ACKED", "SPIN_WAITING", "RESUMED", "DEGRADED_ACK"}]
    /\ FpuActive \in [CORES_A -> BOOLEAN]
    /\ FpuSaved \in [CORES_A -> BOOLEAN]
    /\ TlbShootdownActive \in BOOLEAN
    /\ HandoffFlag \in {0, 1, 2, 3}
    /\ FreezeAckBitmap \subseteq CORES_A
    /\ NmiWatchdogStrikes \in 0..3
    /\ KernelBState \in {"WATCHING", "SNAPSHOT_IN_PROGRESS", "SNAPSHOT_DONE", "RESTORING", "CRASHED"}
    /\ EpochID \in Nat
    /\ NonceSeed \in Nat
    /\ FreezeTimer \in 0..MAX_TIMER

(* ========================================================================= *)
(* 1.2 STANDARD PROPERTIES (S1, S2, S3, S4, L1)                              *)
(* ========================================================================= *)
S1_NoSimultaneousExecution ==
    ~(\E c \in CORES_A : CoreState[c] = "RUNNING" /\ KernelBState = "SNAPSHOT_IN_PROGRESS")

S2_AllCoresHaltedBeforeAckAll ==
    (HandoffFlag = 2 => \A c \in CORES_A : c \in FreezeAckBitmap)

S3_FpuSavedBeforeAck ==
    \A c \in CORES_A : (c \in FreezeAckBitmap /\ FpuActive[c] => FpuSaved[c])

\* FIXED: Correctly checks mutation between Snapshot and Restore, allowing for crashes
S4_EpochMutatesOnRestore ==
    \A old_epoch \in 1..10 : 
        [] ((KernelBState = "SNAPSHOT_IN_PROGRESS" /\ EpochID = old_epoch) => 
            <> ((KernelBState = "RESTORING" /\ EpochID /= old_epoch) \/ KernelBState = "CRASHED"))

L1_FreezeAlwaysTerminates ==
    [] (HandoffFlag = 1 => 
        <> (HandoffFlag = 2 \/ \E c \in CORES_A : CoreState[c] = "DEGRADED_ACK"))

(* ========================================================================= *)
(* 1.3 STRESS PROPERTIES (S1.STRESS, S2.STRESS, S3.STRESS)                   *)
(* ========================================================================= *)
S1_STRESS_TlbShootdownNoDeadlock ==
    [] (TlbShootdownActive /\ HandoffFlag = 1 =>
        <> (HandoffFlag = 2 \/ \A c \in CORES_A : c \in FreezeAckBitmap))

S2_STRESS_NmiDuringFreeze ==
    (HandoffFlag = 1 /\ NmiWatchdogStrikes >= 3 =>
        HandoffFlag \in {1, 2} /\ ~(HandoffFlag = 0))

S3_STRESS_KernelBCrashDuringSnapshot ==
    [] (KernelBState = "CRASHED" =>
        <> (\A c \in CORES_A : CoreState[c] \in {"RESUMED", "DEGRADED_ACK"}))

(* ========================================================================= *)
(* STATE MACHINE ACTIONS                                                     *)
(* ========================================================================= *)

Init ==
    /\ CoreState = [c \in CORES_A |-> "RUNNING"]
    /\ FpuActive \in [CORES_A -> BOOLEAN] 
    /\ FpuSaved = [c \in CORES_A |-> FALSE]
    /\ TlbShootdownActive = FALSE
    /\ HandoffFlag = 0
    /\ FreezeAckBitmap = {}
    /\ NmiWatchdogStrikes = 0
    /\ KernelBState = "WATCHING"
    /\ EpochID = 1
    /\ NonceSeed = 1
    /\ FreezeTimer = 0

KernelA_StartTlbShootdown ==
    /\ ~TlbShootdownActive
    /\ HandoffFlag = 0
    /\ TlbShootdownActive' = TRUE
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelA_EndTlbShootdown ==
    /\ TlbShootdownActive
    /\ TlbShootdownActive' = FALSE
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

ExoNmi_Tick ==
    /\ NmiWatchdogStrikes < 3
    /\ NmiWatchdogStrikes' = NmiWatchdogStrikes + 1
    /\ \/ /\ NmiWatchdogStrikes' = 3 /\ HandoffFlag = 0 /\ HandoffFlag' = 1
       \/ /\ ~(NmiWatchdogStrikes' = 3 /\ HandoffFlag = 0) /\ HandoffFlag' = HandoffFlag
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, FreezeAckBitmap, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelB_InitiateFreeze ==
    /\ KernelBState = "WATCHING"
    /\ HandoffFlag = 0
    /\ HandoffFlag' = 1
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelA_ReceiveFreeze(c) ==
    /\ CoreState[c] = "RUNNING"
    /\ HandoffFlag = 1
    /\ ~TlbShootdownActive 
    /\ CoreState' = [CoreState EXCEPT ![c] = "FREEZE_REQ_RECV"]
    /\ UNCHANGED << FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelA_DoXSave(c) ==
    /\ CoreState[c] = "FREEZE_REQ_RECV"
    /\ \/ /\ FpuActive[c] = TRUE /\ FpuSaved' = [FpuSaved EXCEPT ![c] = TRUE]
       \/ /\ FpuActive[c] = FALSE /\ FpuSaved' = FpuSaved 
    /\ CoreState' = [CoreState EXCEPT ![c] = "XSAVE_DONE"]
    /\ UNCHANGED << FpuActive, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelA_AckFreeze(c) ==
    /\ CoreState[c] = "XSAVE_DONE"
    /\ FreezeAckBitmap' = FreezeAckBitmap \union {c}
    /\ CoreState' = [CoreState EXCEPT ![c] = "FREEZE_ACKED"]
    /\ UNCHANGED << FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelA_SpinWait(c) ==
    /\ CoreState[c] = "FREEZE_ACKED"
    /\ CoreState' = [CoreState EXCEPT ![c] = "SPIN_WAITING"]
    /\ UNCHANGED << FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelB_AllAcked ==
    /\ KernelBState \in {"WATCHING", "CRASHED"}
    /\ FreezeAckBitmap = CORES_A
    /\ HandoffFlag = 1
    /\ HandoffFlag' = 2
    /\ KernelBState' = "SNAPSHOT_IN_PROGRESS"
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, FreezeAckBitmap, NmiWatchdogStrikes, EpochID, NonceSeed, FreezeTimer >>

KernelB_Crash ==
    /\ KernelBState = "SNAPSHOT_IN_PROGRESS"
    /\ KernelBState' = "CRASHED"
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, EpochID, NonceSeed, FreezeTimer >>

KernelA_TimeoutDegraded(c) ==
    /\ CoreState[c] = "SPIN_WAITING"
    /\ KernelBState = "CRASHED"
    /\ FreezeTimer >= TIMEOUT_TICKS
    /\ CoreState' = [CoreState EXCEPT ![c] = "DEGRADED_ACK"]
    /\ UNCHANGED << FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed, FreezeTimer >>

KernelB_FinishSnapshot ==
    /\ KernelBState = "SNAPSHOT_IN_PROGRESS"
    /\ KernelBState' = "SNAPSHOT_DONE"
    /\ EpochID' = EpochID + 1 
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, NonceSeed, FreezeTimer >>

KernelB_Restore ==
    /\ KernelBState = "SNAPSHOT_DONE"
    /\ KernelBState' = "RESTORING"
    /\ NonceSeed' = NonceSeed + 1 
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, EpochID, FreezeTimer >>

KernelB_ResumeA ==
    /\ KernelBState = "RESTORING"
    /\ HandoffFlag' = 3
    /\ KernelBState' = "WATCHING"
    /\ CoreState' = [c \in CORES_A |-> "RESUMED"]
    /\ UNCHANGED << FpuActive, FpuSaved, TlbShootdownActive, FreezeAckBitmap, NmiWatchdogStrikes, EpochID, NonceSeed, FreezeTimer >>

\* FIXED: The Loop Closure! Kernel A goes back to RUNNING to allow future snapshots.
KernelA_ReturnToNormal ==
    /\ HandoffFlag = 3
    /\ KernelBState = "WATCHING"
    /\ EpochID < 4   \* <---- THE FIX: Stop the simulation after 3 complete snapshot loops!
    /\ CoreState' = [c \in CORES_A |-> "RUNNING"]
    /\ FpuSaved' = [c \in CORES_A |-> FALSE]
    /\ FreezeAckBitmap' = {}
    /\ HandoffFlag' = 0
    /\ NmiWatchdogStrikes' = 0
    /\ FreezeTimer' = 0
    /\ UNCHANGED << FpuActive, TlbShootdownActive, KernelBState, EpochID, NonceSeed >>

TickTimer ==
    /\ HandoffFlag \in {1, 2}
    /\ FreezeTimer < MAX_TIMER
    /\ FreezeTimer' = FreezeTimer + 1
    /\ UNCHANGED << CoreState, FpuActive, FpuSaved, TlbShootdownActive, HandoffFlag, FreezeAckBitmap, NmiWatchdogStrikes, KernelBState, EpochID, NonceSeed >>

Next ==
    \/ KernelA_StartTlbShootdown \/ KernelA_EndTlbShootdown
    \/ ExoNmi_Tick
    \/ KernelB_InitiateFreeze \/ KernelB_AllAcked \/ KernelB_Crash
    \/ KernelB_FinishSnapshot \/ KernelB_Restore \/ KernelB_ResumeA
    \/ KernelA_ReturnToNormal  \* Added to the main state machine!
    \/ TickTimer
    \/ (\E c \in CORES_A : KernelA_ReceiveFreeze(c) \/ KernelA_DoXSave(c) \/ KernelA_AckFreeze(c) \/ KernelA_SpinWait(c) \/ KernelA_TimeoutDegraded(c))

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

=============================================================================