--------------------------- MODULE Adversarial ---------------------------
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS MAX_TICKS, IRQS, N_CYCLES

VARIABLES
    Attacker,           \* {"INACTIVE", "FORGING_CAP", "COMBINED_ATTACK", "REPLAYING_TOKEN"}
    AttackSuccessful,   \* BOOLEAN
    SystemIntegrity,    \* 0..100
    RecoveryInProgress, \* BOOLEAN
    HandoffFlag,        \* 0..1
    CryptoServerState,  \* {"RUNNING", "DEAD"}
    BlacklistedIrqs,    \* SUBSET IRQS
    ExpiredTokens,      \* Set of expired token IDs
    LedgerP0Entries,    \* Set of logs
    PhoenixCycleCount,  \* Integer
    Ticks               \* Time counter

vars == <<Attacker, AttackSuccessful, SystemIntegrity, RecoveryInProgress, HandoffFlag, CryptoServerState, BlacklistedIrqs, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount, Ticks>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ Attacker = "INACTIVE"
    /\ AttackSuccessful = FALSE
    /\ SystemIntegrity = 100
    /\ RecoveryInProgress = FALSE
    /\ HandoffFlag = 0
    /\ CryptoServerState = "RUNNING"
    /\ BlacklistedIrqs = {}
    /\ ExpiredTokens = {1, 2}
    /\ LedgerP0Entries = {}
    /\ PhoenixCycleCount = 0
    /\ Ticks = 0

--------------------------------------------------------------
\* ADVERSARIAL ACTIONS
--------------------------------------------------------------
Attack_ForgeCap ==
    /\ Ticks < MAX_TICKS
    /\ Attacker = "INACTIVE" 
    /\ Attacker' = "FORGING_CAP"
    /\ AttackSuccessful' = IF HandoffFlag = 1 THEN FALSE ELSE TRUE
    /\ Ticks' = Ticks + 1
    /\ UNCHANGED <<SystemIntegrity, RecoveryInProgress, HandoffFlag, CryptoServerState, BlacklistedIrqs, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount>>

Attack_CombinedStorm ==
    /\ Ticks < MAX_TICKS
    /\ Attacker = "INACTIVE" 
    /\ Attacker' = "COMBINED_ATTACK"
    /\ CryptoServerState' = "DEAD"
    /\ BlacklistedIrqs' = IRQS
    /\ SystemIntegrity' = 50
    /\ Ticks' = Ticks + 1
    /\ UNCHANGED <<AttackSuccessful, RecoveryInProgress, HandoffFlag, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount>>

Attack_ReplayToken ==
    /\ Ticks < MAX_TICKS
    /\ Attacker = "INACTIVE" 
    /\ Attacker' = "REPLAYING_TOKEN"
    /\ AttackSuccessful' = FALSE
    /\ Ticks' = Ticks + 1
    /\ UNCHANGED <<SystemIntegrity, RecoveryInProgress, HandoffFlag, CryptoServerState, BlacklistedIrqs, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount>>

--------------------------------------------------------------
\* SYSTEM DEFENSES & RECOVERY ACTIONS
--------------------------------------------------------------
System_TriggerDefense ==
    /\ Ticks < MAX_TICKS
    /\ Attacker = "COMBINED_ATTACK"
    /\ HandoffFlag = 0
    /\ HandoffFlag' = 1
    /\ LedgerP0Entries' = LedgerP0Entries \cup {"CRITICAL_BREACH_DETECTED"}
    /\ Ticks' = Ticks + 1
    /\ UNCHANGED <<Attacker, AttackSuccessful, SystemIntegrity, RecoveryInProgress, CryptoServerState, BlacklistedIrqs, ExpiredTokens, PhoenixCycleCount>>

System_Recover ==
    /\ Ticks < MAX_TICKS
    /\ CryptoServerState = "DEAD"
    /\ HandoffFlag = 1
    /\ CryptoServerState' = "RUNNING"
    /\ SystemIntegrity' = 95
    /\ HandoffFlag' = 0
    /\ Attacker' = "INACTIVE" 
    /\ Ticks' = Ticks + 1
    /\ UNCHANGED <<AttackSuccessful, RecoveryInProgress, BlacklistedIrqs, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount>>

System_PhoenixCycle ==
    /\ Ticks < MAX_TICKS
    /\ SystemIntegrity >= 90
    /\ PhoenixCycleCount < N_CYCLES + 2
    /\ PhoenixCycleCount' = PhoenixCycleCount + 1
    /\ Ticks' = Ticks + 1
    /\ UNCHANGED <<Attacker, AttackSuccessful, SystemIntegrity, RecoveryInProgress, HandoffFlag, CryptoServerState, BlacklistedIrqs, ExpiredTokens, LedgerP0Entries>>

--------------------------------------------------------------
\* NEXT STATE
--------------------------------------------------------------
Next ==
    \/ Attack_ForgeCap
    \/ Attack_CombinedStorm
    \/ Attack_ReplayToken
    \/ System_TriggerDefense
    \/ System_Recover
    \/ System_PhoenixCycle
    \* FIX: Allow time to pass if the attacker is stuck on a minor attack, 
    \* but force action if there is a critical combined attack!
    \/ (Ticks < MAX_TICKS /\ Attacker /= "COMBINED_ATTACK" /\ CryptoServerState /= "DEAD" /\ Ticks' = Ticks + 1 /\ UNCHANGED <<Attacker, AttackSuccessful, SystemIntegrity, RecoveryInProgress, HandoffFlag, CryptoServerState, BlacklistedIrqs, ExpiredTokens, LedgerP0Entries, PhoenixCycleCount>>)
    \/ (Ticks >= MAX_TICKS /\ UNCHANGED vars)

Spec == Init /\ [][Next]_vars /\ WF_vars(System_Recover) /\ WF_vars(System_TriggerDefense)

--------------------------------------------------------------
\* SAFETY INVARIANTS (Pure State Predicates)
--------------------------------------------------------------
S_ADV1 == (Attacker = "FORGING_CAP" /\ HandoffFlag = 1) => ~AttackSuccessful
S_ADV3 == (Attacker = "REPLAYING_TOKEN") => ~AttackSuccessful
S_ADV5 == (PhoenixCycleCount >= N_CYCLES /\ CryptoServerState = "RUNNING") => (SystemIntegrity >= 90)

--------------------------------------------------------------
\* LIVENESS PROPERTIES (Temporal Formulas)
--------------------------------------------------------------
S_ADV2 == []((Attacker = "COMBINED_ATTACK" /\ CryptoServerState = "DEAD" /\ BlacklistedIrqs /= {}) 
          => <>( (CryptoServerState = "RUNNING" /\ SystemIntegrity > 90) \/ Ticks >= MAX_TICKS ))

S_ADV4 == []((Attacker = "COMBINED_ATTACK") 
          => <>( (HandoffFlag = 1 /\ Cardinality(LedgerP0Entries) > 0 /\ ~AttackSuccessful) \/ Ticks >= MAX_TICKS ))

=============================================================================
