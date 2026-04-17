--------------------------- MODULE ProcessDeath ---------------------------
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS PIDS, FDS, OBJECTS, THREADS

VARIABLES
    ProcessStates,      \* Mapping: PID -> {RUNNING, DYING, ZOMBIE, DEAD}
    InitServerNotified, \* Mapping: PID -> BOOLEAN
    FdTableState,       \* Mapping: (PID, FD) -> {ACTIVE, STALE, CLOSED}
    FdObjectIds,        \* Mapping: (PID, FD) -> OBJECTS
    WaitersOnFd,        \* Mapping: (PID, FD) -> SUBSET THREADS
    WaitersNotified,    \* Mapping: THREAD -> {NONE, EIO, EOF}
    ExofsObjectExists,  \* Mapping: OBJECTS -> BOOLEAN
    SystemPhase         \* {NORMAL, RESTORING}

vars == <<ProcessStates, InitServerNotified, FdTableState, FdObjectIds, WaitersOnFd, WaitersNotified, ExofsObjectExists, SystemPhase>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ ProcessStates = [p \in PIDS |-> "RUNNING"]
    /\ InitServerNotified = [p \in PIDS |-> FALSE]
    /\ FdTableState = [p \in PIDS, f \in FDS |-> "ACTIVE"]
    /\ FdObjectIds = [p \in PIDS, f \in FDS |-> CHOOSE o \in OBJECTS : TRUE] \* All map to something
    /\ WaitersOnFd = [p \in PIDS, f \in FDS |-> THREADS] \* Simplify: all threads waiting on all FDs for stress
    /\ WaitersNotified = [t \in THREADS |-> "NONE"]
    /\ ExofsObjectExists = [o \in OBJECTS |-> TRUE]
    /\ SystemPhase = "NORMAL"

--------------------------------------------------------------
\* SYSTEM ACTIONS: PROCESS LIFECYCLE
--------------------------------------------------------------
\* SRV-01: A process panics or aborts
ProcessPanic(p) ==
    /\ SystemPhase = "NORMAL"
    /\ ProcessStates[p] = "RUNNING"
    /\ ProcessStates' = [ProcessStates EXCEPT ![p] = "DYING"]
    /\ UNCHANGED <<InitServerNotified, FdTableState, FdObjectIds, WaitersOnFd, WaitersNotified, ExofsObjectExists, SystemPhase>>

\* SRV-01: Kernel reaps the dying process and guarantees ChildDied IPC to init_server (PID 1)
KernelReap(p) ==
    /\ SystemPhase = "NORMAL"
    /\ ProcessStates[p] = "DYING"
    /\ ProcessStates' = [ProcessStates EXCEPT ![p] = "DEAD"]
    /\ InitServerNotified' = [InitServerNotified EXCEPT ![p] = TRUE] \* Atomic guarantee
    /\ UNCHANGED <<FdTableState, FdObjectIds, WaitersOnFd, WaitersNotified, ExofsObjectExists, SystemPhase>>

--------------------------------------------------------------
\* SYSTEM ACTIONS: PHOENIX RESTORE & FD STALENESS
--------------------------------------------------------------
\* Simulate a hard crash. We wake up in RESTORING phase. 
\* The disk rolled back, so some ObjectIds no longer exist!
CrashAndRestore ==
    /\ SystemPhase = "NORMAL"
    /\ SystemPhase' = "RESTORING"
    \* Adversarial/Environment action: Randomly delete objects to simulate disk drift
    /\ \E deleted_objs \in SUBSET OBJECTS :
        ExofsObjectExists' = [o \in OBJECTS |-> IF o \in deleted_objs THEN FALSE ELSE ExofsObjectExists[o]]
    /\ UNCHANGED <<ProcessStates, InitServerNotified, FdTableState, FdObjectIds, WaitersOnFd, WaitersNotified>>

\* CORR-50: The kernel validates FDs. If object is gone, mark_stale() is called, NOT close().
ValidateFd_MarkStale(p, f) ==
    /\ SystemPhase = "RESTORING"
    /\ FdTableState[p, f] = "ACTIVE"
    /\ ExofsObjectExists[FdObjectIds[p, f]] = FALSE
    \* Atomically mark stale AND wake up all waiters with EIO
    /\ FdTableState' = [FdTableState EXCEPT ![p, f] = "STALE"]
    /\ WaitersNotified' = [t \in THREADS |-> 
            IF t \in WaitersOnFd[p, f] THEN "EIO" ELSE WaitersNotified[t]]
    /\ UNCHANGED <<ProcessStates, InitServerNotified, FdObjectIds, WaitersOnFd, ExofsObjectExists, SystemPhase>>

\* Validate a healthy FD
ValidateFd_Healthy(p, f) ==
    /\ SystemPhase = "RESTORING"
    /\ FdTableState[p, f] = "ACTIVE"
    /\ ExofsObjectExists[FdObjectIds[p, f]] = TRUE
    /\ UNCHANGED vars \* Nothing to do, it's healthy

\* Once all FDs are checked, the system resumes normal operation
FinishRestore ==
    /\ SystemPhase = "RESTORING"
    \* Guard: Cannot finish until all ACTIVE FDs map to existing objects
    /\ \A p \in PIDS, f \in FDS : 
        (FdTableState[p, f] = "ACTIVE") => (ExofsObjectExists[FdObjectIds[p, f]] = TRUE)
    /\ SystemPhase' = "NORMAL"
    /\ UNCHANGED <<ProcessStates, InitServerNotified, FdTableState, FdObjectIds, WaitersOnFd, WaitersNotified, ExofsObjectExists>>

Next ==
    \/ \E p \in PIDS : ProcessPanic(p)
    \/ \E p \in PIDS : KernelReap(p)
    \/ CrashAndRestore
    \/ \E p \in PIDS, f \in FDS : ValidateFd_MarkStale(p, f)
    \/ \E p \in PIDS, f \in FDS : ValidateFd_Healthy(p, f)
    \/ FinishRestore

Spec == Init /\ [][Next]_vars

--------------------------------------------------------------
\* SAFETY PROPERTIES & INVARIANTS
--------------------------------------------------------------
\* S44: The kernel guarantees ChildDied delivery independently of the dying process state.
S44_ChildDiedAlwaysDelivered ==
    \A p \in PIDS : (ProcessStates[p] = "DEAD") => (InitServerNotified[p] = TRUE)

\* S45: mark_stale() prevents deadlocks. A stale FD immediately yields EIO to waiters.
S45_StaleNotifiesWaiters ==
    \A p \in PIDS, f \in FDS :
        (FdTableState[p, f] = "STALE") => 
            (\A t \in WaitersOnFd[p, f] : WaitersNotified[t] = "EIO")

\* S46: Post-restore, no active FD points to a deleted object.
S46_NoStaleObjectIds ==
    (SystemPhase = "NORMAL") => 
        (\A p \in PIDS, f \in FDS :
            (FdTableState[p, f] = "ACTIVE") => (ExofsObjectExists[FdObjectIds[p, f]] = TRUE))

=============================================================================
