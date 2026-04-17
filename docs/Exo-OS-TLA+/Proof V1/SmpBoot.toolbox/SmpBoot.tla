----------------------------- MODULE SmpBoot -----------------------------
EXTENDS Naturals, FiniteSets, TLC

(***************************************************************************)
(* MODULE 2 : ExoOS Reverse Boot Séquence SMP                              *)
(* Couverture : SECURITY_READY, AP deadlock (P0-B/C/D), Ordre des 18 étapes*)
(***************************************************************************)

CONSTANTS BSP, APS

VARIABLES 
    BspBootStage,            \* 1..18 (étape courante du BSP)
    ApState,                 \* Fonction : AP -> {INIT, SIPI_RECV, SYSCALL_INIT_DONE, SPINNING, READY}
    SecurityReady,           \* BOOLEAN (atomique Release/Acquire)
    SecurityInitCalled,      \* BOOLEAN (security_init() a été exécutée)
    IommuQueueInited,        \* BOOLEAN (étape 10)
    TscCalibrated,           \* BOOLEAN (étape 12)
    InterruptsEnabled,       \* BOOLEAN (étape 13)
    GsSlot20Written,         \* Ensemble des cores où gs:[0x20] = current_tcb valide
    SyscallMsrInitedPerCore, \* Ensemble des cores avec STAR/LSTAR/SFMASK configurés
    SyscallExecuted,         \* Fonction : Core -> BOOLEAN (Un syscall a-t-il été lancé ?)
    VtdActive,               \* BOOLEAN (Le contrôleur matériel VT-d est activé, étape 9)
    IommuIrqArrived          \* BOOLEAN (Une interruption IOMMU matérielle a été reçue)

ALL_CORES == {BSP} \union APS

vars == << BspBootStage, ApState, SecurityReady, SecurityInitCalled, IommuQueueInited, 
           TscCalibrated, InterruptsEnabled, GsSlot20Written, SyscallMsrInitedPerCore, 
           SyscallExecuted, VtdActive, IommuIrqArrived >>

(* ========================================================================= *)
(* TYPE INVARIANT                                                            *)
(* ========================================================================= *)
TypeOK ==
    /\ BspBootStage \in 1..18
    /\ ApState \in [APS -> {"INIT", "SIPI_RECV", "SYSCALL_INIT_DONE", "SPINNING", "READY"}]
    /\ SecurityReady \in BOOLEAN
    /\ SecurityInitCalled \in BOOLEAN
    /\ IommuQueueInited \in BOOLEAN
    /\ TscCalibrated \in BOOLEAN
    /\ InterruptsEnabled \in BOOLEAN
    /\ GsSlot20Written \subseteq ALL_CORES
    /\ SyscallMsrInitedPerCore \subseteq ALL_CORES
    /\ SyscallExecuted \in [ALL_CORES -> BOOLEAN]
    /\ VtdActive \in BOOLEAN
    /\ IommuIrqArrived \in BOOLEAN

(* ========================================================================= *)
(* PROPRIÉTÉS STANDARD ET STRESS (Safety & Liveness)                         *)
(* ========================================================================= *)

\* S5 : L'ordre est IOMMU_QUEUE.init() → calibrate_tsc_khz() → enable_interrupts()
S5_IommuQueueInitBeforeIrqs ==
    (InterruptsEnabled => IommuQueueInited /\ TscCalibrated)

\* S6 : security_init() doit être appelée AVANT de lever SECURITY_READY (P0-B)
S6_SecurityInitBeforeReady ==
    (SecurityReady => SecurityInitCalled)

\* S7 : Tout AP qui sort de SPIN_WAITING doit avoir ses MSRs configurés (P0-C)
S7_SyscallMsrInitBeforeApReady ==
    \A ap \in APS : (ApState[ap] = "READY" => ap \in SyscallMsrInitedPerCore)

\* S8 : Le dispatcher syscall lit gs:[0x20]. Doit être écrit AVANT (P0-D)
S8_GsSlot20BeforeSyscall ==
    \A c \in ALL_CORES : (SyscallExecuted[c] => c \in GsSlot20Written)

