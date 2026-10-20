# Sensors and cooling

RustWTM 0.13.0 adds a dedicated Sensors & cooling workspace. Temperatures,
fan RPM, duty percentages, power, voltage, load and clocks are shown only when
the provider reports them. Unknown values stay unavailable; a real zero RPM
can mean fan-stop. Duty percentage is never labelled RPM.

## Setup

Release ZIP/MSI packages include `hardware/wtm-hardware.exe` and its license
notices. The helper is self-contained. For a development checkout:

```powershell
cargo build --workspace
./scripts/build-hardware.ps1
target/debug/wtm.exe sensors --json
```

The provider build needs the .NET 8 SDK or newer. It restores the committed
lockfile and pins LibreHardwareMonitorLib 0.9.6. For a release build, use
`-OutputDir target/release/hardware` after `cargo build --release --workspace`.
The helper is discovered beside the executable; debug builds also look in the
checkout's `target/debug/hardware`. No PATH search or arbitrary provider command
is accepted. When the helper is absent, the installed NVIDIA driver's
`System32/nvidia-smi.exe` supplies read-only GPU telemetry if available.

The GUI reads cached data from the service, or starts a dedicated read-only
worker in local mode. The CLI reads locally. Neither collection path runs on
the GUI frame or the governor's process-sampling loop. Discovery/update is
roughly every two seconds; device calls can take longer. Samples older than
ten seconds are marked stale and cannot authorize software fan control.

CPU/motherboard access depends on administrator rights, supported chips and
hardware-access driver compatibility. The helper uses LibreHardwareMonitor's
normal access path; RustWTM does not install a separate driver package or
change Windows security settings. Restart the application/service after a
provider failure or after adding the helper. Mixed 0.12/0.13 clients and services
are rejected by the protocol handshake; update them together (IPC version 10).

## Controls

Only actual provider control channels appear. A channel must allow at least
30% duty and a 100% emergency request; named pump channels are disabled.
Channel labels do not establish physical wiring: a motherboard header connected
to a pump may still be labelled as a fan by firmware.

To apply a manual setting or curve, use an elevated RustWTM window connected to
the matching service, switch the service to enforcing, choose a real temperature
source and confirm the physical change. Enforcing also enables the configured
process policies. Editing controls or reopening the program applies nothing.
Profiles are saved as GUI drafts, while active control is session-only in the
service. Hardware automatic explicitly requests the provider's default mode.

Curves have 2–12 ordered temperature points and nondecreasing duties. Upward
changes are immediate; decreases use a 3% deadband and at most five percentage
points per second. At the selected critical temperature (60–95 °C), both manual
and curve modes request 100%. Missing, nonfinite or stale temperature requests
hardware defaults. Explicit profile requests and errors appear in Events.

The provider records a rollback lease before writing to a device, renews it on
each accepted duty request and attempts defaults after eight seconds without
renewal. It retries failed expiry restores while running, and also attempts
restoration on closed input and normal shutdown. The watchdog shares the
hardware-operation lock: a hung device API, forcibly terminated helper, driver
failure or OS failure can prevent restoration. These mechanisms are not a
hardware fail-safe and do not replace firmware thermal protection.

## Validation on this workstation

Read-only live discovery found NVIDIA GeForce RTX 5080 and AMD integrated
graphics on an ASUS ROG STRIX B650E-E / Ryzen 9 9950X3D system. LibreHardwareMonitor
reported GPU temperature, memory-junction temperature, power, voltage, clocks,
fan RPM and three GPU control channels. The non-elevated probe correctly
reported those channels as read-only. Detailed CPU/board driver access was not
validated. No physical fan duty was changed.

Automated checks cover finite values, freshness, curve validation/interpolation,
thermal override, smoothing, protocol reply correlation, UI gating/confirmation
and fake-control lease deadlines/rollback retries. They cannot establish the
mapping, acoustic response or recovery behavior of a physical channel.

Before claiming write support on a hardware model, record manual/curve response,
measured RPM, hardware-default restoration, stale/missing temperature, service
disconnect and lease expiry on that model. Physical write/crash recovery,
MSI installation/upgrade and CPU/motherboard driver compatibility remain
separate release gates.

Upstream references: [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor),
[IControl bounds and defaults](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/master/LibreHardwareMonitorLib/Hardware/IControl.cs).
