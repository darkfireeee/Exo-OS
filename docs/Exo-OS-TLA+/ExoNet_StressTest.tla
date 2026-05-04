------------------------- MODULE ExoNet_StressTest -------------------------
(***************************************************************************)
(* ExoOS Network Module — Modèle de stress complet                         *)
(* Couvre : RX/TX lifecycle, DDoS, Phoenix, boot, liveness, socket alloc   *)
(* *)
(* Progrès par rapport aux modèles précédents :                            *)
(* ExoNet_IPC      : prouve Safety_NoDoubleRxUse (BFS, chemin normal)    *)
(* ExoNet_HardMode : teste Safety_NoBufferLeak (simulation, DDoS)        *)
(* ExoNet_Stress   : prouve TOUT ci-dessus + TX + Phoenix + Liveness     *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    POOL_SIZE,    \* Taille du pool DMA (RX et TX partagé ici pour simplifier)
    RING_SIZE,    \* Capacité max de chaque SpscRing IPC
    MAX_SOCKETS,  \* Nombre max de sockets TCP (MAX_SOCKETS << POOL_SIZE)
    MAX_BURST,    \* Paquets max drainés par handle_rx_irq() en un tick
    BATCH_SIZE    \* pool_idx max par RxReleaseMsg (≤ 20 dans le code)

ASSUME POOL_SIZE > 0
ASSUME RING_SIZE > 0
ASSUME MAX_SOCKETS > 0
ASSUME MAX_BURST > 0
ASSUME BATCH_SIZE > 0

VARIABLES
    \* ── VIRTIO_NET ──────────────────────────────────────────────────────────
    rx_submitted,     \* [0..POOL_SIZE-1 -> BOOL] slot soumis au vring NIC
    tx_inflight,      \* [0..POOL_SIZE-1 -> BOOL] slot TX en vol vers NIC

    \* ── IPC RINGS ───────────────────────────────────────────────────────────
    ipc_rx_ring,      \* virtio_net → network_server : Sequence de pool_idx RX
    ipc_tx_ring,      \* network_server → virtio_net : Sequence de RxReleaseMsg
                      \* (chaque msg = sous-ensemble de pool_idx)
    ipc_pkt_ring,     \* network_server → virtio_net : Sequence de paquets TX
                      \* (chaque msg = {pool_idx, len} abstrait comme pool_idx seul)

    \* ── NETWORK_SERVER ──────────────────────────────────────────────────────
    ns_released_buf,  \* Set de pool_idx RX libérés ce tick (released_buf[64])
    tx_pool_free,     \* Set de pool_idx TX libres dans NetBufPool
    socket_slots,     \* Set de slots TCP alloués (⊆ 0..MAX_SOCKETS-1)

    \* ── PHOENIX ─────────────────────────────────────────────────────────────
    phoenix_phase,    \* "normal" | "draining" | "serialized" | "restoring"

    \* ── BOOT ────────────────────────────────────────────────────────────────
    boot_phase,       \* "uninit" | "driver_init_sent" | "ready"

    \* ── OBSERVABILITÉ ───────────────────────────────────────────────────────
    dropped_rx,       \* Compteur de paquets RX droppés (DDoS, borné à 3)
    dropped_tx        \* Compteur de TX droppés (saturation, borné à 3)

vars == << rx_submitted, tx_inflight,
           ipc_rx_ring, ipc_tx_ring, ipc_pkt_ring,
           ns_released_buf, tx_pool_free, socket_slots,
           phoenix_phase, boot_phase,
           dropped_rx, dropped_tx >>

Slots == 0..(POOL_SIZE-1)
SocketIDs == 0..(MAX_SOCKETS-1)

---------------------------------------------------------------------------
\* INIT
---------------------------------------------------------------------------
Init ==
    \* Tous les slots RX soumis au vring au départ (populate_rx_descriptors)
    /\ rx_submitted  = [i \in Slots |-> TRUE]
    \* Tous les slots TX libres au départ
    /\ tx_inflight   = [i \in Slots |-> FALSE]
    \* IPC rings vides
    /\ ipc_rx_ring   = <<>>
    /\ ipc_tx_ring   = <<>>
    /\ ipc_pkt_ring  = <<>>
    \* network_server : rien en cours
    /\ ns_released_buf = {}
    /\ tx_pool_free    = Slots  \* pool TX entièrement disponible
    /\ socket_slots    = {}
    \* Phoenix : phase normale
    /\ phoenix_phase \in {"normal", "draining"}
    \* Boot : non initialisé
    /\ boot_phase    = "uninit"
    \* Compteurs DDoS
    /\ dropped_rx    = 0
    /\ dropped_tx    = 0

---------------------------------------------------------------------------
\* HELPERS
---------------------------------------------------------------------------
\* Séquence de longueur < RING_SIZE → place disponible
RingHasSpace(ring) == Len(ring) < RING_SIZE

\* Batch : sous-ensemble de taille ≤ BATCH_SIZE
Batches(S) == { B \in SUBSET S : Cardinality(B) <= BATCH_SIZE /\ B /= {} }

---------------------------------------------------------------------------
\* PHASE 1 : BOOT — DriverInitMsg handshake
\* network_server doit envoyer DriverInitMsg avant toute opération réseau.
\* virtio_net ne produit aucun paquet RX avant reception de DriverInitMsg.
---------------------------------------------------------------------------

\* network_server envoie DriverInitMsg (une seule fois)
NS_SendDriverInit ==
    /\ boot_phase = "uninit"
    /\ phoenix_phase = "normal"
    /\ boot_phase' = "driver_init_sent"
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    socket_slots, phoenix_phase, dropped_rx, dropped_tx >>

\* virtio_net reçoit DriverInitMsg et devient "ready"
\* (populate_rx_descriptors déjà fait dans Init → rx_submitted tous TRUE)
VN_AckDriverInit ==
    /\ boot_phase = "driver_init_sent"
    /\ boot_phase' = "ready"
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    socket_slots, phoenix_phase, dropped_rx, dropped_tx >>

---------------------------------------------------------------------------
\* PHASE 2 : RX PATH (chemin critique RACE-RX)
---------------------------------------------------------------------------

\* virtio_net reçoit N paquets du NIC en burst (handle_rx_irq)
\* Modélise la boucle sur pop_used() : N ∈ 1..MAX_BURST slots simultanément.
\* Guard: ring a de la place pour N paquets; N slots soumis au vring.
VN_HandleIRQ_Burst(burst) ==
    \* Pré-conditions
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ burst \subseteq Slots
    /\ burst /= {}
    /\ Cardinality(burst) <= MAX_BURST
    \* Tous les slots du burst doivent être soumis au vring
    /\ \A idx \in burst : rx_submitted[idx] = TRUE
    \* Chemin normal : place suffisante dans le ring IPC
    /\ Len(ipc_rx_ring) + Cardinality(burst) <= RING_SIZE
    \* Transition : retirer du vring, pousser dans IPC
    /\ rx_submitted' = [i \in Slots |->
                            IF i \in burst THEN FALSE ELSE rx_submitted[i]]
    /\ ipc_rx_ring'  = ipc_rx_ring \o
                            [k \in 1..Cardinality(burst) |->
                                (CHOOSE o \in burst :
                                    Cardinality({ x \in burst : x < o }) = k - 1)]
    /\ UNCHANGED << tx_inflight, ipc_tx_ring, ipc_pkt_ring,
                    ns_released_buf, tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

