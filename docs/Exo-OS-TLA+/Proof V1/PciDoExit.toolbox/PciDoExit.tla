--------------------------- MODULE PciDoExit ---------------------------
EXTENDS Naturals, FiniteSets

CONSTANTS 
    PIDS,       
    BDFS        

BDF_BASES(b) == IF b = "bdf1" THEN 1000 ELSE 2000
BDF_SIZES(b) == IF b = "bdf1" THEN 100 ELSE 100

VARIABLES 
    DeviceClaims,     
    ActiveDrivers,    
    DmaMappings,      
    MmioMappings,     
    IrqHandlers,      
    IommuDomains,     
    BusMasterEnabled, 
    DoExitStage,      
    PciTopology       

vars == <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, DoExitStage, PciTopology>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ DeviceClaims     = [b \in BDFS |-> [owner |-> "NONE", base |-> 0, size |-> 0]]
    /\ ActiveDrivers    = {}
    /\ DmaMappings      = [p \in PIDS |-> FALSE]
    /\ MmioMappings     = [p \in PIDS |-> FALSE]
    /\ IrqHandlers      = {} 
    /\ IommuDomains     = [p \in PIDS |-> "NONE"]
    /\ BusMasterEnabled = [b \in BDFS |-> FALSE]
    /\ DoExitStage      = [p \in PIDS |-> 0]
    /\ PciTopology      = [b \in BDFS |-> "NONE"]

--------------------------------------------------------------
\* SYSTEM CALLS
--------------------------------------------------------------
SysPciSetTopology(bdf, parent) ==
    /\ PciTopology' = [PciTopology EXCEPT ![bdf] = parent]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, DoExitStage>>

SysPciClaim(pid, bdf) ==
    /\ DoExitStage[pid] = 0
    /\ DeviceClaims[bdf].owner = "NONE" 
    /\ DeviceClaims' = [DeviceClaims EXCEPT ![bdf] = [owner |-> pid, base |-> BDF_BASES(bdf), size |-> BDF_SIZES(bdf)]]
    /\ ActiveDrivers' = ActiveDrivers \union {<<pid, bdf>>}
    /\ UNCHANGED <<DmaMappings, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, DoExitStage, PciTopology>>

SysEnableBusMaster(pid, bdf) ==
    /\ DoExitStage[pid] = 0
    /\ <<pid, bdf>> \in ActiveDrivers
    /\ BusMasterEnabled' = [BusMasterEnabled EXCEPT ![bdf] = TRUE]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, IommuDomains, DoExitStage, PciTopology>>

SysMapMemoryAndDma(pid) ==
    /\ DoExitStage[pid] = 0
    /\ \E b \in BDFS : <<pid, b>> \in ActiveDrivers
    /\ DmaMappings' = [DmaMappings EXCEPT ![pid] = TRUE]
    /\ MmioMappings' = [MmioMappings EXCEPT ![pid] = TRUE]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, IrqHandlers, IommuDomains, BusMasterEnabled, DoExitStage, PciTopology>>

SysIrqRegister(pid, bdf) ==
    /\ DoExitStage[pid] = 0
    /\ <<pid, bdf>> \in ActiveDrivers
    /\ PciTopology[bdf] /= "NONE" 
    /\ IrqHandlers' = IrqHandlers \union {<<pid, bdf>>}
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IommuDomains, BusMasterEnabled, DoExitStage, PciTopology>>

--------------------------------------------------------------
\* DO_EXIT CLEANUP STATE MACHINE
--------------------------------------------------------------
ProcessCrashes(pid) ==
    /\ DoExitStage[pid] = 0
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 1]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, PciTopology>>

DoExit_Step1_BusMaster(pid) ==
    /\ DoExitStage[pid] = 1
    /\ BusMasterEnabled' = [b \in BDFS |-> IF <<pid, b>> \in ActiveDrivers THEN FALSE ELSE BusMasterEnabled[b]]
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 2]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, IommuDomains, PciTopology>>

DoExit_Step2_3_Retrain(pid) ==
    /\ DoExitStage[pid] \in {2, 3}
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = @ + 1]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, PciTopology>>

DoExit_Step4_Dma(pid) ==
    /\ DoExitStage[pid] = 4
    /\ DmaMappings' = [DmaMappings EXCEPT ![pid] = FALSE]
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 5]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, PciTopology>>

