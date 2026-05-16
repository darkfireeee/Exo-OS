# DOC1 - Process, signals, capabilities

## SIGNAL-01

Signals are owned by `kernel/src/process/signal/`. Architecture and syscall
code may detect pending delivery, but actual user-visible delivery happens only
on the controlled return-to-userspace path.

## SIGNAL-02

`arch/x86_64/syscall.rs` and syscall dispatch code orchestrate signal return
frames. They must not move signal policy back into scheduler code.

## CAP-01

Capability creation, checking, revocation, and delegation are owned by
`kernel/src/security/capability/`. Other layers pass tokens or object ids; they
do not invent parallel authorization state.
