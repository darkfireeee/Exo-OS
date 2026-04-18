----------------------------- MODULE ExoShield_v1 -----------------------------
EXTENDS Integers, Sequences, TLC

CONSTANTS Services, DefaultBudget, P0Capacity

VARIABLES
    SecurityReady,
    NetworkEnabled,
    MutableFS,
    HandoffFlag,
    BudgetMap,
    P0Log

vars == <<SecurityReady, NetworkEnabled, MutableFS, HandoffFlag, BudgetMap, P0Log>>

HasEvent(log, event) == \E i \in 1..Len(log) : log[i] = event

Prefix(a, b) ==
    /\ Len(a) <= Len(b)
    /\ \A i \in 1..Len(a) : a[i] = b[i]

Init ==
    /\ SecurityReady = FALSE
    /\ NetworkEnabled = FALSE
    /\ MutableFS = FALSE
    /\ HandoffFlag = "BOOT"
    /\ BudgetMap = [s \in Services |-> DefaultBudget]
    /\ P0Log = <<"NicIommuLocked", "BootPhase0">>

CompleteBoot ==
    /\ HandoffFlag = "BOOT"
    /\ SecurityReady = FALSE
    /\ SecurityReady' = TRUE
    /\ NetworkEnabled' = TRUE
    /\ MutableFS' = TRUE
    /\ HandoffFlag' = "NORMAL"
    /\ BudgetMap' = BudgetMap
    /\ P0Log' =
        IF Len(P0Log) < P0Capacity
        THEN Append(P0Log, "SecurityReady")
        ELSE P0Log

UseBudget(s) ==
    /\ HandoffFlag = "NORMAL"
    /\ SecurityReady = TRUE
    /\ s \in Services
    /\ BudgetMap[s] > 0
    /\ SecurityReady' = SecurityReady
    /\ NetworkEnabled' = NetworkEnabled
    /\ MutableFS' = MutableFS
    /\ HandoffFlag' = HandoffFlag
    /\ BudgetMap' = [BudgetMap EXCEPT ![s] = @ - 1]
    /\ P0Log' = P0Log

TriggerHandoff(event) ==
    /\ HandoffFlag \in {"BOOT", "NORMAL"}
    /\ event \in {"CpViolation", "IommuFault", "BootSealViolation"}
    /\ SecurityReady' = FALSE
    /\ NetworkEnabled' = FALSE
    /\ MutableFS' = FALSE
    /\ HandoffFlag' = "FREEZE_REQ"
    /\ BudgetMap' = BudgetMap
    /\ P0Log' =
        IF Len(P0Log) < P0Capacity
        THEN Append(P0Log, event)
        ELSE P0Log

Stutter ==
    /\ UNCHANGED vars

Next ==
    \/ CompleteBoot
    \/ \E s \in Services : UseBudget(s)
    \/ \E event \in {"CpViolation", "IommuFault", "BootSealViolation"} : TriggerHandoff(event)
    \/ Stutter

Spec == Init /\ [][Next]_vars

\* S1 : Boot
S1_BootSafety ==
    ~SecurityReady => (~NetworkEnabled /\ ~MutableFS)

\* S2 : Réseau / verrouillage NIC
S2_NicIommuLockedBeforeNormal ==
    SecurityReady => HasEvent(P0Log, "NicIommuLocked")

\* S3 : Flux de contrôle
S3_ControlFlowViolationForcesHandoff ==
    HasEvent(P0Log, "CpViolation") => HandoffFlag = "FREEZE_REQ"

\* S4 : Budgets monotones
S4_BudgetMonotonicity ==
    [][\A s \in Services : BudgetMap'[s] <= BudgetMap[s]]_vars

\* S5 : Handoff atomique
S5_HandoffAtomicity ==
    [][(HandoffFlag = "FREEZE_REQ") => (HandoffFlag' = "FREEZE_REQ")]_vars

\* S6 : Audit P0 immuable
S6_P0Immutability ==
    [][Prefix(P0Log, P0Log')]_vars

=============================================================================