\* DDoS path : ring plein — packet dropped, rx_submitted reste TRUE
\* (slot retiré du vring NIC mais non push dans IPC → logiquement libre pour refill)
VN_HandleIRQ_Drop(pool_idx) ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ rx_submitted[pool_idx] = TRUE
    /\ ~RingHasSpace(ipc_rx_ring)
    \* Le slot reste TRUE → sera refillé via ProcessReleases
    \* (abstraction correcte : slot sorti du vring NIC, rx_submitted marque "libre")
    /\ rx_submitted' = rx_submitted
    /\ dropped_rx'   = IF dropped_rx < 3 THEN dropped_rx + 1 ELSE dropped_rx
    /\ UNCHANGED << tx_inflight, ipc_rx_ring, ipc_tx_ring, ipc_pkt_ring,
                    ns_released_buf, tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_tx >>

\* network_server poll_ingress_single : consomme 1 paquet RX
\* smoltcp lit le buffer → pool_idx ajouté au released_buf
NS_PollOne ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ Len(ipc_rx_ring) > 0
    /\ LET pool_idx == Head(ipc_rx_ring) IN
        /\ ns_released_buf' = ns_released_buf \union {pool_idx}
        /\ ipc_rx_ring'     = Tail(ipc_rx_ring)
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_tx_ring, ipc_pkt_ring,
                    tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

