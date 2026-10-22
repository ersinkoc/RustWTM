# Agent rules

## Never starve the host

Tests and audit proofs run on the developer's own Windows machine. Never write
or run code that exhausts a system resource: address space (huge thread stacks,
giant reservations), commit or physical memory, handles, threads, kernel pool,
ETW loggers.

A proof that reserved nearly the whole address space as thread stacks to make
`CreateThread` fail crashed Windows three times (bugcheck 0x1A / 0x3453 while
the test process was torn down).

To prove a failure path, inject the failure through a seam in the code (a
spawn or allocation function passed in, as `run_client` in
`crates/wtm-service/src/ipc_server.rs` does). If real exhaustion is the only
way, stop and ask; it may only run in a VM.

Pass these rules on in the prompt of every subagent or audit worker.
