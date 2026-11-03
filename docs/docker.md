# Docker container monitoring

RustWTM 0.24.0 adds a read-only **Containers** page and an Overview summary.
Docker is optional. The process sampler, governor and service keep working when
Docker is missing, busy or unavailable.

## Connection

Start Docker Desktop or a local Docker Engine exposing a Windows named pipe.
RustWTM reads the interactive user's active context. It opens the Engine pipe
with the window's own rights, never through the privileged RustWTM service.
No Docker CLI installation or TCP API exposure is required.

Resolution follows `DOCKER_CONTEXT`, then `DOCKER_HOST`, then `currentContext`
in the user's Docker configuration. An explicit context overrides `DOCKER_HOST`.
`DOCKER_CONFIG` selects the configuration directory; otherwise it is
`%USERPROFILE%\.docker`. Named contexts are resolved through `contexts/meta`.
The default pipe is `npipe:////./pipe/docker_engine`; Docker Desktop's
`desktop-linux` context normally uses `dockerDesktopLinuxEngine`.

Only local `npipe` endpoints are supported. TCP, SSH, Unix-socket and remote
Windows pipe contexts show an explicit unsupported-context message. RustWTM
never changes contexts or falls back silently to a different daemon. The Engine
must support API 1.41 or newer; the client negotiates a compatible version up to
1.53. No credentials are requested, sent or saved by this integration.

## Reading the page

Search by name, image, full/partial container ID or Compose project. Combine
search with the project and running-state filters. Select a container by mouse
or keyboard to open details, including ports, health, creation/start time,
restart count, CPU/memory trends and the last 200 log lines. Logs are selectable
and can be copied. Unsupported logging drivers or oversized log responses show
a read error without hiding the container list.

CPU follows Docker conventions: on Linux one fully used core is 100%, so a
container using several cores can exceed 100%. These values are not added to
Windows system CPU. Linux memory usage subtracts inactive file cache when valid;
the displayed limit is the Engine-reported limit. Windows private-working-set
and CPU fields are used when supplied. Missing metrics are shown as unavailable.

Rates need two counter samples. The first reading, resets, absent counters,
repeated source samples and monitoring gaps do not produce invented zero rates.
Statistics sampling targets the `running` state; other states do not retain
their last live rates. Failed statistics stay
marked until successfully read again. Daemon failures retain the last list with
a stale warning. Samples older than 30 seconds are not presented as current.
Histories contain at most 60 measured values per metric.

## Resource and privacy limits

A single lazy worker polls when Overview or Containers requests data; it becomes
idle after those pages stop requesting data. Refreshes wait two seconds after a
cycle. Each pipe request has a 750 ms overall deadline. Pending overlapped I/O is
cancelled and drained before buffers and handles are released. Pipe clients use
identification-level security, so the server cannot impersonate the window.

Lists are streamed into at most 513 records, showing at most 512 containers.
A cycle attempts at most 16 statistics requests under a two-second budget,
rotating fairly; large installations refresh individual rows less often.
Context metadata scanning is capped at 256 entries, configuration files at
128 KiB, ports at 128 entries per container, list replies at 2 MiB, statistics
at 128 KiB, inspection at 512 KiB and log replies at 256 KiB. Transport limits
include HTTP framing. One container's failed statistics do not fail the list.
A shutdown reply is discarded; an old detail cannot appear under a different
selected container.

All Engine requests are GET: version, container list, statistics, inspection and
logs. The integration does not start, stop, restart, remove, exec into or alter
containers. Container data and logs are held in bounded window memory; they are
not written to the history database, notifications or saved preferences.

## Validation

Unit tests use synthetic Engine responses and small private pipes. Timeout and
error paths use injected replies or one gated silent pipe, never resource
exhaustion. GUI tests use offline fixtures and cover filters, keyboard selection,
copy, stale data, late replies, compact layouts and all five languages. GPU
images use fictional containers, not the developer's real workloads.

The opt-in read-only live check needs a local daemon:

```powershell
cargo test -p wtm-core --locked -j 2 docker_live_read_only_sample -- --ignored --test-threads=1 --nocapture
```

It reads the existing daemon and containers; it creates or modifies nothing.
Regular CI does not require Docker Desktop. Live proof is recorded separately
from fixture tests and hosted installer checks in the release review.

Field and stream definitions: [Docker Engine API](https://docs.docker.com/reference/api/engine/version/v1.53/).
