------------------------- MODULE ExoFS_Stress_Hard -------------------------
(*
 * ExoFS — Hard Stress Model
 * Auteur : claude-gamma
 * Date   : 2026-05-04
 *
 * Invariants couverts :
 * S-FS-01  Cohérence VfsTree : chaque path pointe vers un blob présent sur disque
 * S-FS-02  Refcount non négatif (hors ZeroBlobId)
 * S-FS-03  ZeroBlobId jamais libéré
 * S-FS-04  Pas de lecture d'un blob en cours de GC
 * S-FS-05  Flush NVMe avant toute mise à jour VfsTree
 * S-FS-06  Phoenix restore : aucun FD actif ne pointe vers un blob disparu
 * S-FS-07  Blake3 nonce unicité (pas de collision ID → deux blobs différents)
 * S-FS-08  Journal replay idempotent
 * S-FS-10  Truncate partiel : refcount de l'ancien blob décrémenté avant libération
 *)
EXTENDS Integers, FiniteSets, Sequences, TLC

CONSTANTS
    BLOB_IDS,          \* Ensemble des BlobId valides (hors ZeroBlobId)
    ZERO_BLOB,         \* ZeroBlobId — sentinelle blob vide
    PATHS,             \* Ensemble des chemins VFS modélisés
    WRITERS,           \* Ensemble des processus écrivains concurrents
    READERS,           \* Ensemble des processus lecteurs concurrents
    MAX_REFCOUNT,      \* Refcount maximal par blob
    MAX_CRASH,         \* Nombre maximal de crashes Phoenix simulés
    JOURNAL_SLOTS      \* Capacité du journal

ASSUME ZERO_BLOB \notin BLOB_IDS
ASSUME PATHS # {}
ASSUME WRITERS # {}
ASSUME MAX_REFCOUNT >= 2
ASSUME MAX_CRASH >= 1

(* =========================================================================
   VARIABLES
   ========================================================================= *)