DoExit_Step5_Mmio(pid) ==
    /\ DoExitStage[pid] = 5
    /\ MmioMappings' = [MmioMappings EXCEPT ![pid] = FALSE]
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 6]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, IrqHandlers, IommuDomains, BusMasterEnabled, PciTopology>>

DoExit_Step6_Irq(pid) ==
    /\ DoExitStage[pid] = 6
    /\ IrqHandlers' = {h \in IrqHandlers : h[1] /= pid} 
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 7]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IommuDomains, BusMasterEnabled, PciTopology>>

DoExit_Step7_Claims(pid) ==
    /\ DoExitStage[pid] = 7
    /\ ActiveDrivers' = {d \in ActiveDrivers : d[1] /= pid}
    /\ DeviceClaims' = [b \in BDFS |-> IF DeviceClaims[b].owner = pid THEN [owner |-> "NONE", base |-> 0, size |-> 0] ELSE DeviceClaims[b]]
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 8]
    /\ UNCHANGED <<DmaMappings, MmioMappings, IrqHandlers, IommuDomains, BusMasterEnabled, PciTopology>>

DoExit_Step8_Iommu(pid) ==
    /\ DoExitStage[pid] = 8
    /\ IommuDomains' = [IommuDomains EXCEPT ![pid] = "NONE"]
    /\ DoExitStage' = [DoExitStage EXCEPT ![pid] = 0]
    /\ UNCHANGED <<DeviceClaims, ActiveDrivers, DmaMappings, MmioMappings, IrqHandlers, BusMasterEnabled, PciTopology>>

--------------------------------------------------------------
\* SYSTEM SPECIFICATION
--------------------------------------------------------------
Next == 
    \/ \E pid \in PIDS, bdf \in BDFS, p_bdf \in BDFS : 
        SysPciSetTopology(bdf, p_bdf) \/ SysPciClaim(pid, bdf) \/ 
        SysEnableBusMaster(pid, bdf) \/ SysIrqRegister(pid, bdf)
    \/ \E pid \in PIDS : 
        SysMapMemoryAndDma(pid) \/ ProcessCrashes(pid) \/ DoExit_Step1_BusMaster(pid) \/ 
        DoExit_Step2_3_Retrain(pid) \/ DoExit_Step4_Dma(pid) \/ DoExit_Step5_Mmio(pid) \/ 
        DoExit_Step6_Irq(pid) \/ DoExit_Step7_Claims(pid) \/ DoExit_Step8_Iommu(pid)

Fairness == \A pid \in PIDS : WF_vars(Next)

Spec == Init /\ [][Next]_vars /\ Fairness

--------------------------------------------------------------
\* PROPERTIES TO PROVE
--------------------------------------------------------------
Overlaps(b1, s1, b2, s2) == (b1 < b2 + s2) /\ (b2 < b1 + s1)

S19_NoDuplicateBdfClaim == 
    \A bdf \in BDFS, p1, p2 \in PIDS : 
        (<<p1, bdf>> \in ActiveDrivers /\ <<p2, bdf>> \in ActiveDrivers) => (p1 = p2)

S20_NoPhysicalOverlap == 
    \A b1, b2 \in BDFS : 
        (b1 /= b2 /\ DeviceClaims[b1].owner /= "NONE" /\ DeviceClaims[b2].owner /= "NONE") =>
        ~Overlaps(DeviceClaims[b1].base, DeviceClaims[b1].size, DeviceClaims[b2].base, DeviceClaims[b2].size)

S21_BusMasterDisableBeforeDmaRevoke == 
    \A pid \in PIDS : 
        (DoExitStage[pid] \in 4..6) => 
            (\A bdf \in BDFS : (<<pid, bdf>> \in ActiveDrivers => ~BusMasterEnabled[bdf]))

S22_DmaBeforeMmio == 
    \A pid \in PIDS : (DoExitStage[pid] \in 5..6) => (DmaMappings[pid] = FALSE)

S23_IrqBeforeClaims == 
    \A pid \in PIDS : (DoExitStage[pid] = 7) => (\A bdf \in BDFS : <<pid, bdf>> \notin IrqHandlers)

S24_TopologyBeforeIrqRegister == 
    \A pid \in PIDS, bdf \in BDFS : 
        (<<pid, bdf>> \in IrqHandlers) => (PciTopology[bdf] /= "NONE")

\* STRESS TEST INVARIANT
S19_STRESS_ConcurrentClaimRace ==
    \A bdf \in BDFS : Cardinality({p \in PIDS : <<p, bdf>> \in ActiveDrivers}) <= 1

=============================================================================
