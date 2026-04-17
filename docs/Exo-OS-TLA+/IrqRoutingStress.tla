----------------------- MODULE IrqRoutingStress -----------------------
EXTENDS Naturals, FiniteSets

CONSTANTS 
    IRQS, 
    PIDS, 
    CORES,                
    MAX_PENDING_ACKS,     
    MAX_OVERFLOWS,        
    MAX_GEN               

VARIABLES 
    PendingAcks,     
    MaskedSince,     
    OverflowCount,   
    DispatchGen,     
    BlacklistedIrqs, 
    LapicIsrBit,     
    EoiSent          

vars == <<PendingAcks, MaskedSince, OverflowCount, DispatchGen, BlacklistedIrqs, LapicIsrBit, EoiSent>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ PendingAcks     = [i \in IRQS |-> [g \in 0..MAX_GEN |-> 0]]
    /\ MaskedSince     = [i \in IRQS |-> 0]
    /\ OverflowCount   = [i \in IRQS |-> 0]
    /\ DispatchGen     = [i \in IRQS |-> 0]
    /\ BlacklistedIrqs = {}
    /\ LapicIsrBit     = [c \in CORES |-> [i \in IRQS |-> FALSE]]
    /\ EoiSent         = [c \in CORES |-> {}]

--------------------------------------------------------------
\* ACTIONS
--------------------------------------------------------------
IrqArrives(c, irq) ==
    /\ irq \notin BlacklistedIrqs
    /\ LapicIsrBit[c][irq] = FALSE 
    /\ LapicIsrBit' = [LapicIsrBit EXCEPT ![c][irq] = TRUE]
    /\ UNCHANGED <<PendingAcks, MaskedSince, OverflowCount, DispatchGen, BlacklistedIrqs, EoiSent>>

DispatchIrq(c, irq) ==
    /\ LapicIsrBit[c][irq] = TRUE
    /\ EoiSent' = [EoiSent EXCEPT ![c] = @ \union {irq}]
    /\ LapicIsrBit' = [LapicIsrBit EXCEPT ![c][irq] = FALSE]
    /\ IF irq \in BlacklistedIrqs THEN
           UNCHANGED <<PendingAcks, MaskedSince, OverflowCount, DispatchGen, BlacklistedIrqs>>
       ELSE
           LET gen == DispatchGen[irq]
               acks == PendingAcks[irq][gen]
           IN IF acks < MAX_PENDING_ACKS THEN
                  /\ PendingAcks' = [PendingAcks EXCEPT ![irq][gen] = acks + 1]
                  /\ MaskedSince' = [MaskedSince EXCEPT ![irq] = 1]
                  /\ UNCHANGED <<OverflowCount, DispatchGen, BlacklistedIrqs>>
              ELSE
                  /\ OverflowCount' = [OverflowCount EXCEPT ![irq] = @ + 1]
                  /\ IF OverflowCount'[irq] >= MAX_OVERFLOWS THEN
                         BlacklistedIrqs' = BlacklistedIrqs \union {irq}
                     ELSE
                         UNCHANGED BlacklistedIrqs
                  /\ UNCHANGED <<PendingAcks, MaskedSince, DispatchGen>>

AckIrq(irq, gen) ==
    /\ PendingAcks[irq][gen] > 0
    /\ LET new_acks == PendingAcks[irq][gen] - 1 IN
       /\ PendingAcks' = [PendingAcks EXCEPT ![irq][gen] = new_acks]
       /\ IF new_acks = 0 /\ gen = DispatchGen[irq] THEN
              MaskedSince' = [MaskedSince EXCEPT ![irq] = 0]
          ELSE
              UNCHANGED MaskedSince
    /\ UNCHANGED <<OverflowCount, DispatchGen, BlacklistedIrqs, LapicIsrBit, EoiSent>>

WatchdogReset(irq) ==
    /\ MaskedSince[irq] = 1 
    /\ DispatchGen[irq] < MAX_GEN 
    /\ DispatchGen' = [DispatchGen EXCEPT ![irq] = @ + 1]
    /\ MaskedSince' = [MaskedSince EXCEPT ![irq] = 0]
    /\ UNCHANGED <<PendingAcks, OverflowCount, BlacklistedIrqs, LapicIsrBit, EoiSent>>

--------------------------------------------------------------
\* SYSTEM SPECIFICATION
--------------------------------------------------------------
Next == 
    \/ \E c \in CORES, irq \in IRQS : IrqArrives(c, irq) \/ DispatchIrq(c, irq)
    \/ \E irq \in IRQS, gen \in 0..MAX_GEN : AckIrq(irq, gen)
    \/ \E irq \in IRQS : WatchdogReset(irq)

Fairness ==
    /\ \A c \in CORES, irq \in IRQS : WF_vars(DispatchIrq(c, irq))
    /\ \A irq \in IRQS : WF_vars(WatchdogReset(irq))

Spec == Init /\ [][Next]_vars /\ Fairness

--------------------------------------------------------------
\* STRESS PROPERTIES
--------------------------------------------------------------
S9_EoiAlwaysSentIfIsrBit == 
    [] \A c \in CORES, irq \in IRQS : LapicIsrBit[c][irq] => <> (irq \in EoiSent[c])

S13_MaskedSinceCasVisibility == 
    [] \A irq \in IRQS : MaskedSince[irq] \in {0, 1}

S9_STRESS_SimultaneousLevelStorm == 
    [] \A irq \in IRQS : OverflowCount[irq] <= MAX_OVERFLOWS

S10_STRESS_WatchdogResetDuringStorm ==
    [][ \A irq \in IRQS, gen \in 0..MAX_GEN : 
        (AckIrq(irq, gen) /\ gen < DispatchGen[irq]) => (MaskedSince[irq]' = MaskedSince[irq])
    ]_vars

MaxStateLimits == 
    /\ \A irq \in IRQS : OverflowCount[irq] <= MAX_OVERFLOWS
    /\ \A irq \in IRQS : DispatchGen[irq] <= MAX_GEN

=============================================================================