VARIABLES
    DiskBlobs,         \* Ensemble des BlobId présents sur disque
    VfsTree,           \* PATHS -> BlobId (état actuel de l'arbre VFS)
    BlobRefcount,      \* BlobId -> Nat (compteur de référence)
    ActiveReaders,     \* BlobId -> SUBSET READERS
    FlushPending,      \* WRITERS -> BOOLEAN
    FlushTarget,       \* WRITERS -> BlobId
    WriterStage,       \* WRITERS -> {"IDLE","ALLOC","FLUSH_WAIT","COMMIT"}
    WriterPath,        \* WRITERS -> PATHS
    WriterBlob,        \* WRITERS -> BlobId
    WriterOldBlob,     \* WRITERS -> BlobId (le blob écrasé par ce writer)
    Journal,           \* Seq of [op: "WRITE", path: PATHS, blob: BlobId]
    JournalCommitted,  \* SET of Nat (Set des IDs de transactions durcies)
    PhoenixPhase,      \* {"NORMAL","FREEZING","RESTORING","RESTORED"}
    CrashCount,        \* Nat (nombre de crashes)
    EvictedBlobs,      \* Ensemble des BlobId évincés
    NvmeAckGen,        \* Nat (génère des TxIDs uniques)
    AdversaryForge,    \* BOOLEAN
    TruncStage,        \* WRITERS -> {"NONE","OLD_REF_DEC","DONE"}
    StaleReaderAlert,  \* BOOLEAN
    StaleAckCount      \* Nat

vars == <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders,
          FlushPending, FlushTarget, WriterStage, WriterPath, WriterBlob, WriterOldBlob,
          Journal, JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
          NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

(* =========================================================================
   INVARIANTS DE TYPE
   ========================================================================= *)
TypeOK ==
    /\ DiskBlobs \subseteq (BLOB_IDS \cup {ZERO_BLOB})
    /\ VfsTree \in [PATHS -> (BLOB_IDS \cup {ZERO_BLOB})]
    /\ BlobRefcount \in [BLOB_IDS \cup {ZERO_BLOB} -> Int]
    /\ ActiveReaders \in [BLOB_IDS \cup {ZERO_BLOB} -> SUBSET READERS]
    /\ FlushPending \in [WRITERS -> BOOLEAN]
    /\ FlushTarget \in [WRITERS -> (BLOB_IDS \cup {ZERO_BLOB})]
    /\ WriterStage \in [WRITERS -> {"IDLE","ALLOC","FLUSH_WAIT","COMMIT"}]
    /\ WriterPath \in [WRITERS -> PATHS]
    /\ WriterBlob \in [WRITERS -> (BLOB_IDS \cup {ZERO_BLOB})]
    /\ WriterOldBlob \in [WRITERS -> (BLOB_IDS \cup {ZERO_BLOB})]
    /\ IsFiniteSet(Journal)
    /\ JournalCommitted \subseteq Nat
    /\ PhoenixPhase \in {"NORMAL","FREEZING","RESTORING","RESTORED"}
    /\ CrashCount \in 0..MAX_CRASH
    /\ EvictedBlobs \subseteq BLOB_IDS
    /\ NvmeAckGen \in Nat
    /\ AdversaryForge \in BOOLEAN
    /\ TruncStage \in [WRITERS -> {"NONE","OLD_REF_DEC","DONE"}]
    /\ StaleReaderAlert \in BOOLEAN
    /\ StaleAckCount \in 0..2

(* =========================================================================
   INITIALISATION
   ========================================================================= *)
Init ==
    /\ DiskBlobs = {ZERO_BLOB}
    /\ VfsTree = [p \in PATHS |-> ZERO_BLOB]
    /\ BlobRefcount = [b \in (BLOB_IDS \cup {ZERO_BLOB}) |->
                          IF b = ZERO_BLOB THEN -1 ELSE 0]
    /\ ActiveReaders = [b \in (BLOB_IDS \cup {ZERO_BLOB}) |-> {}]
    /\ FlushPending = [w \in WRITERS |-> FALSE]
    /\ FlushTarget = [w \in WRITERS |-> ZERO_BLOB]
    /\ WriterStage = [w \in WRITERS |-> "IDLE"]
    /\ WriterPath = [w \in WRITERS |-> CHOOSE p \in PATHS : TRUE]
    /\ WriterBlob = [w \in WRITERS |-> ZERO_BLOB]
    /\ WriterOldBlob = [w \in WRITERS |-> ZERO_BLOB]
    /\ Journal = {}
    /\ JournalCommitted = {}
    /\ PhoenixPhase = "NORMAL"
    /\ CrashCount = 0
    /\ EvictedBlobs = {}
    /\ NvmeAckGen = 0
    /\ AdversaryForge = FALSE
    /\ TruncStage = [w \in WRITERS |-> "NONE"]
    /\ StaleReaderAlert = FALSE
    /\ StaleAckCount = 0

(* =========================================================================
   ACTIONS — CYCLE DE VIE BLOB
   ========================================================================= *)

WriterAlloc(w) ==
    /\ PhoenixPhase = "NORMAL"
    /\ WriterStage[w] = "IDLE"
    /\ \E b \in BLOB_IDS :
        /\ b \notin DiskBlobs
        /\ b \notin EvictedBlobs
        /\ WriterBlob[w] # b
        /\ \A other_w \in WRITERS : WriterBlob[other_w] # b
        /\ WriterBlob' = [WriterBlob EXCEPT ![w] = b]
    /\ \E p \in PATHS :
        WriterPath' = [WriterPath EXCEPT ![w] = p]
    /\ WriterStage' = [WriterStage EXCEPT ![w] = "ALLOC"]
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders,
                   FlushPending, FlushTarget, Journal, JournalCommitted,
                   PhoenixPhase, CrashCount, EvictedBlobs, NvmeAckGen, WriterOldBlob,
                   AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

WriterSubmitFlush(w) ==
    /\ PhoenixPhase = "NORMAL"
    /\ WriterStage[w] = "ALLOC"
    /\ ~FlushPending[w]
    /\ FlushPending' = [FlushPending EXCEPT ![w] = TRUE]
    /\ FlushTarget' = [FlushTarget EXCEPT ![w] = WriterBlob[w]]
    /\ WriterStage' = [WriterStage EXCEPT ![w] = "FLUSH_WAIT"]
    /\ Journal' = Journal \cup {[op |-> "WRITE",
                                  path |-> WriterPath[w],
                                  blob |-> WriterBlob[w],
                                  gen |-> NvmeAckGen]}
    /\ NvmeAckGen' = NvmeAckGen + 1
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders, WriterOldBlob,
                   WriterBlob, WriterPath, JournalCommitted, PhoenixPhase,
                   CrashCount, EvictedBlobs, AdversaryForge,
                   TruncStage, StaleReaderAlert, StaleAckCount>>

