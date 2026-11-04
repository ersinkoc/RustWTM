# Product completion roadmap

Baseline: 0.25.0, reviewed on 2026-11-04. An implementation or a CI definition
is not equivalent to hosted validation or a published feature. Record those
outcomes separately.

| Priority | Work | Concrete acceptance gate | Current boundary |
| --- | --- | --- | --- |
| P0 | Windows publisher signing | Own executable payload signed before MSI build; MSI signed; trust/publisher/timestamp verified; final digests match downloaded files | Optional Azure/OIDC workflow prepared; owner identity/account configuration and first signed release required |
| P0 | Build provenance | ZIP/MSI attestation verifies repository, workflow and exact source commit | Release provenance job defined; CI uses a real installer artifact as its verification control |
| P0 | Upgrade durability | Previous release -> candidate, repair, downgrade rejection, config byte retention, SQLite record retention, service IPC and uninstall retention | Dedicated disposable-VM job defined; never run on developer hardware |
| P1 | Visual CI | Gauge gallery at four DPI scales; Overview/Performance/Containers in five languages, both themes and sizes; screenshots uploaded | GPU rendering job defined; image artifacts aid review, not automatic pixel-diff acceptance |
| P1 | Interactive charts | Explicit time ranges, synchronized cursor, zoom/reset, real timestamps and event markers; correct missing/stale samples; keyboard access | Current plots remain fixed-window; start with in-memory ranges, then persisted aggregates for longer windows |
| P1 | Guided first use | Explain service/elevation/capabilities, monitoring vs protection, safe profile choices and what will change before enabling controls | Existing settings/protection levels are available; dedicated onboarding is not implemented |
| P1 | Unified diagnosis | Correlate process, system, sensor, governor and container events by timestamp; explain source and uncertainty; navigate to existing details | Existing diagnostics and events are separate surfaces; a shared incident timeline is not implemented |
| P2 | Personal dashboard | Persist card visibility/order with a default/reset path; responsive layout, keyboard operation and migration tests | Overview card layout remains fixed |
| P2 | Docker persistence and alerts | Bounded retention, container identity across restart/removal, duration-based thresholds and recovery notices; distinguish Docker CPU conventions | Current read-only snapshots are bounded window memory; no persistent container history or alerts |
| P2 | Support bundle | User-selected export with version/capability status, recent errors and redaction; exclude credentials, private command lines and container logs by default | No one-click support bundle |
| P2 | Updater publisher policy | Signed builds reject wrong-publisher/unsigned updates after secure copy; test canceled UAC, installer error and relaunch | Current updater validates digests; do not claim full automatic rollback or publisher pinning |
| P2 | Local native TCP test boundary | Safe ephemeral-listener proof and independent Win32 control; no weaker assertions; pass locally and in hosted Windows | Two local Unknown(0)/missing-listener failures remain; hosted baseline tests pass |
| Manual hardware gate | Physical cooling certification | Per-model RPM/curve/default-restoration, stale sensor, disconnect and lease-expiry evidence, within firmware safety limits | Simulation checks do not certify a physical channel; no fan writes authorized by this roadmap |

## Delivery order

First establish distribution and automated evidence: signing preparation,
provenance, installer lifecycle and visual CI. Next build chart interaction and
guided first use, then a shared diagnosis timeline. Dashboard customization,
container history/alerts and support exports follow those foundations.

For long histories, store bounded aggregates with timestamps and provenance;
do not extrapolate absent samples into invented zero usage. Container history
needs an explicit schema/retention decision and does not belong in per-process
application rows. Unified diagnosis should correlate observations before any
automatic recommendation is allowed to change enforcement.

## Validation separation

- Local checks: PowerShell parsing, workflow lint, real signed Windows-file
  control, rejection of unsigned/wrong-publisher files, and refusal of installer
  execution outside the explicitly enabled disposable runner.
- Hosted CI: source tests/coverage, fresh install, upgrade/repair/data retention,
  localized rendering and provenance verification of the candidate MSI.
- Release: actual signing only after account setup, final-byte checksums,
  downloaded executable/MSI identity, and release-workflow provenance.
- Manual: identity validation and physical hardware controls stay owner-operated.
