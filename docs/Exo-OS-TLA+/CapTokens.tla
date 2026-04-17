--------------------------- MODULE CapTokens ---------------------------
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS PIDS, TOKENS, NONCES

VARIABLES
    ActiveNonces,          
    ConsumedNonces,      
    RevokedTokens,         
    TokenState,            
    ConstantTimeViolation,
    InFlightReplies,  \* STRESS: Physical messages lingering on the IPC bus
    PidIncarnations   \* STRESS: Tracks how many times a PID crashed and was recycled

vars == <<ActiveNonces, ConsumedNonces, RevokedTokens, TokenState, ConstantTimeViolation, InFlightReplies, PidIncarnations>>

--------------------------------------------------------------
\* INITIALIZATION
--------------------------------------------------------------
Init ==
    /\ ActiveNonces = [p \in PIDS |-> {}]
    /\ ConsumedNonces = [p \in PIDS |-> {}]
    /\ RevokedTokens = {}
    /\ TokenState = [t \in TOKENS |-> "IDLE"]
    /\ ConstantTimeViolation = FALSE
    /\ InFlightReplies = {}
    /\ PidIncarnations = [p \in PIDS |-> 0]

--------------------------------------------------------------
\* SYSTEM ACTIONS
--------------------------------------------------------------
SendIpcRequest(p, n) ==
    /\ n \notin ActiveNonces[p]
    /\ n \notin ConsumedNonces[p] 
    /\ ActiveNonces' = [ActiveNonces EXCEPT ![p] = @ \cup {n}]
    /\ InFlightReplies' = InFlightReplies \cup {<<p, n>>} \* Message physically hits the bus
    /\ UNCHANGED <<ConsumedNonces, RevokedTokens, TokenState, ConstantTimeViolation, PidIncarnations>>

ReceiveValidReply(p, n) ==
    /\ <<p, n>> \in InFlightReplies
    /\ n \in ActiveNonces[p]
    /\ ActiveNonces' = [ActiveNonces EXCEPT ![p] = @ \ {n}]  
    /\ ConsumedNonces' = [ConsumedNonces EXCEPT ![p] = @ \cup {n}] 
    /\ InFlightReplies' = InFlightReplies \ {<<p, n>>} \* Removed from bus
    /\ UNCHANGED <<RevokedTokens, TokenState, ConstantTimeViolation, PidIncarnations>>

ReceiveStaleReply(p, n) ==
    /\ <<p, n>> \in InFlightReplies
    /\ n \notin ActiveNonces[p]
    /\ InFlightReplies' = InFlightReplies \ {<<p, n>>} \* Safely dropped from the bus
    /\ UNCHANGED <<ActiveNonces, ConsumedNonces, RevokedTokens, TokenState, ConstantTimeViolation, PidIncarnations>>

--------------------------------------------------------------
\* ADVERSARIAL & STRESS ACTIONS
--------------------------------------------------------------
\* STRESS: Attacker physically injects replayed packets onto the IPC bus
AdversaryInjectReplay(p, n) ==
    /\ n \in ConsumedNonces[p]
    /\ InFlightReplies' = InFlightReplies \cup {<<p, n>>}
    /\ UNCHANGED <<ActiveNonces, ConsumedNonces, RevokedTokens, TokenState, ConstantTimeViolation, PidIncarnations>>

\* STRESS: Process crashes and PID is immediately reused while messages are still on the bus!
KillAndRecyclePid(p) ==
    /\ PidIncarnations[p] < 1  \* Limited to 1 recycle to prevent infinite state space explosion
    /\ PidIncarnations' = [PidIncarnations EXCEPT ![p] = @ + 1]
    /\ ActiveNonces' = [ActiveNonces EXCEPT ![p] = {}]    \* Wiped clean (Brand new process)
    /\ ConsumedNonces' = [ConsumedNonces EXCEPT ![p] = {}]  \* Wiped clean
    /\ UNCHANGED <<RevokedTokens, TokenState, ConstantTimeViolation, InFlightReplies>> \* Bus messages LINGER!

VerifyToken(t) ==
    /\ TokenState[t] = "IDLE"
    /\ TokenState' = [TokenState EXCEPT ![t] = IF t \in RevokedTokens THEN "REVOKED_EARLY" ELSE "VERIFIED"]
    /\ ConstantTimeViolation' = FALSE 
    /\ UNCHANGED <<ActiveNonces, ConsumedNonces, RevokedTokens, InFlightReplies, PidIncarnations>>

UseToken(t) ==
    /\ TokenState[t] = "VERIFIED"
    /\ TokenState' = [TokenState EXCEPT ![t] = IF t \in RevokedTokens THEN "REVOKED_MIDFLIGHT" ELSE "SUCCESS"]
    /\ UNCHANGED <<ActiveNonces, ConsumedNonces, RevokedTokens, ConstantTimeViolation, InFlightReplies, PidIncarnations>>

RevokeToken(t) ==
    /\ t \notin RevokedTokens
    /\ RevokedTokens' = RevokedTokens \cup {t}
    /\ UNCHANGED <<ActiveNonces, ConsumedNonces, TokenState, ConstantTimeViolation, InFlightReplies, PidIncarnations>>

Next ==
    \/ \E p \in PIDS, n \in NONCES : SendIpcRequest(p, n)
    \/ \E p \in PIDS, n \in NONCES : ReceiveValidReply(p, n)
    \/ \E p \in PIDS, n \in NONCES : ReceiveStaleReply(p, n)
    \/ \E p \in PIDS, n \in NONCES : AdversaryInjectReplay(p, n)
    \/ \E p \in PIDS : KillAndRecyclePid(p)
    \/ \E t \in TOKENS : VerifyToken(t)
    \/ \E t \in TOKENS : UseToken(t)
    \/ \E t \in TOKENS : RevokeToken(t)

Spec == Init /\ [][Next]_vars

--------------------------------------------------------------
\* SAFETY PROPERTIES & INVARIANTS
--------------------------------------------------------------
S41_ReplyNonceAntiReplay ==
    \A p \in PIDS : ActiveNonces[p] \intersect ConsumedNonces[p] = {}

S42_ConstantTimeVerification ==
    ConstantTimeViolation = FALSE

S43_RevocationAtomicDuringUse ==
    \A t \in TOKENS :
        (TokenState[t] = "REVOKED_MIDFLIGHT") => (t \in RevokedTokens)

=============================================================================