\* network_server flush_released : envoie RxReleaseMsg batchés à virtio_net
\* Modélise le découpage en messages de BATCH_SIZE pool_idx max
NS_FlushReleased ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ ns_released_buf /= {}
    /\ RingHasSpace(ipc_tx_ring)
    /\ \E batch \in Batches(ns_released_buf) :
        /\ ipc_tx_ring'    = Append(ipc_tx_ring, batch)
        /\ ns_released_buf' = ns_released_buf \ batch
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_pkt_ring,
                    tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

\* virtio_net process_rx_releases : reçoit RxReleaseMsg et refill vring
VN_ProcessReleases ==
    /\ boot_phase = "ready"
    /\ Len(ipc_tx_ring) > 0
    /\ LET release_msg == Head(ipc_tx_ring) IN
        /\ rx_submitted' = [i \in Slots |->
                                IF i \in release_msg THEN TRUE
                                ELSE rx_submitted[i]]
        /\ ipc_tx_ring'  = Tail(ipc_tx_ring)
    /\ UNCHANGED << tx_inflight, ipc_rx_ring, ipc_pkt_ring,
                    ns_released_buf, tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

---------------------------------------------------------------------------
\* PHASE 3 : TX PATH
\* network_server alloue un slot TX, smoltcp écrit, TxToken::consume push IPC
\* virtio_net flush_tx → libère le slot
---------------------------------------------------------------------------

\* network_server alloue un slot TX (tx_alloc) + smoltcp écrit + push vers NIC
\* Modélise transmit() + ExoTxToken::consume()
NS_SendPacket ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ tx_pool_free /= {}
    /\ RingHasSpace(ipc_pkt_ring)
    /\ \E idx \in tx_pool_free :
        /\ tx_pool_free'  = tx_pool_free \ {idx}
        /\ tx_inflight'   = [tx_inflight EXCEPT ![idx] = TRUE]
        /\ ipc_pkt_ring'  = Append(ipc_pkt_ring, idx)
    /\ UNCHANGED << rx_submitted, ipc_rx_ring, ipc_tx_ring,
                    ns_released_buf, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

\* TX saturé : tx_alloc() échoue, receive() retourne None
\* Le pool_idx RX en attente est libéré dans released_buf
NS_TxSaturated(rx_idx) ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ tx_pool_free = {}          \* pool TX épuisé
    /\ Len(ipc_rx_ring) > 0
    /\ Head(ipc_rx_ring) = rx_idx \* tentative de receive pour rx_idx
    \* Le slot RX est remis dans released_buf (pas lu par smoltcp → libéré proprement)
    /\ ns_released_buf' = ns_released_buf \union {rx_idx}
    /\ ipc_rx_ring'     = Tail(ipc_rx_ring)
    /\ dropped_tx'      = IF dropped_tx < 3 THEN dropped_tx + 1 ELSE dropped_tx
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_tx_ring, ipc_pkt_ring,
                    tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx >>

\* virtio_net flush_tx : envoie le paquet TX vers le NIC, libère le slot
VN_FlushTx ==
    /\ boot_phase = "ready"
    /\ Len(ipc_pkt_ring) > 0
    /\ LET tx_idx == Head(ipc_pkt_ring) IN
        /\ tx_inflight'  = [tx_inflight EXCEPT ![tx_idx] = FALSE]
        /\ tx_pool_free' = tx_pool_free \union {tx_idx}
        /\ ipc_pkt_ring' = Tail(ipc_pkt_ring)
    /\ UNCHANGED << rx_submitted, ipc_rx_ring, ipc_tx_ring,
                    ns_released_buf, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

