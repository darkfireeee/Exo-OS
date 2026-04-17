--------------------------- MODULE Memory ---------------------------
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS CORES, VARS, BSP

VARIABLES
    AtomicWrites,     \* Mapping: var -> {value, ordering, writer_core}
    AtomicReads,      \* Mapping: core -> (var -> value)  (Core's local view of memory)
    HappensBefore,    \* Set of <<writer_core, reader_core>> indicating sync edges
    ReleaseFence,     \* Snapshot of writer's local views at time of Release: var -> (var -> value)
    AcquireFence,     \* Set of <<core, var>> representing successful acquires
    VisibilityGap     \* BOOLEAN: Reserved to track stale reads

vars == <<AtomicWrites, AtomicReads, HappensBefore, ReleaseFence, AcquireFence, VisibilityGap>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ AtomicWrites = [v \in VARS |-> [value |-> 0, ordering |-> "None", core |-> "NONE"]]
    /\ AtomicReads = [c \in CORES |-> [v \in VARS |-> 0]]
    /\ HappensBefore = {}
    /\ ReleaseFence = [v \in VARS |-> [vx \in VARS |-> 0]]
    /\ AcquireFence = {}
    /\ VisibilityGap = FALSE

--------------------------------------------------------------
\* ABSTRACT MEMORY MODEL (SNAPSHOT & MERGE)
--------------------------------------------------------------
WriteRelaxed(c, var, val) ==
    /\ AtomicWrites' = [AtomicWrites EXCEPT ![var] = [value |-> val, ordering |-> "Relaxed", core |-> c]]
    /\ AtomicReads' = [AtomicReads EXCEPT ![c][var] = val]
    /\ UNCHANGED <<HappensBefore, ReleaseFence, AcquireFence, VisibilityGap>>

WriteRelease(c, var, val) ==
    /\ AtomicWrites' = [AtomicWrites EXCEPT ![var] = [value |-> val, ordering |-> "Release", core |-> c]]
    /\ AtomicReads' = [AtomicReads EXCEPT ![c][var] = val]
    \* The magic of Release: snapshot the core's entire local view into the fence
    /\ ReleaseFence' = [ReleaseFence EXCEPT ![var] = AtomicReads'[c]]
    /\ UNCHANGED <<HappensBefore, AcquireFence, VisibilityGap>>

ReadAcquire(c, var) ==
    /\ AtomicWrites[var].ordering = "Release"
    \* The magic of Acquire: merge the Release snapshot into the reader's view
    /\ AtomicReads' = [AtomicReads EXCEPT ![c] = 
                        [v \in VARS |-> 
                            IF ReleaseFence[var][v] = 1 THEN 1 ELSE AtomicReads[c][v]]]
    /\ HappensBefore' = HappensBefore \cup {<<AtomicWrites[var].core, c>>}
    /\ AcquireFence' = AcquireFence \cup {<<c, var>>}
    /\ UNCHANGED <<AtomicWrites, ReleaseFence, VisibilityGap>>

--------------------------------------------------------------
\* SYSTEM ACTIONS (EXO-OS BOOT & INTERRUPTS)
--------------------------------------------------------------
BSP_InitIommuSlots ==
    /\ AtomicWrites["iommu_slots"].value = 0
    /\ WriteRelaxed(BSP, "iommu_slots", 1)

BSP_InitIommuFlag ==
    /\ AtomicWrites["iommu_slots"].value = 1
    /\ AtomicWrites["iommu_init"].value = 0
    /\ WriteRelease(BSP, "iommu_init", 1)

BSP_SetSecurityReady ==
    /\ AtomicWrites["iommu_init"].value = 1
    /\ AtomicWrites["SECURITY_READY"].value = 0
    /\ WriteRelease(BSP, "SECURITY_READY", 1)

Core_CasMaskedSince(c) ==
    /\ AtomicWrites["masked_since"].value = 0
    /\ WriteRelease(c, "masked_since", 1)

AP_SyncIommu(c) ==
    /\ c # BSP
    /\ ReadAcquire(c, "iommu_init")

AP_SyncSecurity(c) ==
    /\ c # BSP
    /\ ReadAcquire(c, "SECURITY_READY")

Core_SyncMasked(c) ==
    /\ ReadAcquire(c, "masked_since")

Next ==
    \/ BSP_InitIommuSlots
    \/ BSP_InitIommuFlag
    \/ BSP_SetSecurityReady
    \/ \E c \in CORES : Core_CasMaskedSince(c)
    \/ \E c \in CORES : AP_SyncIommu(c)
    \/ \E c \in CORES : AP_SyncSecurity(c)
    \/ \E c \in CORES : Core_SyncMasked(c)

Spec == Init /\ [][Next]_vars

--------------------------------------------------------------
\* SAFETY PROPERTIES & INVARIANTS
--------------------------------------------------------------
\* S47: APs see SECURITY_READY as TRUE if they synced via Acquire
S47_SecurityReadyVisibility ==
    \A c \in CORES :
        (c # BSP /\ <<c, "SECURITY_READY">> \in AcquireFence) =>
            (AtomicReads[c]["SECURITY_READY"] = 1)

\* S48: masked_since published correctly
S48_MaskedSinceReleaseVisible ==
    \A c \in CORES :
        (<<c, "masked_since">> \in AcquireFence) =>
            (AtomicReads[c]["masked_since"] = 1)

\* S49: IOMMU init Release implies payload slots are visible
S49_IommuInitRelease ==
    \A c \in CORES :
        (AtomicReads[c]["iommu_init"] = 1) =>
            (AtomicReads[c]["iommu_slots"] = 1)

=============================================================================
