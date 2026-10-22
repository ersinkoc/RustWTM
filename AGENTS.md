# Agent rules

Read `CLAUDE.md` first; its rules bind every agent working in this repository.

The one that must never be broken: tests and audit proofs run on the
developer's own Windows machine, so no code may starve the host of address
space, memory, handles, threads or kernel pool. Such a proof crashed Windows
three times. Prove failure paths through a seam in the code instead, and ask
before anything that needs real exhaustion (it may only run in a VM).
