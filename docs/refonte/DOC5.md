# DOC5 - IPC scheduler hooks

IPC may depend on scheduler hooks for blocking, waking, and timeout behavior.
The dependency direction is one-way: scheduler primitives are injected into IPC
through hook registration, while scheduler core must not import IPC message
formats or endpoint policy.