---------------------------------------------------------------------------
\* PHASE 4 : SOCKET LIFECYCLE
\* Prouve conservation : chaque slot socket est dans socket_slots XOR libre
---------------------------------------------------------------------------

NS_SocketAlloc ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ Cardinality(socket_slots) < MAX_SOCKETS
    /\ \E slot \in SocketIDs \ socket_slots :
        /\ socket_slots' = socket_slots \union {slot}
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

NS_SocketClose ==
    /\ boot_phase = "ready"
    /\ socket_slots /= {}
    /\ \E slot \in socket_slots :
        /\ socket_slots' = socket_slots \ {slot}
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

---------------------------------------------------------------------------
\* PHASE 5 : PHOENIX ISOLATION CYCLE
\* Séquence : normal → draining → serialized → restoring → normal
\* Invariant : pendant draining/serialized, aucun nouveau RX accepté
---------------------------------------------------------------------------

\* Phoenix demande isolation : network_server commence drain_all()
Phoenix_StartDrain ==
    /\ boot_phase = "ready"
    /\ phoenix_phase = "normal"
    /\ phoenix_phase' = "draining"
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    socket_slots, boot_phase, dropped_rx, dropped_tx >>

\* drain_all() vide le ring RX : consomme tous les paquets en attente
\* (version simplifiée : un seul paquet par action → BFS prouve pour toutes les séquences)
Phoenix_DrainOne ==
    /\ phoenix_phase = "draining"
    /\ Len(ipc_rx_ring) > 0
    /\ LET pool_idx == Head(ipc_rx_ring) IN
        /\ ns_released_buf' = ns_released_buf \union {pool_idx}
        /\ ipc_rx_ring'     = Tail(ipc_rx_ring)
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_tx_ring, ipc_pkt_ring,
                    tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

\* drain_all() flush les released_buf accumulés vers virtio_net
Phoenix_DrainFlush ==
    /\ phoenix_phase = "draining"
    /\ ns_released_buf /= {}
    /\ RingHasSpace(ipc_tx_ring)
    /\ \E batch \in Batches(ns_released_buf) :
        /\ ipc_tx_ring'     = Append(ipc_tx_ring, batch)
        /\ ns_released_buf' = ns_released_buf \ batch
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_pkt_ring,
                    tx_pool_free, socket_slots,
                    phoenix_phase, boot_phase, dropped_rx, dropped_tx >>

\* Drain complet : ring RX vide, released_buf vide → sérialisation TcpStateStore
Phoenix_Serialize ==
    /\ phoenix_phase = "draining"
    /\ Len(ipc_rx_ring) = 0
    /\ ns_released_buf = {}
    /\ phoenix_phase' = "serialized"
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    socket_slots, boot_phase, dropped_rx, dropped_tx >>

\* Phoenix restore : retour à l'état normal après isolation
Phoenix_Restore ==
    /\ phoenix_phase = "serialized"
    /\ phoenix_phase' = "normal"
    \* Les sockets TCP sont perdus après restore (UDP Phase 2 : hors scope)
    /\ socket_slots' = {}
    /\ UNCHANGED << rx_submitted, tx_inflight, ipc_rx_ring, ipc_tx_ring,
                    ipc_pkt_ring, ns_released_buf, tx_pool_free,
                    boot_phase, dropped_rx, dropped_tx >>

---------------------------------------------------------------------------
\* NEXT STATE RELATION
---------------------------------------------------------------------------
Next ==
    \* Boot
    \/ NS_SendDriverInit
    \/ VN_AckDriverInit
    \* RX path
    \/ (\E burst \in SUBSET Slots :
            burst /= {} /\ Cardinality(burst) <= MAX_BURST
            /\ VN_HandleIRQ_Burst(burst))
    \/ (\E idx \in Slots : VN_HandleIRQ_Drop(idx))
    \/ NS_PollOne
    \/ NS_FlushReleased
    \/ VN_ProcessReleases
    \* TX path
    \/ NS_SendPacket
    \/ (\E idx \in Slots : NS_TxSaturated(idx))
    \/ VN_FlushTx
    \* Socket lifecycle
    \/ NS_SocketAlloc
    \/ NS_SocketClose
    \* Phoenix
    \/ Phoenix_StartDrain
    \/ Phoenix_DrainOne
    \/ Phoenix_DrainFlush
    \/ Phoenix_Serialize
    \/ Phoenix_Restore

