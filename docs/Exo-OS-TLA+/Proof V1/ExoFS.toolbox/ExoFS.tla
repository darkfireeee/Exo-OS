----------------------------- MODULE ExoFS -----------------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS 
    ZeroBlobId, 
    BLOB_IDS

VARIABLES
    DiskBlocks,
    VfsTree,
    BlobRefcounts,
    NvmeFlushPending,
    CryptoServerReady,
    VfsServerReady,
    ReadOpStage,
    TruncateMode,
    DirectIoRequests

vars == <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending,
          CryptoServerReady, VfsServerReady, ReadOpStage, TruncateMode, DirectIoRequests>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ DiskBlocks = {ZeroBlobId}
    /\ VfsTree = [path \in {"file1"} |-> ZeroBlobId]
    /\ BlobRefcounts = [b \in BLOB_IDS \cup {ZeroBlobId} |-> IF b = ZeroBlobId THEN -1 ELSE 0] 
    /\ NvmeFlushPending = FALSE
    /\ CryptoServerReady = FALSE
    /\ VfsServerReady = FALSE
    /\ ReadOpStage = "IDLE"
    /\ TruncateMode = {}
    /\ DirectIoRequests = {}

--------------------------------------------------------------
\* ACTIONS
--------------------------------------------------------------
\* Boot Sequence
BootCrypto ==
    /\ ~CryptoServerReady
    /\ CryptoServerReady' = TRUE
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, VfsServerReady, ReadOpStage, TruncateMode, DirectIoRequests>>

BootVfs ==
    /\ CryptoServerReady
    /\ ~VfsServerReady
    /\ VfsServerReady' = TRUE
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, ReadOpStage, TruncateMode, DirectIoRequests>>

\* Write and Flush (S31, S10)
NormalWrite ==
    /\ VfsServerReady
    /\ ~NvmeFlushPending
    /\ NvmeFlushPending' = TRUE
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, CryptoServerReady, VfsServerReady, ReadOpStage, TruncateMode, DirectIoRequests>>

NvmeAckFlush ==
    /\ NvmeFlushPending
    /\ NvmeFlushPending' = FALSE
    \* At NVMe FLUSH ACK, the physical block is committed, AND the VFS is atomically updated
    /\ \E b \in BLOB_IDS:
        /\ DiskBlocks' = DiskBlocks \cup {b}
        /\ VfsTree' = [VfsTree EXCEPT !["file1"] = b]
    /\ UNCHANGED <<BlobRefcounts, CryptoServerReady, VfsServerReady, ReadOpStage, TruncateMode, DirectIoRequests>>

\* Read Sequence (S9)
ReadStart ==
    /\ ReadOpStage = "IDLE"
    /\ ReadOpStage' = "READ_DISK"
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, TruncateMode, DirectIoRequests>>

ReadChecksum ==
    /\ ReadOpStage = "READ_DISK"
    /\ ReadOpStage' = "CHECKSUMMED"
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, TruncateMode, DirectIoRequests>>

ReadDecompress ==
    /\ ReadOpStage = "CHECKSUMMED"
    /\ ReadOpStage' = "DECOMPRESSED"
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, TruncateMode, DirectIoRequests>>

ReadFinish ==
    /\ ReadOpStage = "DECOMPRESSED"
    /\ ReadOpStage' = "IDLE"
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, TruncateMode, DirectIoRequests>>

\* Direct IO (S32)
DirectIoWrite ==
    /\ VfsServerReady
    /\ \E offset \in {0, 512, 1024, 4096}:
        DirectIoRequests' = DirectIoRequests \cup {[offset |-> offset]}
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, ReadOpStage, TruncateMode>>

\* Truncate (S29, S30)
TruncateFull ==
    /\ VfsServerReady
    /\ TruncateMode' = TruncateMode \cup {[blob_id |-> ZeroBlobId, size |-> 4096]}
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, ReadOpStage, DirectIoRequests>>

TruncatePartial ==
    /\ VfsServerReady
    /\ \E b \in BLOB_IDS:
        TruncateMode' = TruncateMode \cup {[blob_id |-> b, size |-> 1024]}
    /\ UNCHANGED <<DiskBlocks, VfsTree, BlobRefcounts, NvmeFlushPending, CryptoServerReady, VfsServerReady, ReadOpStage, DirectIoRequests>>

Next ==
    \/ BootCrypto \/ BootVfs
    \/ NormalWrite \/ NvmeAckFlush
    \/ ReadStart \/ ReadChecksum \/ ReadDecompress \/ ReadFinish
    \/ DirectIoWrite
    \/ TruncateFull \/ TruncatePartial

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
\* INVARIANTS (Checking single-state facts)
S10_VfsConsistency ==
    \A path \in DOMAIN VfsTree : VfsTree[path] \in DiskBlocks

S29_ZeroBlobRefcountProtected ==
    BlobRefcounts[ZeroBlobId] = -1

S30_ZeroBlobOnlyForFullPages ==
    \A op \in TruncateMode : (op.blob_id = ZeroBlobId => op.size = 4096)

S32_DirectIoAlignment ==
    \A req \in DirectIoRequests : req.offset % 512 = 0

L2_CryptoBeforeVfs ==
    VfsServerReady => CryptoServerReady

\* PROPERTIES (Checking transitions between states)
S9_ChecksumBeforeDecompression ==
    [][ (ReadOpStage' = "DECOMPRESSED" /\ ReadOpStage /= "DECOMPRESSED") => ReadOpStage = "CHECKSUMMED" ]_vars

\* FIX: If a flush is pending AND REMAINS pending in the next step, VfsTree is frozen.
S31_NoPhantomVfsPointers ==
    [][ (NvmeFlushPending /\ NvmeFlushPending') => VfsTree' = VfsTree ]_vars

=============================================================================