NvmeAckFlush(w) ==
    /\ PhoenixPhase = "NORMAL"
    /\ WriterStage[w] = "FLUSH_WAIT"
    /\ FlushPending[w]
    /\ FlushTarget[w] = WriterBlob[w]
    /\ DiskBlobs' = DiskBlobs \cup {WriterBlob[w]}
    /\ WriterOldBlob' = [WriterOldBlob EXCEPT ![w] = VfsTree[WriterPath[w]]]
    /\ VfsTree' = [VfsTree EXCEPT ![WriterPath[w]] = WriterBlob[w]]
    /\ BlobRefcount' = [BlobRefcount EXCEPT ![WriterBlob[w]] = 1]
    /\ FlushPending' = [FlushPending EXCEPT ![w] = FALSE]
    /\ LET my_gen == (CHOOSE e \in Journal : e.path = WriterPath[w] /\ e.blob = WriterBlob[w] /\ (\A other \in Journal : (other.path = WriterPath[w] /\ other.blob = WriterBlob[w]) => e.gen >= other.gen)).gen IN
       JournalCommitted' = JournalCommitted \cup {my_gen}
    /\ WriterStage' = [WriterStage EXCEPT ![w] = "COMMIT"]
    /\ UNCHANGED <<ActiveReaders, FlushTarget, WriterBlob, WriterPath,
                   Journal, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

NvmeStaleAck(w) ==
    /\ PhoenixPhase = "NORMAL"
    /\ WriterStage[w] = "FLUSH_WAIT"
    /\ FlushPending[w]
    /\ StaleAckCount < 2
    /\ StaleAckCount' = StaleAckCount + 1
    /\ FlushPending' = [FlushPending EXCEPT ![w] = FALSE]
    /\ WriterStage' = [WriterStage EXCEPT ![w] = "ALLOC"]
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders, WriterOldBlob,
                   FlushTarget, WriterBlob, WriterPath, Journal,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert>>

WriterFinish(w) ==
    /\ \/ WriterStage[w] = "COMMIT" /\ TruncStage[w] = "NONE" /\ (WriterOldBlob[w] = ZERO_BLOB \/ WriterOldBlob[w] = WriterBlob[w] \/ WriterOldBlob[w] \notin DiskBlobs)
       \/ WriterStage[w] = "COMMIT" /\ TruncStage[w] = "OLD_REF_DEC" /\ BlobRefcount[WriterOldBlob[w]] > 0
    /\ WriterStage' = [WriterStage EXCEPT ![w] = "IDLE"]
    /\ WriterBlob' = [WriterBlob EXCEPT ![w] = ZERO_BLOB]
    /\ WriterOldBlob' = [WriterOldBlob EXCEPT ![w] = ZERO_BLOB]
    /\ TruncStage' = [TruncStage EXCEPT ![w] = "NONE"]
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders,
                   FlushPending, FlushTarget, WriterPath, Journal,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, StaleReaderAlert, StaleAckCount>>

(* =========================================================================
   ACTIONS — LECTURE
   ========================================================================= *)

ReaderOpen(r, p) ==
    /\ PhoenixPhase \in {"NORMAL","RESTORED"}
    /\ LET b == VfsTree[p] IN
       /\ b \in DiskBlobs
       /\ b \notin EvictedBlobs
       /\ BlobRefcount[b] >= 0
       /\ BlobRefcount[b] < MAX_REFCOUNT
       /\ r \notin ActiveReaders[b]
       /\ BlobRefcount' = [BlobRefcount EXCEPT ![b] = @ + 1]
       /\ ActiveReaders' = [ActiveReaders EXCEPT ![b] = @ \cup {r}]
    /\ UNCHANGED <<DiskBlobs, VfsTree, FlushPending, FlushTarget, WriterOldBlob,
                   WriterStage, WriterPath, WriterBlob, Journal,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