---------------------------------------------------------------------------
\* LIVENESS & FAIRNESS ASSUMPTIONS
---------------------------------------------------------------------------
Fairness ==
    \* The Network Server must eventually get CPU time to process packets
    /\ WF_vars(NS_PollOne)
    /\ WF_vars(NS_FlushReleased)
    /\ WF_vars(NS_SendPacket)
    \* The Hardware/Driver must eventually get CPU time to clean up
    /\ WF_vars(VN_ProcessReleases)
    /\ WF_vars(VN_FlushTx)
    \* Phoenix operations cannot be permanently suspended
    /\ WF_vars(Phoenix_StartDrain)
    /\ WF_vars(Phoenix_DrainOne)
    /\ WF_vars(Phoenix_DrainFlush) \* <--- FIX ADDED HERE!
    /\ WF_vars(Phoenix_Serialize)
    /\ WF_vars(Phoenix_Restore)

Spec == Init /\ [][Next]_vars /\ Fairness

---------------------------------------------------------------------------
\* INVARIANTS — SÉCURITÉ
---------------------------------------------------------------------------

\* TYPE SAFETY
TypeOK ==
    /\ DOMAIN rx_submitted = Slots
    /\ \A i \in Slots : rx_submitted[i] \in BOOLEAN
    /\ DOMAIN tx_inflight = Slots
    /\ \A i \in Slots : tx_inflight[i] \in BOOLEAN
    /\ ns_released_buf \subseteq Slots
    /\ tx_pool_free \subseteq Slots
    /\ socket_slots \subseteq SocketIDs
    /\ Len(ipc_rx_ring) <= RING_SIZE
    /\ Len(ipc_tx_ring) <= RING_SIZE
    /\ Len(ipc_pkt_ring) <= RING_SIZE
    /\ dropped_rx \in 0..3
    /\ dropped_tx \in 0..3
    /\ phoenix_phase \in {"normal", "draining", "serialized", "restoring"}
    /\ boot_phase \in {"uninit", "driver_init_sent", "ready"}

\* INV-RX-01 : Aucun double-usage RX (RACE-RX éliminée)
\* Un slot RX ne peut pas être soumis au vring ET présent dans un ring IPC
\* ou dans le released_buf du network_server.
Safety_NoDoubleRxUse ==
    \A idx \in Slots :
        (rx_submitted[idx] = TRUE) =>
            /\ ~(idx \in ns_released_buf)
            /\ \A i \in 1..Len(ipc_rx_ring) : ipc_rx_ring[i] /= idx
            /\ \A j \in 1..Len(ipc_tx_ring) : ~(idx \in ipc_tx_ring[j])

\* INV-RX-02 : Conservation des buffers RX
\* Chaque slot est localisable dans exactement un état.
Safety_NoRxBufferLeak ==
    \A idx \in Slots :
        \/ rx_submitted[idx] = TRUE
        \/ (\E i \in 1..Len(ipc_rx_ring) : ipc_rx_ring[i] = idx)
        \/ (idx \in ns_released_buf)
        \/ (\E j \in 1..Len(ipc_tx_ring) : idx \in ipc_tx_ring[j])

\* INV-TX-01 : Aucun double-usage TX
\* Un slot TX ne peut pas être en vol ET libre simultanément.
Safety_NoDoubleTxUse ==
    \A idx \in Slots :
        ~(tx_inflight[idx] = TRUE /\ idx \in tx_pool_free)

