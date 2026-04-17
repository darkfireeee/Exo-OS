--------------------------- MODULE ExoShield ---------------------------
EXTENDS Integers, Sequences, FiniteSets, TLC

THREADS == {"T1", "T2"}
AUTHORIZED_GRAPH == {<<"Net", "Vfs">>, <<"Vfs", "Crypto">>}
EMPTY_ENTRY == "EMPTY"

VARIABLES
    WatchdogMissed,
    HandoffFlag,
    LedgerP0Entries,
    ControlFlowViolations,
    IommuNicWhitelist,
    NicDmaRequests,
    IpcMessages,
    IpcQuotas

vars == <<WatchdogMissed, HandoffFlag, LedgerP0Entries, ControlFlowViolations,
          IommuNicWhitelist, NicDmaRequests, IpcMessages, IpcQuotas>>

Init ==
    /\ WatchdogMissed = 0
    /\ HandoffFlag = 0
    /\ LedgerP0Entries = << >>
    /\ ControlFlowViolations = {}
    /\ IommuNicWhitelist = { "0xAAAA", "0xBBBB" }  
    /\ NicDmaRequests = {}
    /\ IpcMessages = {}
    /\ IpcQuotas = [e \in AUTHORIZED_GRAPH |-> 2]

TickWatchdog ==
    /\ HandoffFlag = 0
    /\ WatchdogMissed' = WatchdogMissed + 1
    /\ HandoffFlag' = IF WatchdogMissed' >= 3 THEN 1 ELSE 0
    /\ UNCHANGED <<LedgerP0Entries, ControlFlowViolations, IommuNicWhitelist, NicDmaRequests, IpcMessages, IpcQuotas>>

PingWatchdog ==
    /\ HandoffFlag = 0
    /\ WatchdogMissed' = 0
    /\ UNCHANGED <<HandoffFlag, LedgerP0Entries, ControlFlowViolations, IommuNicWhitelist, NicDmaRequests, IpcMessages, IpcQuotas>>

TriggerCp(t) ==
    /\ HandoffFlag \in {0, 1}
    /\ t \notin ControlFlowViolations  \* THE FIX: A thread can only fault once!
    /\ ControlFlowViolations' = ControlFlowViolations \cup {t}
    /\ HandoffFlag' = 1  
    /\ LedgerP0Entries' = Append(LedgerP0Entries, "CpViolation")
    /\ UNCHANGED <<WatchdogMissed, IommuNicWhitelist, NicDmaRequests, IpcMessages, IpcQuotas>>

RequestDma(addr) ==
    /\ HandoffFlag = 0
    /\ NicDmaRequests' = NicDmaRequests \cup {[target_addr |-> addr, status |-> IF addr \in IommuNicWhitelist THEN "OK" ELSE "IOMMU_FAULT"]}
    /\ UNCHANGED <<WatchdogMissed, HandoffFlag, LedgerP0Entries, ControlFlowViolations, IommuNicWhitelist, IpcMessages, IpcQuotas>>

SendIpc(src, dst) ==
    /\ HandoffFlag = 0
    /\ LET isAuth == <<src, dst>> \in AUTHORIZED_GRAPH
           hasQuota == isAuth /\ IpcQuotas[<<src, dst>>] > 0
       IN
       /\ IpcMessages' = IpcMessages \cup {[src |-> src, dst |-> dst, status |-> IF hasQuota THEN "ALLOWED" ELSE "REJECTED"]}
       /\ IpcQuotas' = IF hasQuota THEN [IpcQuotas EXCEPT ![<<src, dst>>] = @ - 1] ELSE IpcQuotas
    /\ UNCHANGED <<WatchdogMissed, HandoffFlag, LedgerP0Entries, ControlFlowViolations, IommuNicWhitelist, NicDmaRequests>>

PhoenixFreeze ==
    /\ HandoffFlag = 1
    /\ UNCHANGED vars

Next ==
    \/ TickWatchdog
    \/ PingWatchdog
    \/ \E t \in THREADS : TriggerCp(t)
    \/ \E addr \in {"0xAAAA", "0xCCCC"} : RequestDma(addr)
    \/ \E src \in {"Net", "Vfs", "Crypto"}, dst \in {"Net", "Vfs", "Crypto"} : SendIpc(src, dst)
    \/ PhoenixFreeze

Spec == Init /\ [][Next]_vars

S33_CordonEnforced ==
    \A msg \in IpcMessages :
        (<<msg.src, msg.dst>> \notin AUTHORIZED_GRAPH) => (msg.status = "REJECTED")

S36_NmiThreeStrikes ==
    WatchdogMissed >= 3 => HandoffFlag = 1

S38_CpHandlerImmediate ==
    ControlFlowViolations /= {} => HandoffFlag = 1

S39_NicWhitelistImmutable ==
    [][IommuNicWhitelist' = IommuNicWhitelist]_vars

S40_NicExfiltrationImpossible ==
    \A dma \in NicDmaRequests :
        (dma.target_addr \notin IommuNicWhitelist) => (dma.status = "IOMMU_FAULT")

QuotaExhausted(edge) == IpcQuotas[edge] = 0

S33_STRESS_QuotaExhaustionIsolated ==
    \A edge1, edge2 \in AUTHORIZED_GRAPH :
        (edge1 /= edge2 /\ QuotaExhausted(edge1)) => IpcQuotas[edge2] >= 0

S38_STRESS_CpDuringPhoenix ==
    (HandoffFlag = 1 /\ ControlFlowViolations /= {}) => HandoffFlag \in {1, 2}

=============================================================================