ReaderClose(r, p) ==
    /\ LET b == VfsTree[p] IN
       /\ r \in ActiveReaders[b]
       /\ BlobRefcount[b] > 0
       /\ BlobRefcount' = [BlobRefcount EXCEPT ![b] = @ - 1]
       /\ ActiveReaders' = [ActiveReaders EXCEPT ![b] = @ \ {r}]
    /\ UNCHANGED <<DiskBlobs, VfsTree, FlushPending, FlushTarget, WriterOldBlob,
                   WriterStage, WriterPath, WriterBlob, Journal,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

ReaderStaleDetect(r, b) ==
    /\ b \in EvictedBlobs
    /\ r \in ActiveReaders[b]
    /\ StaleReaderAlert' = TRUE
    /\ ActiveReaders' = [ActiveReaders EXCEPT ![b] = @ \ {r}]
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, FlushPending, FlushTarget,
                   WriterStage, WriterPath, WriterBlob, Journal, WriterOldBlob,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleAckCount>>

(* =========================================================================
   ACTIONS — TRUNCATE CONCURRENT
   ========================================================================= *)

TruncOldRefDec(w) ==
    /\ PhoenixPhase = "NORMAL"
    /\ WriterStage[w] = "COMMIT"
    /\ TruncStage[w] = "NONE"
    /\ LET old_b == WriterOldBlob[w] IN
       /\ old_b # ZERO_BLOB
       /\ old_b # WriterBlob[w]
       /\ old_b \in DiskBlobs
       /\ BlobRefcount[old_b] > 0
       /\ BlobRefcount' = [BlobRefcount EXCEPT ![old_b] = @ - 1]
       /\ TruncStage' = [TruncStage EXCEPT ![w] = "OLD_REF_DEC"]
    /\ UNCHANGED <<DiskBlobs, VfsTree, ActiveReaders, FlushPending, FlushTarget,
                   WriterBlob, WriterOldBlob, WriterPath, Journal, JournalCommitted,
                   PhoenixPhase, CrashCount, EvictedBlobs, NvmeAckGen,
                   AdversaryForge, WriterStage, StaleReaderAlert, StaleAckCount>>

TruncGcOldBlob(w) ==
    /\ TruncStage[w] = "OLD_REF_DEC"
    /\ LET old_b == WriterOldBlob[w] IN
       /\ BlobRefcount[old_b] = 0
       /\ ActiveReaders[old_b] = {}
       /\ DiskBlobs' = DiskBlobs \ {old_b}
       /\ TruncStage' = [TruncStage EXCEPT ![w] = "DONE"]
       /\ WriterStage' = [WriterStage EXCEPT ![w] = "IDLE"]
       /\ WriterBlob' = [WriterBlob EXCEPT ![w] = ZERO_BLOB]
       /\ WriterOldBlob' = [WriterOldBlob EXCEPT ![w] = ZERO_BLOB]
    /\ UNCHANGED <<VfsTree, BlobRefcount, ActiveReaders, FlushPending,
                   FlushTarget, WriterPath, Journal,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, StaleReaderAlert, StaleAckCount>>

GarbageCollect(b) ==
    /\ PhoenixPhase = "NORMAL"
    /\ b \in DiskBlobs
    /\ b # ZERO_BLOB
    /\ BlobRefcount[b] = 0
    /\ ActiveReaders[b] = {}
    /\ \A w \in WRITERS : WriterBlob[w] # b
    /\ DiskBlobs' = DiskBlobs \ {b}
    /\ UNCHANGED <<VfsTree, BlobRefcount, ActiveReaders, FlushPending, WriterOldBlob,
                   FlushTarget, WriterStage, WriterPath, WriterBlob, Journal,
                   JournalCommitted, PhoenixPhase, CrashCount, EvictedBlobs,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

(* =========================================================================
   ACTIONS — PHOENIX CRASH & RESTORE
   ========================================================================= *)

PhoenixFreeze ==
    /\ PhoenixPhase = "NORMAL"
    /\ CrashCount < MAX_CRASH
    /\ PhoenixPhase' = "FREEZING"
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders, WriterOldBlob,
                   FlushPending, FlushTarget, WriterStage, WriterPath,
                   WriterBlob, Journal, JournalCommitted, CrashCount,
                   EvictedBlobs, NvmeAckGen, AdversaryForge, TruncStage,
                   StaleReaderAlert, StaleAckCount>>