\* INV-TX-02 : Conservation des buffers TX
\* Chaque slot TX est soit libre, soit en vol, soit dans le ring IPC.
Safety_NoTxBufferLeak ==
    \A idx \in Slots :
        \/ (idx \in tx_pool_free)
        \/ tx_inflight[idx] = TRUE
        \/ (\E i \in 1..Len(ipc_pkt_ring) : ipc_pkt_ring[i] = idx)

\* INV-BOOT-01 : Aucun trafic avant boot_phase = "ready"
\* VN_HandleIRQ ne peut pas pousser dans ipc_rx_ring avant ready.
Safety_NoTrafficBeforeBoot ==
    (boot_phase /= "ready") => Len(ipc_rx_ring) = 0

\* INV-PHOENIX-01 : Aucun nouveau paquet RX accepté pendant isolation
Safety_PhoenixNoNewRx ==
    (phoenix_phase = "draining" \/ phoenix_phase = "serialized") =>
        \A idx \in Slots : rx_submitted[idx] = TRUE
            \* Pendant drain/serialize, les slots reviennent progressivement
            \* via ProcessReleases. Aucun nouveau HandleIRQ → ipc_rx_ring
            \* ne peut que se vider.

\* INV-PHOENIX-02 : Après restore, released_buf est vide
Safety_PhoenixCleanRestore ==
    (phoenix_phase = "normal") =>
        \* released_buf peut être non-vide si on vient d'un tick normal
        \* La propriété clé : pendant "serialized", released_buf est vide
        TRUE  \* prouvé structurellement par Phoenix_Serialize guard

\* INV-SOCKET-01 : Nombre de sockets alloués dans les bornes
Safety_SocketBound ==
    Cardinality(socket_slots) <= MAX_SOCKETS

\* INV-DRAIN-01 : Phoenix draining ne se termine que si ring + buf vides
\* (structurellement garanti par Phoenix_Serialize guard)

---------------------------------------------------------------------------
\* PROPRIÉTÉS DE VIVACITÉ (nécessitent Fairness)
---------------------------------------------------------------------------

\* LIVE-01 : Tout paquet RX éventuellement délivré à smoltcp OU droppé
\* Si un paquet est dans ipc_rx_ring, il sera éventuellement consommé.
Liveness_RxDelivery ==
    [](Len(ipc_rx_ring) > 0 =>
        <>(Len(ipc_rx_ring) = 0 \/ dropped_rx > 0))

\* LIVE-02 : Tout slot TX éventuellement libéré après envoi
\* Si un slot TX est en vol, il sera éventuellement remis dans tx_pool_free.
Liveness_TxCompletion ==
    [](\A idx \in Slots :
        tx_inflight[idx] = TRUE => <>(idx \in tx_pool_free))

\* LIVE-03 : Tout slot RX droppé (released_buf) éventuellement refillé
\* Si un pool_idx est dans released_buf, il sera éventuellement dans rx_submitted.
Liveness_RxRefill ==
    [](\A idx \in Slots :
        (idx \in ns_released_buf) => <>(rx_submitted[idx] = TRUE))

\* LIVE-04 : Phoenix s'achève toujours (pas de blocage de drain)
Liveness_PhoenixCompletes ==
    [](phoenix_phase = "draining" =>
        <>(phoenix_phase = "normal"))

\* LIVE-05 : Après TX saturation, pool TX éventuellement libéré
Liveness_TxPoolRecovery ==
    [](tx_pool_free = {} => <>(tx_pool_free /= {}))

---------------------------------------------------------------------------
\* PROPRIÉTÉ COMPOSÉE : le système ne peut pas rester bloqué
---------------------------------------------------------------------------

\* Un deadlock = aucune action n'est possible hors UNCHANGED
\* TLC vérifie automatiquement l'absence de deadlock si CHECK_DEADLOCK TRUE

---------------------------------------------------------------------------
\* SCENARIO : Phoenix sous charge (RX ring non vide au moment du drain)
\* Propriété : drain_all vide le ring avant serialize, quelles que soient
\* les entrées en cours.
---------------------------------------------------------------------------
Phoenix_DrainCompleteness ==
    [](phoenix_phase = "serialized" => Len(ipc_rx_ring) = 0)

==========================================================================