\* S5.STRESS : Pas de race condition pendant l'initialisation de sécurité
S5_STRESS_ApRaceWithSecurityInit ==
    ~(\E ap \in APS : ApState[ap] = "SPINNING" /\ SecurityReady = TRUE /\ ~SecurityInitCalled)

\* S6.STRESS : Prouve que l'architecture empêche l'arrivée d'une IRQ IOMMU dans une queue non-initialisée
S6_STRESS_IommuIrqBeforeInit ==
    ~(IommuIrqArrived /\ ~IommuQueueInited)

\* L2 : Le boot termine toujours, sans deadlock des APs
L2_BootAlwaysCompletes ==
    <> (\A ap \in APS : ApState[ap] = "READY")

(* ========================================================================= *)
(* MACHINE À ÉTATS                                                           *)
(* ========================================================================= *)

Init ==
    /\ BspBootStage = 1
    /\ ApState = [ap \in APS |-> "INIT"]
    /\ SecurityReady = FALSE
    /\ SecurityInitCalled = FALSE
    /\ IommuQueueInited = FALSE
    /\ TscCalibrated = FALSE
    /\ InterruptsEnabled = FALSE
    /\ GsSlot20Written = {}
    /\ SyscallMsrInitedPerCore = {}
    /\ SyscallExecuted = [c \in ALL_CORES |-> FALSE]
    /\ VtdActive = FALSE
    /\ IommuIrqArrived = FALSE

\* Le BSP progresse à travers ses 18 étapes critiques de Boot
BSP_Progress ==
    /\ BspBootStage < 18
    /\ BspBootStage' = BspBootStage + 1
    /\ IF BspBootStage' = 5 THEN
          /\ SyscallMsrInitedPerCore' = SyscallMsrInitedPerCore \union {BSP}
          /\ GsSlot20Written' = GsSlot20Written \union {BSP}
          /\ UNCHANGED << IommuQueueInited, TscCalibrated, InterruptsEnabled, SecurityInitCalled, SecurityReady, VtdActive >>
       ELSE IF BspBootStage' = 9 THEN
          /\ VtdActive' = TRUE \* Le hardware IOMMU commence à fonctionner en arrière-plan
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, IommuQueueInited, TscCalibrated, InterruptsEnabled, SecurityInitCalled, SecurityReady >>
       ELSE IF BspBootStage' = 10 THEN
          /\ IommuQueueInited' = TRUE
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, TscCalibrated, InterruptsEnabled, SecurityInitCalled, SecurityReady, VtdActive >>
       ELSE IF BspBootStage' = 12 THEN
          /\ TscCalibrated' = TRUE
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, IommuQueueInited, InterruptsEnabled, SecurityInitCalled, SecurityReady, VtdActive >>
       ELSE IF BspBootStage' = 13 THEN
          /\ InterruptsEnabled' = TRUE
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, IommuQueueInited, TscCalibrated, SecurityInitCalled, SecurityReady, VtdActive >>
       ELSE IF BspBootStage' = 16 THEN
          /\ SecurityInitCalled' = TRUE
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, IommuQueueInited, TscCalibrated, InterruptsEnabled, SecurityReady, VtdActive >>
       ELSE IF BspBootStage' = 18 THEN
          /\ SecurityReady' = TRUE
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, IommuQueueInited, TscCalibrated, InterruptsEnabled, SecurityInitCalled, VtdActive >>
       ELSE
          /\ UNCHANGED << SyscallMsrInitedPerCore, GsSlot20Written, IommuQueueInited, TscCalibrated, InterruptsEnabled, SecurityInitCalled, SecurityReady, VtdActive >>
    /\ UNCHANGED << ApState, SyscallExecuted, IommuIrqArrived >>

