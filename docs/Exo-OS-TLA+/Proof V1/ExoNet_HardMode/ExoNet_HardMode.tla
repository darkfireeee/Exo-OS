------------------------- MODULE ExoNet_HardMode -------------------------
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS 
    POOL_SIZE, 
    RING_SIZE

VARIABLES 
    rx_submitted, 
    ipc_rx_ring, 
    ipc_tx_ring, 
    ns_released_buf,
    dropped_count

vars == << rx_submitted, ipc_rx_ring, ipc_tx_ring, ns_released_buf, dropped_count >>

-------------------------------------------------------------------------
\* INIT STATE
Init == 
    /\ rx_submitted = [i \in 0..(POOL_SIZE-1) |-> TRUE]
    /\ ipc_rx_ring = <<>>
    /\ ipc_tx_ring = <<>>
    /\ ns_released_buf = {}
    /\ dropped_count = 0

-------------------------------------------------------------------------
\* VIRTIO_NET ACTIONS

VirtioNet_HandleIRQ(pool_idx) ==
    /\ rx_submitted[pool_idx] = TRUE
    /\ IF Len(ipc_rx_ring) < RING_SIZE THEN
           \* Normal Path: Ring has space. Push packet to IPC.
           /\ rx_submitted' = [rx_submitted EXCEPT ![pool_idx] = FALSE]
           /\ ipc_rx_ring' = Append(ipc_rx_ring, pool_idx)
           /\ dropped_count' = dropped_count
       ELSE
           \* DDOS PATH: Ring is full! We drop the packet.
           \* To prevent Infinite State Space, we stop counting at 3.
           /\ rx_submitted' = rx_submitted
           /\ ipc_rx_ring' = ipc_rx_ring
           /\ dropped_count' = IF dropped_count < 3 THEN dropped_count + 1 ELSE dropped_count
    \* Explicitly declare untouched variables
    /\ ipc_tx_ring' = ipc_tx_ring
    /\ ns_released_buf' = ns_released_buf

VirtioNet_ProcessReleases ==
    /\ Len(ipc_tx_ring) > 0
    /\ LET release_msg == Head(ipc_tx_ring) IN
       /\ rx_submitted' = [i \in 0..(POOL_SIZE-1) |-> 
                              IF i \in release_msg THEN TRUE ELSE rx_submitted[i]]
       /\ ipc_tx_ring' = Tail(ipc_tx_ring)
    \* Explicitly declare untouched variables
    /\ ipc_rx_ring' = ipc_rx_ring
    /\ ns_released_buf' = ns_released_buf
    /\ dropped_count' = dropped_count

-------------------------------------------------------------------------
\* NETWORK_SERVER ACTIONS

NetworkServer_PollOne ==
    /\ Len(ipc_rx_ring) > 0
    /\ LET pool_idx == Head(ipc_rx_ring) IN
       /\ ns_released_buf' = ns_released_buf \union {pool_idx}
       /\ ipc_rx_ring' = Tail(ipc_rx_ring)
    \* Explicitly declare untouched variables
    /\ rx_submitted' = rx_submitted
    /\ ipc_tx_ring' = ipc_tx_ring
    /\ dropped_count' = dropped_count

NetworkServer_FlushReleased ==
    /\ ns_released_buf /= {}
    /\ Len(ipc_tx_ring) < RING_SIZE
    /\ ipc_tx_ring' = Append(ipc_tx_ring, ns_released_buf) 
    /\ ns_released_buf' = {}
    \* Explicitly declare untouched variables
    /\ rx_submitted' = rx_submitted
    /\ ipc_rx_ring' = ipc_rx_ring
    /\ dropped_count' = dropped_count

-------------------------------------------------------------------------
\* STATE TRANSITIONS
Next == 
    \/ (\E idx \in 0..(POOL_SIZE-1) : VirtioNet_HandleIRQ(idx))
    \/ VirtioNet_ProcessReleases
    \/ NetworkServer_PollOne
    \/ NetworkServer_FlushReleased

-------------------------------------------------------------------------
\* MATHEMATICAL PROOFS (INVARIANTS)

TypeOK ==
    /\ DOMAIN rx_submitted = 0..(POOL_SIZE-1)
    /\ ns_released_buf \subseteq 0..(POOL_SIZE-1)
    /\ Len(ipc_rx_ring) <= RING_SIZE
    /\ Len(ipc_tx_ring) <= RING_SIZE
    /\ dropped_count <= 3

Safety_NoDoubleRxUse ==
    \A idx \in 0..(POOL_SIZE-1) :
        (rx_submitted[idx] = TRUE) => 
            /\ ~(idx \in ns_released_buf)
            /\ \A i \in 1..Len(ipc_rx_ring) : ipc_rx_ring[i] /= idx
            /\ \A j \in 1..Len(ipc_tx_ring) : ~(idx \in ipc_tx_ring[j])

Safety_NoBufferLeak ==
    \A idx \in 0..(POOL_SIZE-1) :
        (rx_submitted[idx] = TRUE) \/
        (\E i \in 1..Len(ipc_rx_ring) : ipc_rx_ring[i] = idx) \/
        (idx \in ns_released_buf) \/
        (\E j \in 1..Len(ipc_tx_ring) : idx \in ipc_tx_ring[j])

Spec == Init /\ [][Next]_vars
=========================================================================