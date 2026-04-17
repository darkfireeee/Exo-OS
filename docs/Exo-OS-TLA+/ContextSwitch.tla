--------------------------- MODULE ContextSwitch ---------------------------
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS 
    CORES,      
    NULL        

\* FIX: Explicitly define exactly 3 threads instead of 8 combinations!
TCB_SET == {
    [kstack_ptr |-> 100, fs_base |-> 10, user_gs_base |-> 30],
    [kstack_ptr |-> 200, fs_base |-> 20, user_gs_base |-> 40],
    [kstack_ptr |-> 300, fs_base |-> 50, user_gs_base |-> 60]
}

VARIABLES 
    CurrentTcb,      
    TssRsp0,         
    Cr0TsBit,        
    FsBase,          
    UserGsBase,      
    GsSlot20,        
    FpuRegisters,    
    XSaveArea,       
    SwitchStage,     
    NextTcb          

vars == <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase, GsSlot20, 
          FpuRegisters, XSaveArea, SwitchStage, NextTcb>>

SymmetryCores == Permutations(CORES)

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ CurrentTcb      = [c \in CORES |-> CHOOSE t \in TCB_SET : TRUE]
    /\ TssRsp0         = [c \in CORES |-> CurrentTcb[c].kstack_ptr]
    /\ Cr0TsBit        = [c \in CORES |-> TRUE] 
    /\ FsBase          = [c \in CORES |-> CurrentTcb[c].fs_base]
    /\ UserGsBase      = [c \in CORES |-> CurrentTcb[c].user_gs_base]
    /\ GsSlot20        = [c \in CORES |-> CurrentTcb[c]]
    /\ FpuRegisters    = [c \in CORES |-> 0]   
    /\ XSaveArea       = [t \in TCB_SET |-> 0] 
    /\ SwitchStage     = [c \in CORES |-> 0]
    /\ NextTcb         = [c \in CORES |-> NULL]

--------------------------------------------------------------
\* ACTIONS
--------------------------------------------------------------
SysUseFpu(c) ==
    /\ SwitchStage[c] = 0
    /\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = FALSE]
    /\ FpuRegisters' = [FpuRegisters EXCEPT ![c] = CurrentTcb[c].kstack_ptr]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, FsBase, UserGsBase, GsSlot20, XSaveArea, SwitchStage, NextTcb>>

StartSwitch(c, next) ==
    /\ SwitchStage[c] = 0
    /\ CurrentTcb[c] /= next
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 1]
    /\ NextTcb' = [NextTcb EXCEPT ![c] = next]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea>>

Step1_Xsave(c) ==
    /\ SwitchStage[c] = 1
    /\ XSaveArea' = IF ~Cr0TsBit[c] 
                    THEN [XSaveArea EXCEPT ![CurrentTcb[c]] = FpuRegisters[c]]
                    ELSE XSaveArea
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 2]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase, GsSlot20, FpuRegisters, NextTcb>>

Step2_SetLazyBit(c) ==
    /\ SwitchStage[c] = 2
    /\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = TRUE]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 3]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step3_4_Internal(c) ==
    /\ SwitchStage[c] \in 3..4
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step5_AsmSwitch(c) ==
    /\ SwitchStage[c] = 5
    /\ CurrentTcb' = [CurrentTcb EXCEPT ![c] = NextTcb[c]]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 6]
    /\ UNCHANGED <<TssRsp0, Cr0TsBit, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step6_7_Internal(c) ==
    /\ SwitchStage[c] \in 6..7
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step8_UpdateTss(c) ==
    /\ SwitchStage[c] = 8
    /\ TssRsp0' = [TssRsp0 EXCEPT ![c] = CurrentTcb[c].kstack_ptr]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 9]
    /\ UNCHANGED <<CurrentTcb, Cr0TsBit, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step9_10_RestoreMSRs(c) ==
    /\ SwitchStage[c] \in 9..10
    /\ FsBase' = IF SwitchStage[c] = 9 THEN [FsBase EXCEPT ![c] = CurrentTcb[c].fs_base] ELSE FsBase
    /\ UserGsBase' = IF SwitchStage[c] = 10 THEN [UserGsBase EXCEPT ![c] = CurrentTcb[c].user_gs_base] ELSE UserGsBase
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step11_Finish(c) ==
    /\ SwitchStage[c] = 11
    /\ GsSlot20' = [GsSlot20 EXCEPT ![c] = CurrentTcb[c]]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 0]
    /\ NextTcb' = [NextTcb EXCEPT ![c] = NULL]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase, FpuRegisters, XSaveArea>>

Next == \E c \in CORES :
    \/ SysUseFpu(c)
    \/ \E t \in TCB_SET : StartSwitch(c, t)
    \/ Step1_Xsave(c)
    \/ Step2_SetLazyBit(c)
    \/ Step3_4_Internal(c)
    \/ Step5_AsmSwitch(c)
    \/ Step6_7_Internal(c)
    \/ Step8_UpdateTss(c)
    \/ Step9_10_RestoreMSRs(c)
    \/ Step11_Finish(c)

Spec == Init /\ [][Next]_vars

--------------------------------------------------------------
\* INVARIANTS
--------------------------------------------------------------
SwitchInProgress(c) == SwitchStage[c] /= 0

S25_STRESS_IrqFpuSafety ==
    \A c \in CORES : (~Cr0TsBit[c]) => (FpuRegisters[c] = CurrentTcb[c].kstack_ptr)

S26_TssRsp0MatchesCurrentTcb ==
    \A c \in CORES : (~SwitchInProgress(c) => TssRsp0[c] = CurrentTcb[c].kstack_ptr)

S27_FsGsMatchNewThread ==
    \A c \in CORES : (~SwitchInProgress(c) => 
            FsBase[c] = CurrentTcb[c].fs_base /\ UserGsBase[c] = CurrentTcb[c].user_gs_base)

S28_GsSlot20MatchesCurrentTcb ==
    \A c \in CORES : (~SwitchInProgress(c) => GsSlot20[c] = CurrentTcb[c])

=============================================================================