\* Un AP reçoit l'IPI de réveil du BSP
AP_ReceiveSIPI(ap) ==
    /\ ApState[ap] = "INIT"
    /\ BspBootStage >= 2  \* Le BSP envoie l'INIT/SIPI très tôt
    /\ ApState' = [ApState EXCEPT ![ap] = "SIPI_RECV"]
    /\ UNCHANGED << BspBootStage, SecurityReady, SecurityInitCalled, IommuQueueInited, TscCalibrated, InterruptsEnabled, GsSlot20Written, SyscallMsrInitedPerCore, SyscallExecuted, VtdActive, IommuIrqArrived >>

\* Un AP initialise localement ses propres MSRs Syscall
AP_InitSyscallMsr(ap) ==
    /\ ApState[ap] = "SIPI_RECV"
    /\ SyscallMsrInitedPerCore' = SyscallMsrInitedPerCore \union {ap}
    /\ ApState' = [ApState EXCEPT ![ap] = "SYSCALL_INIT_DONE"]
    /\ UNCHANGED << BspBootStage, SecurityReady, SecurityInitCalled, IommuQueueInited, TscCalibrated, InterruptsEnabled, GsSlot20Written, SyscallExecuted, VtdActive, IommuIrqArrived >>

\* Un AP écrit le TCB dans son slot GS
AP_WriteGsSlot(ap) ==
    /\ ApState[ap] = "SYSCALL_INIT_DONE"
    /\ GsSlot20Written' = GsSlot20Written \union {ap}
    /\ ApState' = [ApState EXCEPT ![ap] = "SPINNING"]
    /\ UNCHANGED << BspBootStage, SecurityReady, SecurityInitCalled, IommuQueueInited, TscCalibrated, InterruptsEnabled, SyscallMsrInitedPerCore, SyscallExecuted, VtdActive, IommuIrqArrived >>

\* Spin-Wait : L'AP ne peut passer à READY que si le BSP a publié la barrière (Release/Acquire)
AP_SpinWait(ap) ==
    /\ ApState[ap] = "SPINNING"
    /\ SecurityReady = TRUE
    /\ ApState' = [ApState EXCEPT ![ap] = "READY"]
    /\ UNCHANGED << BspBootStage, SecurityReady, SecurityInitCalled, IommuQueueInited, TscCalibrated, InterruptsEnabled, GsSlot20Written, SyscallMsrInitedPerCore, SyscallExecuted, VtdActive, IommuIrqArrived >>

\* Simulation d'un Syscall pour prouver les failles P0-C et P0-D
Core_ExecuteSyscall(c) ==
    /\ \/ c = BSP /\ BspBootStage = 18
       \/ c \in APS /\ ApState[c] = "READY"
    /\ ~SyscallExecuted[c]
    /\ SyscallExecuted' = [SyscallExecuted EXCEPT ![c] = TRUE]
    /\ UNCHANGED << BspBootStage, ApState, SecurityReady, SecurityInitCalled, IommuQueueInited, TscCalibrated, InterruptsEnabled, GsSlot20Written, SyscallMsrInitedPerCore, VtdActive, IommuIrqArrived >>

\* Une IRQ matérielle frappe le système
Iommu_TriggerIrq ==
    /\ VtdActive           \* Le hardware tourne
    /\ InterruptsEnabled   \* Le CPU accepte les interruptions vectorielles (STI)
    /\ ~IommuIrqArrived
    /\ IommuIrqArrived' = TRUE
    /\ UNCHANGED << BspBootStage, ApState, SecurityReady, SecurityInitCalled, IommuQueueInited, TscCalibrated, InterruptsEnabled, GsSlot20Written, SyscallMsrInitedPerCore, SyscallExecuted, VtdActive >>

Next ==
    \/ BSP_Progress
    \/ (\E ap \in APS : AP_ReceiveSIPI(ap))
    \/ (\E ap \in APS : AP_InitSyscallMsr(ap))
    \/ (\E ap \in APS : AP_WriteGsSlot(ap))
    \/ (\E ap \in APS : AP_SpinWait(ap))
    \/ (\E c \in ALL_CORES : Core_ExecuteSyscall(c))
    \/ Iommu_TriggerIrq

Spec == Init /\ [][Next]_vars /\ WF_vars(Next)

=============================================================================