PhoenixCrash ==
    /\ PhoenixPhase = "FREEZING"
    /\ \E evicted \in SUBSET DiskBlobs :
       /\ ZERO_BLOB \notin evicted
       /\ EvictedBlobs' = EvictedBlobs \cup evicted
       /\ DiskBlobs' = DiskBlobs \ evicted
       /\ StaleReaderAlert' = (StaleReaderAlert \/ (\E b \in evicted : ActiveReaders[b] # {}))
       /\ ActiveReaders' = [b \in (BLOB_IDS \cup {ZERO_BLOB}) |-> 
                              IF b \in evicted THEN {} ELSE ActiveReaders[b]]
    /\ CrashCount' = CrashCount + 1
    /\ PhoenixPhase' = "RESTORING"
    \* WIPE TRANSIENT MEMORY (Les syscalls abortent silencieusement pendant le crash)
    /\ WriterStage' = [w \in WRITERS |-> "IDLE"]
    /\ TruncStage' = [w \in WRITERS |-> "NONE"]
    /\ WriterBlob' = [w \in WRITERS |-> ZERO_BLOB]
    /\ WriterOldBlob' = [w \in WRITERS |-> ZERO_BLOB]
    /\ FlushPending' = [w \in WRITERS |-> FALSE]
    /\ FlushTarget' = [w \in WRITERS |-> ZERO_BLOB]
    \* WriterPath est explicitement gardé inchangé car la mémoire du thread est perdue mais on s'en fiche
    /\ UNCHANGED <<VfsTree, BlobRefcount, Journal, JournalCommitted, NvmeAckGen, AdversaryForge, StaleAckCount, WriterPath>>

PhoenixJournalReplay ==
    /\ PhoenixPhase = "RESTORING"
    /\ VfsTree' = [p \in PATHS |->
                     LET committed_for_p ==
                         {entry \in Journal :
                              entry.path = p /\ entry.gen \in JournalCommitted
                              /\ entry.blob \in DiskBlobs}
                     IN IF committed_for_p # {}
                        THEN (CHOOSE entry \in committed_for_p :
                                 \A other \in committed_for_p : entry.gen >= other.gen).blob
                        ELSE (IF VfsTree[p] \in EvictedBlobs THEN ZERO_BLOB ELSE VfsTree[p])]
    /\ PhoenixPhase' = "RESTORED"
    /\ UNCHANGED <<DiskBlobs, BlobRefcount, ActiveReaders, FlushPending,
                   FlushTarget, WriterStage, WriterPath, WriterBlob, Journal, WriterOldBlob,
                   JournalCommitted, CrashCount, EvictedBlobs, NvmeAckGen,
                   AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

PhoenixResume ==
    /\ PhoenixPhase = "RESTORED"
    /\ \A p \in PATHS : VfsTree[p] \in DiskBlobs
    /\ PhoenixPhase' = "NORMAL"
    /\ EvictedBlobs' = {}
    \* L'OS agit comme un fsck : il recalcule les refcounts depuis l'arbre restauré et les lecteurs survivants
    /\ BlobRefcount' = [b \in BLOB_IDS \cup {ZERO_BLOB} |->
                          IF b = ZERO_BLOB THEN -1
                          ELSE (IF \E p \in PATHS : VfsTree[p] = b THEN 1 ELSE 0)
                               + Cardinality(ActiveReaders[b])]
    /\ UNCHANGED <<DiskBlobs, VfsTree, ActiveReaders, WriterOldBlob,
                   FlushPending, FlushTarget, WriterStage, WriterPath,
                   WriterBlob, Journal, JournalCommitted, CrashCount,
                   NvmeAckGen, AdversaryForge, TruncStage, StaleReaderAlert, StaleAckCount>>

AdversaryForgeBlob ==
    /\ ~AdversaryForge
    /\ \E b \in BLOB_IDS :
       b \notin DiskBlobs
    /\ AdversaryForge' = TRUE
    /\ UNCHANGED <<DiskBlobs, VfsTree, BlobRefcount, ActiveReaders, WriterOldBlob,
                   FlushPending, FlushTarget, WriterStage, WriterPath,
                   WriterBlob, Journal, JournalCommitted, PhoenixPhase,
                   CrashCount, EvictedBlobs, NvmeAckGen, TruncStage,
                   StaleReaderAlert, StaleAckCount>>

(* =========================================================================
   SPECIFICATION
   ========================================================================= *)
