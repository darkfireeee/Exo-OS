--------------------------- MODULE IommuQueue ---------------------------
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS 
    PRODUCERS,   \* Set of CPU Cores producing faults (e.g., {c1, c2, c3})
    CAPACITY,    \* Size of the ring buffer
    MAX_FAULTS   \* Limit total faults to prevent infinite state explosion

VARIABLES 
    QueueSlots,        \* Array of CAPACITY slots: [seq, event]
    QueueHead,         \* AtomicUsize: next position to claim (Producer)
    QueueTail,         \* AtomicUsize: next position to read (Consumer)
    QueueDropped,      \* Count of dropped faults (queue full)
    QueueInitialized,  \* BOOLEAN: init() called
    WorkerActive,      \* Ghost: is the worker currently processing?
    IommuFaults,       \* Set of all injected fault IDs
    ProcessedFaults,   \* SEQUENCE of processed fault IDs (to verify FIFO)
    
    \* Local state for producers (simulates multi-step CAS/Write/Release)
    ProdState,         \* [p -> "IDLE" or "WRITING"]
    ProdPos,           \* [p -> claimed position in the queue]
    ProdFault,         \* [p -> fault ID being written]
    GlobalFaultId      \* Monotonic ID generator for FIFO testing

vars == <<QueueSlots, QueueHead, QueueTail, QueueDropped, QueueInitialized, WorkerActive, 
          IommuFaults, ProcessedFaults, ProdState, ProdPos, ProdFault, GlobalFaultId>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ QueueSlots       = [i \in 0..CAPACITY-1 |-> [seq |-> 0, event |-> 0]] 
    /\ QueueHead        = 0
    /\ QueueTail        = 0
    /\ QueueDropped     = 0
    /\ QueueInitialized = FALSE
    /\ WorkerActive     = FALSE
    /\ IommuFaults      = {}
    /\ ProcessedFaults  = <<>>
    /\ ProdState        = [p \in PRODUCERS |-> "IDLE"]
    /\ ProdPos          = [p \in PRODUCERS |-> 0]
    /\ ProdFault        = [p \in PRODUCERS |-> 0]
    /\ GlobalFaultId    = 1

--------------------------------------------------------------
\* ACTIONS
--------------------------------------------------------------
\* Ring 0 Boot: Initialize the MPSC queue
SystemInit ==
    /\ ~QueueInitialized
    \* Critical: Each slot's seq must be initialized to its index
    /\ QueueSlots' = [i \in 0..CAPACITY-1 |-> [seq |-> i, event |-> 0]]
    /\ QueueInitialized' = TRUE
    /\ UNCHANGED <<QueueHead, QueueTail, QueueDropped, WorkerActive, IommuFaults, ProcessedFaults, ProdState, ProdPos, ProdFault, GlobalFaultId>>

\* ISR Producer: Core receives IOMMU Fault and attempts to claim a slot (FetchAndAdd)
ProducerClaim(p) ==
    /\ QueueInitialized
    /\ ProdState[p] = "IDLE"
    /\ GlobalFaultId <= MAX_FAULTS
    /\ LET fault == GlobalFaultId IN
       /\ IommuFaults' = IommuFaults \union {fault}
       /\ GlobalFaultId' = GlobalFaultId + 1
       /\ IF QueueHead - QueueTail >= CAPACITY THEN
              \* Queue is full -> explicit drop
              /\ QueueDropped' = QueueDropped + 1
              /\ UNCHANGED <<QueueHead, ProdState, ProdPos, ProdFault>>
          ELSE
              \* Atomic FetchAndAdd(QueueHead, 1) -> Claims the slot!
              /\ ProdPos' = [ProdPos EXCEPT ![p] = QueueHead]
              /\ ProdFault' = [ProdFault EXCEPT ![p] = fault]
              /\ ProdState' = [ProdState EXCEPT ![p] = "WRITING"]
              /\ QueueHead' = QueueHead + 1
              /\ UNCHANGED <<QueueDropped>>
    /\ UNCHANGED <<QueueSlots, QueueTail, QueueInitialized, WorkerActive, ProcessedFaults>>

\* ISR Producer: Core writes data and explicitly Releases the seq lock
ProducerWrite(p) ==
    /\ ProdState[p] = "WRITING"
    /\ LET pos == ProdPos[p]
           idx == pos % CAPACITY
           slot == QueueSlots[idx]
       IN 
       \* CAS Release semantics: Core can ONLY write if seq == pos
       /\ slot.seq = pos 
       /\ QueueSlots' = [QueueSlots EXCEPT ![idx] = [seq |-> pos + 1, event |-> ProdFault[p]]]
       /\ ProdState' = [ProdState EXCEPT ![p] = "IDLE"]
       /\ UNCHANGED <<QueueHead, QueueTail, QueueDropped, QueueInitialized, WorkerActive, IommuFaults, ProcessedFaults, ProdPos, ProdFault, GlobalFaultId>>

\* Worker Thread: Consumer reads from Tail
ConsumerPop ==
    /\ QueueInitialized
    /\ LET pos == QueueTail
           idx == pos % CAPACITY
           slot == QueueSlots[idx]
       IN
       \* Acquire semantics: Worker can ONLY read if seq == pos + 1
       /\ slot.seq = pos + 1 
       /\ ProcessedFaults' = Append(ProcessedFaults, slot.event)
       \* Mark slot as free for the NEXT wrap-around loop (pos + CAPACITY)
       /\ QueueSlots' = [QueueSlots EXCEPT ![idx] = [seq |-> pos + CAPACITY, event |-> 0]]
       /\ QueueTail' = QueueTail + 1
       /\ UNCHANGED <<QueueHead, QueueDropped, QueueInitialized, WorkerActive, IommuFaults, ProdState, ProdPos, ProdFault, GlobalFaultId>>

--------------------------------------------------------------
\* SYSTEM SPECIFICATION
--------------------------------------------------------------
Next == 
    \/ SystemInit
    \/ \E p \in PRODUCERS: ProducerClaim(p) \/ ProducerWrite(p)
    \/ ConsumerPop

Fairness ==
    /\ \A p \in PRODUCERS : WF_vars(ProducerWrite(p))
    /\ WF_vars(ConsumerPop)

Spec == Init /\ [][Next]_vars /\ Fairness

--------------------------------------------------------------
\* PROPERTIES TO PROVE
--------------------------------------------------------------
\* Safety S18 — Boot invariant: push() cannot be called before init()
S18_InitBeforePush == 
    \A p \in PRODUCERS : (ProdState[p] = "WRITING") => QueueInitialized

\* Safety S15 — No orphaned slots
\* Si un slot est réservé (WRITING), sa séquence correspond exactement à pos,
\* donc le consumer (qui attend pos + 1) ne peut physiquement pas le lire !
S15_NoOrphanedSlots == 
    \A p \in PRODUCERS : 
        (ProdState[p] = "WRITING") => (QueueSlots[ProdPos[p] % CAPACITY].seq = ProdPos[p])

\* Safety S16 — Strict FIFO order
\* Verify that the sequence of processed faults is strictly sorted by ID
S16_FifoOrdering == 
    \A i, j \in 1..Len(ProcessedFaults) : 
        i < j => ProcessedFaults[i] < ProcessedFaults[j]

\* Safety S17 — Correct dropped count when queue is full
\* When the storm is over and everything is processed, the math must align perfectly
S17_DroppedAccurate == 
    (GlobalFaultId > MAX_FAULTS /\ QueueHead = QueueTail) => 
        (Cardinality(IommuFaults) = Len(ProcessedFaults) + QueueDropped)

=============================================================================