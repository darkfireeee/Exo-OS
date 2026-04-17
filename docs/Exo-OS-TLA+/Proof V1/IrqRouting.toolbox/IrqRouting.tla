--------------------------- MODULE IrqRouting ---------------------------
EXTENDS Naturals, FiniteSets

CONSTANTS 
    IRQS, 
    PIDS, 
    MAX_HANDLERS_PER_IRQ, 
    MAX_PENDING_ACKS,     
    MAX_OVERFLOWS,        
    MAX_GEN               

VARIABLES 
    PendingAcks,     
    MaskedSince,     
    OverflowCount,   
    HandledCount,    
    DispatchGen,     
    HandlersByIrq,   
    AlivePids,       
    BlacklistedIrqs, 
    LapicIsrBit,     
    EoiSent          

vars == <<PendingAcks, MaskedSince, OverflowCount, HandledCount, DispatchGen, HandlersByIrq, AlivePids, BlacklistedIrqs, LapicIsrBit, EoiSent>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ PendingAcks     = [i \in IRQS |-> 0]
    /\ MaskedSince     = [i \in IRQS |-> 0]
    /\ OverflowCount   = [i \in IRQS |-> 0]
    /\ HandledCount    = [i \in IRQS |-> 0]
    /\ DispatchGen     = [i \in IRQS |-> 0]
    /\ HandlersByIrq   = [i \in IRQS |-> {}]
    /\ AlivePids       = PIDS
    /\ BlacklistedIrqs = {}
    /\ LapicIsrBit     = [i \in IRQS |-> FALSE]
    /\ EoiSent         = {}

--------------------------------------------------------------
\* ACTIONS
--------------------------------------------------------------
IrqArrives(irq) ==
    /\ irq \notin BlacklistedIrqs
    /\ LapicIsrBit[irq] = FALSE 
    /\ LapicIsrBit' = [LapicIsrBit EXCEPT ![irq] = TRUE]
    /\ UNCHANGED <<PendingAcks, MaskedSince, OverflowCount, HandledCount, DispatchGen, HandlersByIrq, AlivePids, BlacklistedIrqs, EoiSent>>

DispatchIrq(irq) ==
    /\ LapicIsrBit[irq] = TRUE
    /\ PendingAcks[irq] < MAX_PENDING_ACKS 
    /\ PendingAcks' = [PendingAcks EXCEPT ![irq] = @ + 1]
    /\ MaskedSince' = [MaskedSince EXCEPT ![irq] = IF @ = 0 THEN 1 ELSE @]
    /\ EoiSent' = EoiSent \union {irq}
    /\ LapicIsrBit' = [LapicIsrBit EXCEPT ![irq] = FALSE]
    /\ UNCHANGED <<OverflowCount, HandledCount, DispatchGen, HandlersByIrq, AlivePids, BlacklistedIrqs>>

AckIrq(irq) ==
    /\ PendingAcks[irq] > 0
    /\ LET new_acks == PendingAcks[irq] - 1 IN
       /\ PendingAcks' = [PendingAcks EXCEPT ![irq] = new_acks]
       /\ IF new_acks = 0 THEN
             /\ MaskedSince' = [MaskedSince EXCEPT ![irq] = 0]
             /\ HandledCount' = [HandledCount EXCEPT ![irq] = 0]
          ELSE
             /\ UNCHANGED <<MaskedSince, HandledCount>>
    /\ UNCHANGED <<OverflowCount, DispatchGen, HandlersByIrq, AlivePids, BlacklistedIrqs, LapicIsrBit, EoiSent>>

WatchdogReset(irq) ==
    /\ MaskedSince[irq] = 1 
    /\ DispatchGen[irq] < MAX_GEN 
    /\ DispatchGen' = [DispatchGen EXCEPT ![irq] = @ + 1]
    /\ PendingAcks' = [PendingAcks EXCEPT ![irq] = 0]
    /\ MaskedSince' = [MaskedSince EXCEPT ![irq] = 0]
    /\ HandledCount' = [HandledCount EXCEPT ![irq] = 0]
    /\ UNCHANGED <<OverflowCount, HandlersByIrq, AlivePids, BlacklistedIrqs, LapicIsrBit, EoiSent>>

SysIrqRegister(irq, pid) ==
    /\ pid \in AlivePids
    /\ LET valid_handlers == {h \in HandlersByIrq[irq] : h \in AlivePids} IN
       /\ Cardinality(valid_handlers) < MAX_HANDLERS_PER_IRQ
       /\ HandlersByIrq' = [HandlersByIrq EXCEPT ![irq] = valid_handlers \union {pid}]
    /\ UNCHANGED <<PendingAcks, MaskedSince, OverflowCount, HandledCount, DispatchGen, AlivePids, BlacklistedIrqs, LapicIsrBit, EoiSent>>

ProcessDies(pid) ==
    /\ pid \in AlivePids
    /\ AlivePids' = AlivePids \ {pid}
    /\ UNCHANGED <<PendingAcks, MaskedSince, OverflowCount, HandledCount, DispatchGen, HandlersByIrq, BlacklistedIrqs, LapicIsrBit, EoiSent>>

--------------------------------------------------------------
\* SYSTEM SPECIFICATION
--------------------------------------------------------------
Next == 
    \/ \E irq \in IRQS : IrqArrives(irq) \/ DispatchIrq(irq) \/ AckIrq(irq) \/ WatchdogReset(irq)
    \/ \E irq \in IRQS, pid \in PIDS : SysIrqRegister(irq, pid)
    \/ \E pid \in PIDS : ProcessDies(pid)

\* Tell TLC that the CPU must eventually process EVERY irq that is pending
Fairness ==
    /\ \A irq \in IRQS : WF_vars(DispatchIrq(irq))
    /\ \A irq \in IRQS : WF_vars(WatchdogReset(irq))

Spec == Init /\ [][Next]_vars /\ Fairness

--------------------------------------------------------------
\* PROPERTIES & CONSTRAINTS
--------------------------------------------------------------
S9_EoiAlwaysSentIfIsrBit == 
    [] \A irq \in IRQS : LapicIsrBit[irq] => <> (irq \in EoiSent)

S12_NoPendingAcksUnderflow == 
    \A irq \in IRQS : PendingAcks[irq] >= 0 /\ PendingAcks[irq] <= MAX_PENDING_ACKS

S14_NoOrphanHandlers == 
    [][ \A irq \in IRQS, pid \in PIDS : 
        SysIrqRegister(irq, pid) => 
        (HandlersByIrq[irq]' \subseteq AlivePids) ]_vars

MaxStateLimits == 
    /\ \A irq \in IRQS : OverflowCount[irq] <= MAX_OVERFLOWS
    /\ \A irq \in IRQS : DispatchGen[irq] <= MAX_GEN

=============================================================================