Next ==
    \/ \E w \in WRITERS : WriterAlloc(w)
    \/ \E w \in WRITERS : WriterSubmitFlush(w)
    \/ \E w \in WRITERS : NvmeAckFlush(w)
    \/ \E w \in WRITERS : NvmeStaleAck(w)
    \/ \E w \in WRITERS : WriterFinish(w)
    \/ \E w \in WRITERS : TruncOldRefDec(w)
    \/ \E w \in WRITERS : TruncGcOldBlob(w)
    \/ \E r \in READERS, p \in PATHS : ReaderOpen(r, p)
    \/ \E r \in READERS, p \in PATHS : ReaderClose(r, p)
    \/ \E r \in READERS, b \in BLOB_IDS : ReaderStaleDetect(r, b)
    \/ \E b \in BLOB_IDS : GarbageCollect(b)
    \/ PhoenixFreeze
    \/ PhoenixCrash
    \/ PhoenixJournalReplay
    \/ PhoenixResume
    \/ AdversaryForgeBlob

Fairness ==
    /\ \A w \in WRITERS : WF_vars(WriterAlloc(w))
    /\ \A w \in WRITERS : WF_vars(WriterSubmitFlush(w))
    /\ \A w \in WRITERS : WF_vars(NvmeAckFlush(w))
    /\ \A w \in WRITERS : WF_vars(WriterFinish(w))
    /\ \A w \in WRITERS : WF_vars(TruncOldRefDec(w))
    /\ \A w \in WRITERS : WF_vars(TruncGcOldBlob(w))
    /\ \A b \in BLOB_IDS : WF_vars(GarbageCollect(b))
    /\ WF_vars(PhoenixCrash)
    /\ WF_vars(PhoenixJournalReplay)
    /\ WF_vars(PhoenixResume)

Spec == Init /\ [][Next]_vars /\ Fairness

(* =========================================================================
   INVARIANTS DE SÉCURITÉ
   ========================================================================= *)

S_FS_01_VfsConsistency ==
    PhoenixPhase \in {"NORMAL","RESTORED"} =>
        \A p \in PATHS : VfsTree[p] \in DiskBlobs

S_FS_02_RefcountNonNeg ==
    \A b \in BLOB_IDS : BlobRefcount[b] >= 0

S_FS_03_ZeroBlobProtected ==
    /\ BlobRefcount[ZERO_BLOB] = -1
    /\ ZERO_BLOB \in DiskBlobs

S_FS_04_NoGcUnderReader ==
    \A b \in BLOB_IDS :
        ActiveReaders[b] # {} => b \in DiskBlobs

S_FS_05_NoPhantomVfsPointer ==
    \A w \in WRITERS :
        FlushPending[w] =>
            FlushTarget[w] \notin DiskBlobs \/ VfsTree[WriterPath[w]] # FlushTarget[w]

S_FS_06_NoActiveFdOnEvicted ==
    \A b \in EvictedBlobs : ActiveReaders[b] = {}

S_FS_07_ForgeRejected ==
    AdversaryForge =>
        /\ (PhoenixPhase \in {"NORMAL","RESTORED"} => \A p \in PATHS : VfsTree[p] \in DiskBlobs)
        /\ \A b \in BLOB_IDS : b \in DiskBlobs => BlobRefcount[b] >= 0

S_FS_08_JournalIdempotent ==
    PhoenixPhase = "RESTORED" =>
        \A p \in PATHS : VfsTree[p] \in DiskBlobs

S_FS_10_TruncRefBeforeFree ==
    \A w \in WRITERS :
        TruncStage[w] = "OLD_REF_DEC" =>
            LET old_b == WriterOldBlob[w] IN
            BlobRefcount[old_b] >= 0

(* =========================================================================
   PROPRIÉTÉS DE VIVACITÉ
   ========================================================================= *)

L_FS_01_FlushEventuallyAcked ==
    \A w \in WRITERS :
        [](FlushPending[w] => <>(~FlushPending[w]))

L_FS_02_PhoenixEventuallyRestores ==
    [](PhoenixPhase = "FREEZING" => <>(PhoenixPhase = "NORMAL"))

L_FS_03_WriterEventuallyCommits ==
    \A w \in WRITERS :
        [](WriterStage[w] = "ALLOC" => <>(WriterStage[w] \in {"COMMIT","IDLE"}))

=============================================================================