# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.30.1] - 2026-11-12

### Added

- AI assistant with your own API key. Right-click a process → *Diagnostics* →
  *Ask AI about it*, or *Ask AI about this PC* in the system overview: RustWTM
  gathers the facts (usage, path, command line, signature, parent and child
  processes, hosted services, ports, startup entries, 24-hour history) and a
  model explains what the process is, whether it looks legitimate, whether it
  is safe to end, and what slows the PC down. Providers: Anthropic (Claude,
  native Messages API), OpenAI, Google Gemini, OpenRouter, Groq, DeepSeek,
  Mistral, xAI, and local models through Ollama, LM Studio or any
  OpenAI-compatible server. Set up in *Settings → AI* (with *List models* and
  *Test connection*) or with `wtm ai setup`, `wtm ai key`, `wtm ai explain`,
  `wtm ai advise`; `--dry-run` prints what would be sent.
- Privacy and safety: off until you agree; API keys are sealed with DPAPI for
  your Windows account; secrets in command lines (passwords, tokens, keys,
  URL credentials) and your account name are masked before sending, command
  lines can be left out entirely, and *Show what was sent* displays the exact
  text. Facts are fenced as data so program-chosen names cannot instruct the
  model, and the model only advises: it is never given control. Keys travel
  only over HTTPS or to this PC, and redirects are never followed with a key.
- The assistant can look things up: when the facts it was given do not settle a
  question, the model may call RustWTM's read-only tools (another process, its
  tree or modules, services, startup programs, connections, history). Bounded
  to six rounds and five minutes, every result masked and fenced like the
  facts, control tools refused, and *Stop* ends it. Models without tool
  support answer from the facts. Turn it off in *Settings → AI assistant* or
  with `wtm ai setup --lookups off`. Ready-made questions (what slows this PC
  down, startup programs to disable, is it safe to end, could it be malware)
  and *Ask AI about it* in the selected process's details.
- MCP server: new read-only tools `process_report`, `process_tree`,
  `process_modules`, `services`, `startup_items` and `app_history`, and the
  prompts `explain_process`, `diagnose_slowdown` and `security_review`.
- Processes: new optional User name, Architecture (x86/x64/ARM64), Elevated and
  File version columns. The Diagnostics column profile includes them. Values
  that this window's rights cannot read show as "—", never as "No".
- Hide system processes: a toolbar switch hides Windows components and services
  (core processes, session-0 programs from the Windows folder,
  System32/SysWOW64/SystemApps helpers). Settings → Processes has editable
  "Always show" and "Also hide" program lists. The selected process always
  stays visible.
- Process tree: nodes can be collapsed and expanded individually. The column
  header menu has Expand all / Collapse all, and there is a "Start the process
  tree collapsed" option.
- Group processes by user: Settings → Processes offers Automatic / Always group
  / Never group for the List view. Automatic groups only while more than one
  person runs programs. A group's context menu offers Manage user accounts and
  "Sign out…", which uses the existing sign-off confirmation.

### Changed

- Verify and reuse complete published website image packages on repeated backfills,
  preserving their bytes when measured frame timings differ in a fresh render.
- Build a versioned website image package from the exact release's fixture-rendered
  interface, and publish it with the installers so homepage highlights and the
  screenshot gallery can follow new stable releases automatically.
- Refresh the product website after successful release publication, release-note
  edits and changelog changes through a repository-scoped GitHub App connection.
- The CPU overload alert closes by itself 15 seconds after the load is back to
  normal, with a visible countdown; *Keep it open* stops it.
- Suspending a process, or every process of an app, now asks for confirmation
  first and explains that its windows stop responding until it is resumed.
  Core Windows processes and RustWTM itself stay protected as before.

## [0.30.0] - 2026-11-11

### Added

- A skippable five-step quick setup opens on first use and once after each
  version change. Review language, appearance, process layout, tray readouts
  and refresh/update preferences before applying. Existing choices are preserved;
  reopen the setup from Settings at any time. The final review groups choices
  into compact sections with expandable details and direct editing; edited
  sections return to the review without committing the draft.
- Startup apps support case-insensitive name, command, scope and source search,
  combined with enabled/disabled filters, Ctrl+F, Escape and filter reset.
- Choose row and column separators independently across tables. Processes can
  show the Windows file description with the executable name in muted text.
  Name sorting follows the selected presentation and search accepts either name.
- Right-click process headers for sort direction, grouped column visibility,
  fitting widths, column layouts and display options. Data views expose copy,
  navigation and applicable item actions through context menus.
- Process actions group resource limits and diagnostics into compact submenus.
  Disk activity menus copy file paths and readings, and open the selected owner
  in Processes. Process and port menus share viewport-bounded scrolling.

### Fixed

- Overview survives zero-width native startup and measurement passes. Resource
  cards fall back to one column before gutters can produce a negative width.
  Page layout waits for a nonempty client area while visibility and close
  handling remain active. Regression coverage includes every page and recovery
  to a normal window size.
- Restored native window geometry respects the 820×500 minimum, including stale
  64×64 saved windows. Valid sizes, position, maximization and visibility are
  preserved; non-finite dimensions fall back to the normal default size.
- Quick setup keeps language-save errors visible above its scrolling review
  without shifting navigation. Reduced motion also disables automatic scroll
  animations throughout the window and restores normal scrolling when disabled.
- Pages open directly into their content. Settings tabs and cards use the full
  workspace width. Sampling age and the process Inspector menu are in the footer.
- Service thresholds use aligned setting rows with associated accessible labels.
- Installed application and container tables follow the selected row density.
  Container selection highlights the full row and accepts clicks on its values.
- Table separator controls are also available in Appearance settings. Startup
  scope labels use readable account names and the selected-entry toolbar wraps.

## [0.29.0] - 2026-11-11

### Added

- 22 more interface languages, for 27 in total: Simplified and Traditional
  Chinese, Hindi, Bengali, Brazilian Portuguese, Russian, Indonesian, Japanese,
  Marathi, Vietnamese, Telugu, Swahili, Filipino, Tamil, Korean, Italian,
  Gujarati, Kannada, Polish, Ukrainian, Dutch and Punjabi (Gurmukhi). The Windows
  language option recognizes them, each loads only when shown, and Chinese,
  Japanese and Korean use the matching Windows font. Right-to-left languages
  remain unavailable until the GUI toolkit supports bidirectional text.
- Optional numeric notification-area icons for CPU percentage, memory percentage
  and a selected temperature sensor. Configure each icon, source, refresh interval
  and logo visibility under Settings / This window. Hidden-window logic updates
  the values; missing or stale samples render dashes. Icons share the existing
  Show/Notifications/Exit menu and retain a recoverable logo fallback.
- Device workspaces with bounded recent charts, exact identities, missing-data
  states and read-only storage, GPU, network, CPU, memory and battery telemetry.
  Query NVMe health through Windows storage properties; expose temperature,
  endurance and lifetime counters when the controller permits it.
- Current-user Store inventory and optional signed Microsoft WinGet discovery.
  Review up to 16 selected source/ID/version updates and recheck each identity
  before requesting its interactive installer. Installation VM validation pending.
- Persistent five-second resource/contributor history with recorded protection
  actions, representative replay moments and reviewed diagnostic JSON copy.
- Local file/folder drop and Restart Manager owner investigation, with explicit
  scan limits and identity-safe navigation to existing process controls.
- Visual process rule editor, action/threshold controls, pure-engine simulation
  and reviewed Work/Game/Battery policy profiles.
- First-run guide, keyboard reference, opt-in global show/hide shortcut, window
  repaint intervals and Mbps selection for device charts.
- Upgrade coverage from the preceding 0.28.0 MSI, alongside the existing pinned
  baselines, on disposable Windows CI runners.

### Changed

- IPC protocol 16 adds incident replay. Upgrade the GUI, CLI and service together
  with the MSI or bundled installer; protocol 15 clients/services are rejected
  during the handshake rather than receiving unsupported replay requests.
- Rework device navigation with per-device sparklines and independently scrolling
  selection. Disk Details separates measured health, endurance, temperature,
  available NVMe spare capacity, lifetime counters, specifications and raw health
  fields. GPU Overview shows stable engine and dedicated/shared-memory graphs;
  memory shows a composition bar only when both used and available are known.
  Compact layouts retain the same controls and square, padded sections.
- Refine all desktop views through shared graphite/light surfaces, raised page
  headers, quieter inset navigation, aligned resource readings and padded section
  cards. Give service settings a responsive two-column layout while preserving
  compact-window actions and square geometry.

- Present RustWTM as an MIT-licensed oxog.net project maintained by OXOGNET OÜ,
  Estonia, across the desktop About page, documentation and executable/installer
  metadata. Use the active rustwtm.com and rustwtm/rustwtm project links.

### Fixed

- Confirm zero-state TCP rows against the operating system's listener-only table
  before identifying them as listeners. Unconfirmed rows retain their unknown
  state; raw zero is never globally reclassified. Bound table allocation and
  growth retries even when an API reports an unreasonable required size.

## [0.28.0] - 2026-11-10

### Changed

- Discover updates and installer releases by the existing GitHub repository ID,
  preserving the source across owner/name transfers without a manual cutover.
  Keep discovered recovery links and asset/checksum pairs together, bound release
  metadata reads and exercise transfer scenarios in CI.
- Prepare canonical project links, installer metadata and update endpoints for
  github.com/rustwtm/rustwtm, with rustwtm.com as the project website. Upgrade CI
  follows the repository identity supplied by GitHub Actions.

### Added

- Add dedicated hardware specifications and installed desktop application pages.
  Show system/BIOS, processor cache/clock and disk firmware details; provide
  application search, scope filters, sorting and registered uninstall handoff
  with confirmation and registration revalidation. Package updates remain pending.
- Search all desktop pages by name or description with Quick navigation
  (Ctrl+K). Open a result with the mouse or arrow keys and Enter; Escape closes
  the dialog. Existing filters, drafts and confirmation shortcuts are preserved.
- Show reported RAM generation and module form factor in system identity cards,
  hardware details and the copied hardware summary; unsupported codes remain
  unknown instead of being inferred from a product name.

## [0.27.2] - 2026-11-10

### Fixed

- Copy the actual hardware inventory from System overview, including model,
  manufacturer, RAM module, disk and graphics details. Keep copying disabled
  until inventory is available; never substitute current utilization readings.

### Improved

- Apply a shared page gutter and section spacing to every desktop tab. Align
  form headings/navigation with their content; remove stacked card margins and
  keep raw configuration editing at full width.
- Unify tab/selector styling, distinguish primary actions and group window
  settings into readable cards. Use consistent resizable tables for user
  sessions and managed processes, retaining scrolling and action confirmations.

## [0.27.1] - 2026-11-10

### Improved

- Refine both desktop themes with explicit neutral control states, quieter
  navigation and card boundaries, more legible secondary text, lighter heat maps
  and a consistent footer scale. Align settings controls in wide windows and
  stack them in compact windows; simplify chart legend typography and keep
  resource colours focused on the readings.

- Give desktop cards small gutters while preserving square corners and joined
  table cells. Sortable table headers now paint and respond across the full cell.
- Toggle startup apps directly from their status switch; filter services by
  running/stopped state; adjust service percentage thresholds with sliders.
- Place the navigation toggle in the brand header and separate footer connection,
  resource and action information into compact rows with inline notifications.
- Show CPU, motherboard, RAM module, disk and GPU identities from a bounded,
  read-only Windows inventory, loaded off the UI thread once per window.
  Embed identities in the system inspector resource cards and group identical
  RAM modules; match disk models by physical disk index.

- Align page-level toolbars and gutters across all desktop views; group limit,
  kill-list and service editors into readable sections; use adaptive About cards.
  Restore missing header/numeric-cell padding and horizontal scrolling for
  history and configured rule lists so compact-window actions remain reachable.

- Use square, adjoining charts, tables, cards, toolbars and dialogs, separated
  by fine boundary lines. Restore measured content padding and control spacing
  while keeping neighbouring panels and table cells joined.
- Add Appearance, Process workspace and Charts and history settings, with
  remembered scale, row heights, reduced animation, sidebar layout, resource-card
  height, table behavior, column profiles, chart range/grid/fill/events/cursors,
  reset actions and independent settings-page scrolling in all five languages.

- Add an adaptive system/process inspector with saved visibility modes, a
  read-only system overview and explicit summary copying.
- Show complete system totals above sortable process columns, independent of
  row filters; clarify physical-memory, process-I/O and adapter-total scopes.
- Group the process-view selector, calm table row backgrounds and enlarge
  resource-strip values while retaining compact layouts.
- Refine the system pane with compact vector-icon facts, utilization tracks,
  visible system counts and explicit unavailable readings; distinguish reading
  values from units typographically and retain accessible close controls.

## [0.27.0] - 2026-11-09

### Added

- Shared CPU, memory, disk, network and GPU resource cards above Processes,
  with keyboard-accessible navigation and a saved visibility preference.
- Live process inspection that holds identity-based row order under the pointer
  and while menus or dialogs are open, while resource values keep updating.
  List, tree and Apps membership reconcile new/exited processes and PID reuse;
  F6 retains its separate frozen-snapshot behavior.
- Basic, CPU, Memory, I/O, Network and Diagnostics column profiles, a searchable
  transactional column editor, accessible ordering and saved column layouts.
  Unknown future column identifiers preserve compatible preferences.
- Timestamp-based Performance timeline with covered 1/2/5-minute ranges,
  synchronized cursors, pinned inspection, zoom/reset, keyboard stepping,
  recent event markers and return to live. Sampling gaps and invalid values
  remain unavailable instead of appearing as zero activity.
- Focused CPU, memory, GPU, network and disk views with saved resource selection,
  CPU performance-tier filters and searchable current GPU engine types.

### Improved

- Coordinated dark surfaces, resource colors, selection and process inspector
  hierarchy, with primary actions near process identity and current load.
- Compact process filters, layouts and scroll behavior, plus localized controls
  and synthetic visual coverage in all five supported languages.
- Portable packages include the documentation's screenshot previews and logo.

### Scope

The timeline uses the existing bounded recent samples. Physical device-specific
history, persistent system history and the remaining desktop roadmap milestones
are not included in this release. Service policy and IPC contracts are unchanged.

## [0.26.2] - 2026-11-06

### Added

- Optional Windows Task Manager replacement in Settings → This window, with
  `Ctrl+Shift+Esc` opening or bringing forward RustWTM. Enabling requires an
  elevated window and an administrator-protected installation. Existing foreign
  replacements/image filters are preserved. The setting is off by default.
- `wtm task-manager` reports the current integration; `--enable` enables it and
  `--restore` removes only this installation's owned registration. Uninstall
  restores the Windows launch path before removing binaries, including when the
  service was already removed. Cleanup failures keep the installation files.

### Fixed

- Keep port menus on their original listener when a refresh inserts rows;
  close stale menus if the protocol, owning PID or process creation time changes.
- Keep process and app context menus attached to their original targets when
  live sorting moves rows; an old menu cannot transfer to a reused PID.
- Keep background launches visible when the notification-area icon cannot be
  created, so the window remains accessible without a tray icon.
- Monitor/Protect controls now handle valid quoted TOML keys and omitted default
  mode keys, preserving comments and unrelated rule text when saving.

## [0.26.1] - 2026-11-05

### Fixed

- Reject incomplete or malformed trailers in chunked Docker HTTP responses.
- Restore existing governor throttles when an application becomes exempt.
- Retain failed rollback requests and original settings until reconciliation
  succeeds; retry them on live-process ticks and preserve other active sources.
- Keep process rollback information when termination fails, so later restores
  can still undo priority, efficiency and limit changes.

## [0.26.0] - 2026-11-04

### Improved

- Disk analysis opens in its own page instead of moving below the live file
  activity list. Folder, largest-file and file-type results scroll within the
  available window height; returning to disks and reopening preserves results,
  expansion and scroll position. Folder columns fit compact windows.
- Reopening analysis while an earlier scan is still pending retains the worker
  instead of accumulating scans blocked on filesystem access.
- Remember administrator mode after an elevated launch, for manual reopen and
  optional Windows sign-in startup. Fixed current-user tasks require a protected
  installation path, highest interactive privilege and administrator-owned task
  ACLs; portable builds retain the normal UAC fallback. Settings expose both
  preferences and report save failures. Uninstall removes owned launch tasks.

### Distribution and validation

- Optional Azure Artifact Signing with OIDC in a separate signing job: sign the
  four own executables, rebuild/sign MSI, verify trust/publisher/timestamp, then
  compute final ZIP/MSI digests. Configured signing failures block publication.
- Final-artifact GitHub provenance and verification of a real CI installer.
- Localized GPU rendering CI with four-DPI gauge galleries and screenshot artifacts.
- Dedicated clean-VM upgrade/repair/downgrade rejection checks with configuration
  and SQLite history retention, alongside existing fresh-install tests.
- Estonia company/individual signing guidance and an evidence-based completion
  roadmap. No public-trust certificate or physical hardware certification is
  implied by these workflow definitions.

## [0.25.0] - 2026-11-04

### Improved

- Resource gauges render complete, smooth circles at different DPI scales,
  with round progress caps, an inner rim, fine scale marks and resource colors.
  Fixes the four flat edges caused by the track stroke extending past its clip.
- Overview resource cards have clearer values, more graph space, subtle grid
  lines, fading area fills and visible latest samples. The same sparklines
  improve container histories and per-core tiles.
- Performance graphs use a lighter grid, clearer resource lines and quieter
  fills; per-core tiles reflect load intensity. Visual-review fixtures include
  timestamps so the full performance histories are visible during review.

## [0.24.1] - 2026-11-03

### Added

- Publishes the Docker container monitoring introduced in 0.24.0: the Containers
  page, Overview summary, CPU/memory histories, network/disk rates, Compose
  filters, ports, health and bounded read-only logs in all five languages.

### Fixed

- Classify native named-pipe wait timeouts correctly even when Windows' whole
  millisecond wait expires before the request's high-resolution deadline.
  Cancellation and draining still complete before releasing I/O buffers.
- A deterministic regression covers native timeout and zero-millisecond poll
  codes independently of wall-clock scheduling, alongside the silent-peer test.

The 0.24.0 source tag is retained for traceability; its release workflow stopped
at the timeout regression, before publishing any downloadable assets.

## [0.24.0] - 2026-11-03

### Added

- Containers page for read-only Docker Engine monitoring: search by name, image,
  ID or Compose project, filter by project and running state, and sort the list.
- Per-container CPU, memory/limit, network and disk rates, bounded CPU/memory
  history, IPv4/IPv6 port mappings, health, start time, restart count and the last
  200 log lines with full-text copy. Overview includes a Docker summary and link.
- Active local Docker context discovery, including Docker Desktop Linux and
  Windows named-pipe endpoints, DOCKER_CONTEXT, DOCKER_HOST and DOCKER_CONFIG.
  Unsupported remote endpoints show an explicit diagnostic.
- Container controls in English, Turkish, German, French and Spanish,
  with compact/wide layouts and light/dark themes.

### Reliability

- Docker uses a separate lazy worker and read-only Engine API requests under
  the interactive user's rights. Requests have deadlines and byte caps; pending
  pipe I/O is cancelled and drained safely. No Docker CLI subprocess polling.
- First, missing, repeated, reset or stale samples cannot manufacture zero usage
  or traffic rates. Sampling is fair and bounded, histories are capped, and
  failed statistics remain marked until a successful reading.
- Docker failures retain an explicitly stale snapshot; leaving the relevant
  pages pauses polling. Selected-container replies are checked against identity.
  No container control operations, service protocol changes or new dependencies.

## [0.23.0] - 2026-11-03

### Added

- Search the Events page by process name, PID or message, including translated
  messages. Combine search with severity filters and clear it in one click.
- Open an event message with a click or keyboard activation to read its full,
  selectable text in a bounded details card, and copy it without truncation.
  Details remain open while filtering and close when their event is evicted.

### Improved

- Events controls use a separate card and a matching-result count. Navigation
  rows show an explicit keyboard focus outline in both themes.
- New event controls are translated into Turkish, German, French and Spanish.

### Fixed

- Failed process controls remain failures on retry instead of being reported as
  successful without another attempt. Applied-state summaries now reflect only
  successful operations, retaining independent state after partial failures.
- Disabling and re-enabling memory protection or the rescue core before the
  next sample resets the observation timer. Time spent disabled cannot cause
  an immediate memory trim or rescue-core engagement. Existing trim cooldowns
  and release behavior are preserved.
- IPv6 TCP and UDP endpoints preserve their interface scope identifiers, so
  link-local addresses retain the interface needed to identify the endpoint.

## [0.22.0] - 2026-10-29

### Improved

- The Users table scrolls both ways, keeping long account names and larger
  session lists usable in compact windows.
- Overview resource cards open their matching performance graphs with one
  click or keyboard activation. Cards expose their current values to screen
  readers and show a visible focus outline.
- The status toolbar wraps to fit small windows. Operation messages have a
  separate, truncated row with the full text available on hover, keeping
  task-launch and administrator actions visible even after long errors.

### Fixed

- Keyboard navigation and selections opened from other pages reveal the
  selected process in long lists without overriding later manual scrolling.
- The first new service notification is delivered even when the initial
  event backlog is empty. History is fetched once per connection, and
  reconnecting still keeps old notifications silent.
- Old or failed monitoring samples no longer trigger CPU overload popups.
  Repeated cached readings do not count as sustained load, and a monitoring
  gap starts a new observation window.
- Turning the CPU governor off and back on resets its overload and recovery
  timers, so time spent disabled cannot trigger immediate throttling.

## [0.21.0] - 2026-10-26

### Fixed

- Match-all kill and suspend rules cannot bypass the safety check by adding
  an empty or stars-only command-line filter. Broad kill-list entries also
  require a command-line filter that actually narrows the selected programs.
- Wildcard patterns keep matching when a command line contains a literal `*`.
  Rules and kill-list filters previously missed matching commands after
  treating that character as the pattern's star instead of its input.
- An IPC client that panics returns its connection slot, so failed clients
  cannot permanently reduce the service's available connection quota.
- The GUI navigation test waits for its own process in a service snapshot,
  rather than treating a cached snapshot from before test startup as ready.

### Changed

- Release builds verify that the tag, Rust workspace, hardware provider and
  dated release notes agree before building. Format, Clippy and documentation
  checks now gate publication alongside tests, using the committed Cargo lockfile.

## [0.20.2] - 2026-10-23

### Fixed

- `wtm` commands and Telegram's /kill, /info, /suspend and /wait match
  process names case-insensitively beyond ASCII (`çizim` is `Çizim.exe`), as
  /find already did.
- `wtm app-limit set` and `remove` find an app's limit whatever the case of
  its name; a limit set under another case used to be added a second time,
  and its budget ignored.
- Settings and the Overview's protection levels change a configuration that
  writes its tables inline (`governor = { enabled = true }`) or with dotted
  keys; before, the change was refused, and a level button did nothing.
- Game mode no longer starts for an ordinary maximized window (a browser, an
  editor) on a monitor without a visible taskbar: with the taskbar hidden
  automatically, or on a second monitor, such a window covered the whole
  monitor and counted as full screen.

## [0.20.1] - 2026-10-22

### Fixed

- A rule or kill-list entry that ends or suspends `*.exe`, `**`, `*.*` or
  `match.path = "*"` is refused like `*`: such a rule ended every
  unprotected program as it started. A command line (or a narrower name or
  path) still picks programs out, e.g. `*.exe` with `cmdline = "vitest"`.
  A configuration holding such an entry no longer loads; the service runs on
  its defaults in dry run and says why.
- App limits on names that are not plain ASCII (`Çizim.exe`) are applied;
  before, they planned the processes but capped nothing, and the Limits tab
  and `wtm app-limit list` showed them without usage.
- Turning game mode, or its power plan option, off while a game is in front
  puts the previous power plan back at once instead of when the service
  stops.
- A program added to the rescue core's exemptions while it is engaged gets
  every core back at once, not when the load recovers.
- Insights: a second finding with actions (memory growth after chronic CPU)
  reaches the instances already running, and a finding that clears takes its
  actions back while another one stands.
- The stuck-swarm cleanup never ends copies of RustWTM's own programs.
- Updating works from a user folder with a typographic apostrophe
  (`C:\Users\O’Brien`); such a quote could end the quoted path in the
  update scripts.
- Telegram: when the bot cannot answer a button press in time, it still
  reports what the press ended or suspended.

## [0.20.0] - 2026-10-22

### Changed

- Stopping, restarting or disabling a service and blocking a program's
  network access are asked first.
- Snapshots and the service's recent points carry the time they were
  measured (IPC protocol 15). The data age and the out-of-date warning use
  it, so a snapshot received again from the service's cache ages instead of
  reading as new; the age now reads *Data measured N s ago*.
- Network, Services and Ports searches clear with Escape or a ✖ button as
  the process search does; Network and Ports say when nothing matches and
  offer to clear the search; table toolbars wrap in a narrow window.
- Performance and process graphs place each point at its measurement time:
  a slower `sample_interval_ms` or a delayed sample keeps its real place on
  the time axis, and a snapshot received twice is drawn once.
- The block list refuses programs whose blocking would break signing in,
  elevation, services, consoles or installers (`consent.exe`,
  `svchost.exe`, `msiexec.exe` and others). A rule's `cpu_cap_relative`
  needs `when.cpu_above`, which is also the least it resolves against.
- The MCP server offers `create_time` as text: JavaScript clients round it
  as a JSON number, which a rounded value still matches.

### Security

- Policies acting on their own (kill list, rules, governor, game mode,
  insights) never end or suspend `svchost.exe` or RustWTM's own programs;
  the config refuses such kill-list entries and rules, and a `*` name with
  kill or suspend. Insights no longer touch service processes (session 0).
- MCP control tools require `create_time`, so a process that took the PID
  since the listing is never changed; `wtm panic` and `recent_events` use
  only the installed service's pipe, not one a program took while the
  service is not running.
- `wtm-service install` refuses a program folder that others than SYSTEM,
  Administrators and TrustedInstaller can change, or a folder above it they
  could rename or take: whoever can replace the program gets SYSTEM. Set
  `RUSTWTM_ALLOW_ANY_INSTALL_DIR=1` on a development machine to install from
  a build folder. No folder on the way may be a link.
- `install.ps1` makes the install folder admin-only before copying, through
  handles and without following links (`wtm-service prepare-install-dir`
  sets aside what is not only administrators' and refuses a folder others
  could move, or one reached through a link), refuses stray DLLs, checks
  and copies the download from an admin-only folder, and `uninstall.ps1`
  continues from a fresh admin-only folder instead of a fixed name in the
  user's temp folder. Both delete folders without following links in them.
- The updater checks the installer again after copying it to a folder only
  administrators can reach, and installs that copy: the download in the
  user's temp folder could be swapped after it was checked.
- The hardware provider no longer unpacks native libraries to a temp folder
  (as SYSTEM possibly `C:\Windows\Temp`, where .NET would reuse files planted
  beforehand); they ship next to it.
- Releases are built with read-only permissions, without a build cache and
  with third-party actions pinned to a commit; only a separate job that runs
  no project code may publish.
- The service makes sure its data folder is its own before it reads it: a
  folder (or anything in it) owned by someone else, writable by others, a
  link or a file with a second name is set aside under an unguessable name
  and created anew, without following any link. If that cannot be done the
  service stops with a service-specific error instead of running on it.
- Clients no longer trust the service pipe while the service is installed
  but not running, and the service keeps its pipe name even when a client
  vanishes before it is accepted.
- Protection rules judge the image of the process actually opened, not the
  name a request gives (also for refusing to dump `lsass.exe`).

### Fixed

- Limits a failed lift left on a process are lifted again on the next tick
  and when the service stops, instead of staying for good.
- An app limit whose launcher exited before its child was counted is lifted
  rather than dropped with the child still capped.
- Changing the governor, memory guard, insight or game mode settings lets go
  at once of what they applied (as rules already did): a newly exempt app
  is no longer kept throttled.
- A failed history query keeps the insight findings instead of lifting and
  re-announcing them all.
- Setting the clock forward no longer makes every copy of a crowd old enough
  to end.
- *Why won't it end* and the module list check the process's start time and
  hold it open while they look, so a PID taken by a later process is never
  described as the one asked about.
- Downloads and Telegram answers are limited in size and total time: a
  server sending without end can no longer grow the service's memory or
  hold it.
- Reading a command line retries when the process changes it meanwhile;
  restarting a service waits when its status cannot be read.
- `wtm kill --tree` and `free-port --tree` no longer report failure for
  children that ended with their parent, and `free-port` lists every
  failure of a tree, not the first.
- MCP: answers to the client and malformed messages are handled as JSON-RPC
  requires (`-32600`, unknown tools `-32602`), and *why not responding*
  says when its analysis is incomplete.
- The Ports tab no longer ends a program missing from the process list (it
  could have been a later program with the same PID); it says to try again.
- Without the service, "Calm them all down" spares the app in front as it
  does with the service.
- The Network tab narrows to the selected process only while that process
  runs (not a later one with its PID), and offers *Show all*.
- The Apps view adds up the network rates of an app's processes.
- The Disks tab checks a typed folder off the window's thread: a sleeping
  network drive no longer freezes the window.
- GPU engine load per type is its busiest engine, as in Task Manager: two
  video engines at 60% each read 60%, not 100%; a process shows its busiest
  engine instead of the sum of engines of one type.
- Trace sessions no longer pile up (Windows allows 64): a window waits for
  its worker to stop its sessions when it closes, its own disk trace stops
  after 30 s unread or once the service answers, the service clears those
  closed windows left every 10 minutes, and session names carry the owner's
  start time so a reused PID no longer keeps an orphan alive.
- A power plan game mode switched is put back after a crash too: it is
  remembered on disk while replaced and restored at the next start or on
  uninstall, instead of leaving the machine on High performance.
- The service stopped accepting clients for good after 16 client threads
  failed to start (for example during a memory shortage); a failed start now
  gives its slot back.
- A connection's announced frame length no longer reserves up to 32 MiB
  before any of the body has arrived.
- Unblocking a program deleted a `Debugger` value another program had put
  in its place, and failed outright when RustWTM's own value was already
  gone.
- App history lost the earlier part of a minute written twice (the service
  restarted within it, or the clock went back); the parts are now merged,
  weighted by sample count.
- An update check started while an update was installing replaced the
  install's progress and offered the install again.
- A priority or efficiency mode rolled back to the value from before an
  unrelated earlier limit instead of the one in place when RustWTM took it.
- *End process tree* on a PID reused by a newer process ended that
  process's children.
- A process whose exit code is 259 was reported as still running, and a
  window title longer than 63 characters could match a different window.
- Panic, *End all* and *Calm them all* lost requests past the service's 16
  clients and showed only the last success; requests now go in order over
  one connection and failures are reported.
- Removing or editing a rule left what it applied in place; switching from
  dry-run to enforcing never acted on processes already running.
- A network adapter coming back up showed a burst of its whole traffic
  since boot; a failed sensor provider was never started again; lifting a
  CPU and memory limit stopped at the first failure and lost the job.
- App history counted an app's CPU only over the samples it was seen in,
  so a short-lived helper looked like a chronic hog; fully throttled
  processes held the governor's slots.
- Wait-chain requests behind a walk stuck in a frozen process each left a
  thread blocked; process icons were cropped on scaled displays.
- The first local sample after losing the service averaged over all the
  time the service was used; limits set without the service could not be
  lifted once it connected; acting on a process that is not shown replaced
  the details of the one that is.
- The kill list saved an edit over whichever entry had moved into its
  place; kill and limit forms emptied before the save was confirmed;
  Settings saved over changes made elsewhere meanwhile without a warning.
- `wtm kill <name> --tree` failed on matches inside another match's tree;
  `wtm free-port` could end a process that got a port owner's PID.
- MCP server: a process started since the service's last snapshot was
  reported as missing; a line that is not UTF-8 ended the server; batches
  and non-object messages got no answer; `network_connections` with a
  malformed `pid` listed every process's connections.

## [0.19.0] - 2026-10-21

### Added

- `wtm-service cleanup` removes RustWTM's blocked-program entries and its
  firewall rules; the MSI runs it on a full uninstall and
  `wtm-service uninstall` (used by `uninstall.ps1`) runs it too.
- The MSI asks a running RustWTM window to close before it removes or
  replaces files.
- `wtm list`, `wtm info` and `wtm ports` take `--json`.
- Snapshots say whether a process is suspended and whether per-process
  network traffic was measured; the service status carries the error of a
  configuration file it could not load (IPC protocol 14).

### Fixed

- Uninstalling left blocked programs pointing at a deleted `rustwtm.exe` and
  RustWTM's firewall rules in place.
- *Remove limits* reported success for a process RustWTM had not limited; it
  now says so.
- Resuming a process suspended several times left it frozen; resume now
  undoes every suspension.
- A client could suspend itself through the service and freeze.
- An invalid configuration at service start was only logged to file; it is
  now an event and part of the status.
- A priority or efficiency mode set by hand was undone when the governor
  later rolled back its own change.
- The hardware sensor helper kept running after the Sensors tab was closed;
  it now stops after 30 s without requests unless a fan profile is active.
- `wtm list`/`top` and the MCP server no longer present unmeasured
  per-process network traffic as 0 B/s. The MCP server measures CPU, disk
  and network over a fresh window instead of since the previous call (or
  uses the service's sample), and its control tools accept the
  `create_time` from `list_processes` to refuse a reused PID.
- The stale 0.13.0 review is no longer installed by the MSI.
- An Explorer window on a folder named RustWTM stopped RustWTM from
  starting; the single-instance check now uses a per-session lock and finds
  the running window by its program.
- *Restart as administrator* could end with no RustWTM running; the new copy
  waits for the old one to exit.
- With a newer service the window advised installing an older version; the
  advice and the status badge now name the actual problem.
- Events were listed twice after reconnecting to the same service.
- Ending processes or testing Telegram no longer holds up monitoring
  updates while the request runs.
- A RustWTM hidden in the tray now quits when Windows or an installer asks
  it to.
- Raw error codes on a service restart and command-line wording in the
  window's error messages are replaced by plain messages.

### Changed

- The window draws with OpenGL instead of wgpu: about 250 MB less memory
  and a 30% smaller program.
- Visual pass: readable status colours and visible bar tracks and text
  fields in the light theme; full column headers in every language with
  right-aligned numbers; one card title and sortable-header style; loading
  and empty states with text; a stale-data banner that dims the page; the
  data age on the subtitle line; a gear icon for Settings; labels before
  controls in the power and cooling forms; translated priority, startup and
  finding names; level filter and sorting in Events; a *⋯* menu for the
  busiest apps' actions.
- Processes marks suspended processes and shows "—" for network traffic
  that is not measured; Settings and the Overview show why the service
  could not load `config.toml`.
- Ctrl+Tab and Ctrl+Shift+Tab switch pages, Ctrl+F searches Services,
  Network and Ports, and Esc closes the process details.
- The *More below* hint also shows on Performance, About and the process
  details.

## [0.18.1] - 2026-10-21

### Changed

- Settings: split into pages chosen at the top (*This window*, *Service
  settings*, *config.toml*, *Telegram*) instead of one long scrolling page;
  the `config.toml` editor fills the window.
- Pages and lists that continue below the visible area (the Disks tab, the
  space analysis tables, the config editor and other long pages) fade at the
  bottom and show a *More below* button that scrolls further.

## [0.18.0] - 2026-10-21

### Changed

- Performance pass across every crate, measured with new ignored timing tests
  (2000 synthetic processes):
  - Policies: one tick of rules, limits, kill watch, insights and the other
    policies costs about 2 ms instead of 55–70 ms. Wildcard matching no longer
    allocates, and per-rule state is kept only for processes a rule matches.
  - Window: the process table is filtered, grouped and sorted only when the
    data, search, filters or sort change, not on every frame; a frame of the
    Processes tab takes about 1 ms instead of 2–3.5 ms.
  - Service: snapshot, status and recent-event replies are encoded straight
    from shared state instead of copied first; history samples and queries
    allocate far less (History tab query 3–4 times faster); the start watcher
    runs only when a rule needs it.
  - System queries: GPU and disk counters, process snapshots, connection
    tables and the disk and network traces reuse their buffers and make fewer
    calls; per-event maps use a faster hasher.
  - Space analysis: workers share folders at every depth, so one huge folder
    no longer runs on a single thread (about twice as fast there).
  - Process tree lookups and sorting by name are 6–10 times faster.

## [0.17.0] - 2026-10-21

### Added

- Processes: *Started in the last N min* lists only recently started processes
  (presets from 1 minute to 4 hours, or any number of minutes) and shows the
  *Running for* column. *More filters* adds disk-active (1 MiB/s+), GPU-active
  (5%+) and session (user apps, or services and system in session 0) filters;
  active filters show as removable chips and combine with the quick filters
  and search.
- Disks tab: drives with label, file system and free space; live activity per
  physical disk and the processes with the most disk I/O; and a space analysis
  of any drive or folder with a folder tree (sizes, share of parent, file
  counts), the largest files and space per file type, with *Show in Explorer*.
  Scans run on background-priority worker threads, can be stopped, report
  unreadable folders and never follow junctions or symbolic links.
- Disks tab: *Files being read and written* per program, from a kernel ETW
  trace that runs only while the tab is open (service or administrator).
  Without administrator rights the service shows only your session's programs.
- Not responding programs: marked in Processes (with a quick filter) and on
  the Overview; *Analyze wait chain* in the details panel shows what their
  blocked threads wait for and flags deadlocks.
- *Why won't it end?*: protection, drivers that withhold the right to end it,
  critical processes and the programs still holding it open.
- *Modules*: loaded DLLs with their Authenticode or catalog signer, unsigned
  and invalid ones first.
- Network bytes per process (ETW in the service, or in the window when run as
  administrator): a *Network* column, details rows and the busiest programs in
  the Network tab. *Close* resets an IPv4 TCP connection.
- *Block network access*: two Windows Firewall rules per program, grouped as
  RustWTM; listed with *Unblock* in the Network tab.
- Game mode (`[game_mode]`): while a full-screen app is in front, busy
  background apps get below-normal priority and efficiency mode, optionally the
  High performance power plan; rolled back when it ends or the service stops.
  Only apps in the game's own session are calmed, never services or other
  users' apps.
  Shown on the Overview and in the status bar, configurable in Settings.
- Handle and thread leak findings in the app history (report only).
- `wtm mcp [--allow-control]`: a Model Context Protocol server on stdio for AI
  assistants, read-only unless `--allow-control`.

### Changed

- Memory dumps are written from a process snapshot (the process is frozen for
  milliseconds), falling back to a live dump; lsass and LsaIso are refused.
- IPC protocol version 13: foreground reports carry full screen, status
  carries game mode, and connection closing, firewall and disk activity
  requests were added. Install the service of the same version.
- Usage history stores average handles and threads per app; older databases
  are upgraded in place.

## [0.15.0] - 2026-10-21

### Added

- A monitoring-data receipt age on every GUI page, with a stale-data warning
  after a sampling failure or a delayed update. The last received values remain
  visible. The timeout allows slower sampling intervals; successful updates
  clear the warning. All five interface languages are supported.
- Resources reserved for RustWTM (`[reserve]`): while the CPU is overloaded a
  rescue core is kept free for RustWTM and Windows by moving other programs off
  it with CPU Sets, and handed back when the load recovers. The service keeps
  `working_set_mb` resident and opts out of EcoQoS; the window runs above
  normal priority with 64 MB resident. Shown on the Overview and in the status
  bar, configurable in Settings.
- Window drawing times on the About page, and `RUSTWTM_FRAME_REPORT=<file>`
  to benchmark every page and write per-page frame timings.

### Changed

- The window reads command lines and image paths of new processes without
  holding the data lock it needs for every frame, so a burst of new processes
  no longer stalls drawing. The About logo is uploaded once instead of every
  frame, and the Overview cards line up at equal height.
- IPC protocol version 12 (status carries the rescue core).

## [0.14.0] - 2026-10-21

### Added

- GUI and desktop notifications in English, Turkish, German, French and Spanish.
  The language selector applies immediately and persists per Windows user;
  the default follows the Windows UI language. Missing translations fall back
  to English, while unknown/native diagnostic details retain their source text.
- Translation coverage and placeholder checks in the regular test suite,
  five-language page/navigation tests and optional visual reviews in both themes
  at wide and compact sizes.
- 96 and 128 px sizes in the embedded product icon, alongside the existing
  16, 20, 24, 32, 48, 64 and 256 px sizes; executable resource verification.

### Changed

- IPC remains protocol 11.
- Navigation width follows translated labels; saved actions, process identities
  and service configuration keys remain language-independent.

## [0.13.1] - 2026-10-21

### Added

- Hybrid CPU topology from Windows CPU Sets: `wtm cores [--json]`, processor
  group/core identifiers and relative P/E/LP-E tiers without hard-coded class values.
- `wtm affinity <target> --pcores|--ecores|--lp-ecores|--all-cores` and a CPU core
  type menu in Processes. PID identity/protection checks remain enforced;
  processes spanning multiple groups are rejected rather than truncated.
- Performance graphs label hybrid tiers; uniform CPUs omit the hybrid controls.
- Explicit shared Windows frequency limits in Sensors & cooling and
  `wtm power show|limit|restore`. Administrator access and confirmation are
  required for writes. Original values are saved before mutation, partial
  failures attempt rollback, and detected external changes are preserved.
  These are power-plan settings, not per-PID MHz caps; class 1 is not assumed
  to mean E-cores.

### Changed

- IPC protocol 11 adds core-tier requests.
  Update the GUI and service together.

## [0.13.0] - 2026-10-20

### Added

- **Sensors & cooling**: device-grouped temperatures, RPM, power, voltage and
  clocks, current/min/max, recent samples, filtering and explicit stale/missing
  values; `wtm sensors [--json]` provides local read-only telemetry.
- Optional, packaged LibreHardwareMonitor 0.9.6 provider, with NVIDIA driver
  telemetry as a read-only fallback when the helper is absent.
- Capability-based manual duty and editable temperature curves, Quiet/Balanced/
  Performance presets, thermal full-speed override and hardware-default mode.
  Writes require administrator access, an enforcing service, fresh temperature
  and explicit confirmation. Drafts persist without reapplying on startup.
  Provider leases attempt default restoration after missed renewals/closed input;
  physical write and failure-recovery validation remains pending per hardware.
- **Pause process updates** (button or F6): inspect a stable process snapshot
  while monitoring, history, protection and alerts continue. Search, sorting
  and process actions remain available; actions retain PID-reuse checks.
- **Quick process filters**: high CPU (at least 5%, excluding System Idle),
  high memory (at least 512 MiB), and managed processes. Filters combine with
  search, include a matching-process count and offer a clear-filter empty state.
- Optional GPU visual review: `cargo test -p wtm-gui --features visual-tests
  render_release_review -- --nocapture` renders five pages in two themes and
  two window sizes to `target/visual-review` using synthetic data.
- **Stuck swarms are cleaned up** (opt-in): with `[swarm] end_after_mins`
  (Settings → Service settings → Swarm warning) the service ends copies of a
  swarm that have run longer than that, together with what they started
  (their console hosts), up to 100 per second, with one notification per
  round; in dry run it only reports them. The swarm card offers the same by
  hand: *End the N running over 10 min*.

### Changed

- Navigation uses consistent vector icons with accessible names, labelled
  groups and a clear active marker; page headings explain each view.
- Refined typography and light/dark surfaces, Windows UI and symbol font
  fallbacks, readable resource-text colours and named theme selectors.
- Overview and configuration pages scroll in short windows; the last row of
  overview metrics fills the available width. Processes supports horizontal
  column scrolling and a scrollable, closeable details panel.
- Process search also matches executable paths.
- Named-pipe protocol is now version 10; update the GUI, CLI and service together.

### Fixed

- Delete in a focused search field no longer opens an End task confirmation
  for the selected process. The process table owns the shortcut and respects
  text fields, menus and modals.
- A process start time far in the future no longer overflows.
- App-limit integration fixtures now wait for the ping child to be ready before
  changing its jobs or ending its parent, avoiding an intermittent startup race.

## [0.12.0] - 2026-10-20

### Added

- **Tell me when it exits**: right-click a process and pick *Notify me when
  it exits* (a long build, a download, a test run) for a desktop
  notification when it ends; in Telegram, `/wait 1234` or the button under
  `/info` sends a message instead (`/unwait` cancels). Waiting works in a
  report-only bot too.
- Telegram: `/restartsvc NAME` restarts a service.

### Changed

- **Overview**: five cards across the window (CPU, memory, GPU, disk,
  network), each with its last minute as a sparkline; the verdict sits in a
  coloured strip and also warns about swarms; protection, the busiest apps
  and recent actions are cards, the last two side by side on wide windows.
- **Graphs start full**: the service keeps the last five minutes of
  system-wide load, and the window fills its graphs from it when it
  connects instead of starting empty.
- **Settings**: the main service settings (enforcing or monitoring, CPU
  governor thresholds, memory guard, swarm warning, history) are a form;
  editing `config.toml` by hand moved under *Advanced*, and comments in the
  file are kept.
- **Performance**: slimmer disk and GPU engine bars with their figures beside
  them; the system summary moved to the shorter column.

### Fixed

- The swarm card and its notification had a run of spaces in the middle of a
  sentence.

## [0.11.0] - 2026-10-20

### Added

- **Telegram bot**: watch and control the PC from your phone. Set it up in
  Settings → Telegram (a token from @BotFather and the chat IDs allowed to
  use it; the bot tells strangers their ID). `/status`, `/top`, `/mem`,
  `/find`, `/info`, `/ports`, `/services`, `/swarms` and `/events` report;
  `/kill` (by PID or name), `/tree`, `/suspend`, `/resume`, `/free PORT`,
  `/panic`, `/startsvc` and `/stopsvc` act, ending only after a tap on
  *End*, with *End* buttons under every list. Alerts (CPU overload, swarms,
  the kill list) arrive as messages; `/mute` pauses them. The bot runs in
  the service, so it works with no window open; the token is kept in
  `%ProgramData%\RustWTM\telegram.toml`, readable only by administrators,
  and control can be switched off for a report-only bot. Protected Windows
  processes and service hosts are never ended.
- Everything Task Manager does is here too: **Create memory dump file** and
  **Search online** in the process menu (`wtm dump 1234` writes a full
  dump), **Restart** for services, and **Disconnect** / **Sign off** for
  users, each asking first.
- Processes: the details panel lists the ports the selected process listens
  on, with the tool they belong to and who can reach them, and opens the
  Ports tab.

## [0.10.0] - 2026-10-20

### Added

- **Swarm warning**: when one program runs in 150 copies or more (a tool
  that keeps starting helpers and never waits for them, such as a status
  probe launching `powershell.exe` every few seconds), the service sends a
  desktop notification and Overview shows who started them, how much memory
  they hold, and buttons to end them all, only those whose parent exited, or
  to add the program to the kill list. `svchost.exe` and other normal crowds
  are ignored; set `[swarm] min_count` and `exempt` in the configuration.

## [0.9.0] - 2026-10-20

### Added

- **Ports tab**: every listening TCP port (and bound UDP port, on request)
  with its program, PID, CPU and memory, the addresses it listens on, who can
  reach it (*This PC only*, *One network*, *All networks*) and how many
  clients are connected. Sort by any column, search, right-click to end the
  program or its tree, show it in Processes, open `http://localhost:<port>`,
  copy the address or add it to the kill list.
- **Developer tools & servers**: the usual ports of about 75 dev servers,
  databases, caches, queues, search engines, container tools, monitoring and
  AI tools, with whether they are in use and by whom; *Open* and *End* per
  row, *Show free ports too* for the whole catalog, and ports of your own to
  watch (remembered between runs).
- **Free a port**: type a port and end what keeps it busy, every program
  holding it at once. Windows' own ports (System, service hosts) are refused
  with the reason; programs that only relay a port for Docker or WSL carry a
  warning.
- `wtm ports [--udp] [--known]` and `wtm free-port <port> [--yes] [--tree]`.

## [0.8.0] - 2026-10-20

### Added

- **Updates inside the app**: RustWTM checks GitHub for a newer release at
  start and twice a day and shows a bar with *Update now*, which downloads
  the installer, verifies it against the SHA-256 published with the release
  (nothing is installed on a mismatch), runs it (Windows asks for
  administrator rights) and starts the new version. *What's new*, *Later*
  and *Skip this version* are on the bar; *Check for updates* is in About,
  and automatic checks can be turned off in Settings → This window. Copies
  not installed with the MSI get a download link instead.
- Theme choice (System / Light / Dark) in Settings → This window.

### Changed

- **A new look**: rounder, airier widgets in the logo's orange, refined
  light and dark themes, sections on cards.
- Performance: every resource in its own colour with filled graphs, a small
  history graph for each logical processor (as Task Manager shows them),
  coloured engine and disk bars, and a balanced two-column layout.
- Overview: ring gauges for CPU, memory and GPU.

### Fixed

- The taskbar and Alt+Tab show the RustWTM logo instead of the default
  window icon, also when RustWTM starts in the tray or as administrator.

## [0.7.0] - 2026-10-20

### Added

- **Kill list** (new tab, `[[kill]]` in the configuration, `wtm kill-list`):
  programs that may be ended, matched by name and optionally by command line
  (`node.exe` running `vitest`, but no other Node program). With a CPU
  threshold the service ends one that keeps using that much for a while and
  does not recover, optionally only while the whole PC is busy, together with
  the processes it started.
- **Panic button** (status bar, Ctrl+Shift+K, the overload alert,
  `wtm panic --yes`): ends everything on the kill list at once after showing
  what will be ended.
- Rules match on the command line too: `match = { name = "node.exe",
  cmdline = "vitest" }`.
- Right-click a process → *Add to kill list…*.

## [0.6.0] - 2026-10-20

### Added

- **Overload alert**: when the CPU stays at or above 90% for 5 seconds,
  RustWTM pops up above all windows (also from the tray) with the busiest
  apps and one-click fixes: *Calm them all down* (efficiency mode and lower
  priority for everything but the app in use), efficiency mode, limit to one
  core, pause or end per app, and turning on automatic protection. RustWTM
  raises its own priority while the alert is open, so it stays responsive.
  Close (quiet for 5 minutes), snooze for an hour, or turn alerts off;
  threshold and duration are in Settings → This window.
- The window's close button keeps RustWTM running in the tray (Exit is in the
  tray menu); RustWTM starts in the tray at sign-in (`rustwtm --background`),
  both switchable in Settings → This window.
- Starting RustWTM again shows the running one instead of a second copy
  (`--new-instance` starts another anyway).

## [0.5.0] - 2026-10-20

### Added

- **Heat map** in the process list: CPU, memory, disk and GPU cells are
  shaded by how much of the machine they use (Columns → Heat map turns it off).
- Keyboard: ↑ / ↓ move the selection in the process list, Delete asks to end
  the selected process.
- The details panel graphs the selected process's CPU and memory over the
  last two minutes (recorded for every process, so it shows at once).
- The tray icon shows the CPU load as a bar (green, orange from 60%, red from
  90%); its tooltip adds memory use.
- Performance: activity, read and write rate of every physical disk, and the
  load of every GPU engine (3D, Copy, Video decode, ...).
- The Overview says in one sentence how the machine is doing and which app
  is behind a busy CPU or a full memory.
- `wtm list` and `wtm top` add a line with GPU, network and disk rates and
  the busiest disk's activity.
- App limits warn (event and desktop notification) when a budget uses 95% of
  its memory, since its processes then fail to allocate memory; once more
  only after it dropped below 85%.
- The Limits tab shows CPU and memory use as bars against the budget (the
  fullest instance for per-instance budgets), orange from 75%, red from 95%.

### Changed

- The Performance tab is more compact: two sections side by side on wide
  windows, shorter graphs, and only the four busiest GPU engines (the rest on
  hover).
- The Limits, Settings and App history tabs say when the installed service is
  another version (instead of "not running").
- IPC protocol version 7 (system statistics carry disks and GPU engines);
  update the GUI, CLI and service together.

## [0.4.0] - 2026-10-20

### Added

- **Apps view** in the process list (next to List and Tree): one row per app
  with the totals of all its processes, expandable to the processes, with a
  menu that acts on all of them (efficiency mode, free memory, suspend /
  resume, limit, end all with one confirmation).
- Keyboard: Ctrl+F jumps to the process search, Escape clears it.
- **Performance** tab: load of every logical processor, network receive /
  send rate and total disk / I/O read and write rate, each with a history
  graph.
- The window remembers its size and position, the last tab, the navigation
  state, the process list view, sort and columns, and the sort of every other
  table (stored in `%APPDATA%\RustWTM\data\app.ron`).
- **Run new task** (status bar): start a program, folder, document or website,
  optionally with administrator rights.
- **Restart as administrator** (status bar and Overview) when the window runs
  without administrator rights, so every process can be controlled.

### Changed

- IPC protocol version 6 (system statistics carry the new counters); update
  the GUI, CLI and service together.

## [0.3.1] - 2026-10-20

### Fixed

- App limits no longer flood the event log with `Access is denied`: processes
  that already run in a job of their own (sandboxes, build tools such as
  cargo/rustc/git/bun started by an IDE or agent) cannot join an app budget;
  this is now noted once per app and shown as *N outside* in the Limits tab.
  Exited processes are ignored silently.
- Every RustWTM job object allows `CREATE_BREAKAWAY_FROM_JOB`, so limiting
  browsers, IDEs and sandboxed tools never prevents them from starting child
  processes.

### Added

- Collapsible navigation with icons: an icon-only rail on narrow windows (or
  on request), scrollable on short ones.

- Program icons in the process list and on the Overview, read from each
  executable once and cached.
- Left-hand navigation grouped into monitoring, protection and system,
  replacing the crowded tab bar; CPU / memory and the connection badge moved
  to the status bar.
- The Overview explains when the installed service is a different version
  and links the latest release.

## [0.3.0] - 2026-10-20

### Added

- **Overview** tab, the new start page: CPU / memory / GPU at a glance,
  one-click *Turn protection on*, CPU protection levels (Off, Gentle,
  Balanced, Strict), the busiest apps with quick actions (efficiency mode,
  limit, details) and the latest automatic actions.
- **Foreground protection** (`governor.protect_foreground`, on by default): the
  app you are working in, and everything it started, is never throttled and is
  released at once when you switch to it. The RustWTM window reports the
  foreground app; the service accepts reports only for the client's own
  session (IPC protocol 5).
- **About** tab with credits, version and links.
- **Logo and icons**: a gauge logo drawn in code, used for the window, the tray,
  the executables (with version information), the MSI / Apps list and the
  Start Menu shortcut, and the README (`assets/`).
- App history: *Limit <app>…* opens the Limits tab with a CPU budget suggested
  from the app's p95 usage.
- **Memory guard** (`[memory_guard]`, on by default): when RAM stays above
  90%, the biggest background apps have their working sets trimmed (nothing
  is closed or lost); never the foreground app. Manual *Free memory* in the
  process menu and on the Overview, `wtm trim <target>`.
- Limits tab: pick the program from the running apps (busiest first).
- Process list: *Command line* column (shown by default); the search box also
  matches command lines. Command lines are read by the GUI with the user's own
  rights, never sent by the service.
- `wtm list --cmdline` (`-c`) adds a COMMAND LINE column.

### Changed

- IPC protocol 5: the GUI, CLI and service must all be 0.3.

## [0.2.0] - 2026-10-19

### Added

- **App limits** (`[[limit]]`): a CPU (in cores) and/or memory budget per app,
  shared by all of its processes and everything they start (`scope = "group"`),
  or per top-level instance (`scope = "each"`), enforced with job objects by
  the service. New **Limits** tab in the GUI (with *Limit all <name>…* in the
  process menu) and `wtm app-limit set | remove | list`; the process list's
  Limits column shows app-limit membership.
- **Program blocking** (`block = ["app.exe"]`): listed programs never start
  (Image File Execution Options); Windows launches `rustwtm.exe --blocked`
  instead, which notifies the user. Only RustWTM's own entries are touched,
  and they are removed when the service stops.
- **Sorting everywhere**: every list sorts by any column (click a header, click
  again to reverse): Users, Network, Services, Startup apps, App history,
  Limits, managed processes, and the process list (now also by Limits).
- Process list: optional Commit, Working set, GPU memory, Session, Base
  priority and Running-for columns, chosen in the new *Columns* menu.
- **Settings** tab: edit the service's configuration with live validation,
  switch between dry-run and enforcing, save (elevated).
- `wtm config show | default | check <file> | apply <file>`.
- Process details show the command line and the user account (GUI details
  panel with *Copy command line*, `wtm info`); *Open file location* in the
  details panel and the context menu.
- Long-term history records per-app GPU usage; App history (GUI and
  `wtm history`) shows and plots it, and insights flag chronic GPU users
  (`chronic_gpu_percent`, `chronic_gpu_actions`; report only by default).
  Existing history databases are upgraded in place.
- Rule condition `gpu_above`, e.g. to suspend a background GPU miner.
- IPC protocol 4: `GetConfig` and `SetConfig` (admin only; validated, written
  atomically). Clients and service must both be 0.2.

### Changed

- Test line coverage raised to 98% (from 91%); the SCM integration test also
  collects coverage from the installed service process.

### Fixed

- Tree view: processes whose parents form a cycle (reused PIDs with identical
  start times) were missing from the list.
## [0.1.1] - 2026-10-19

### Fixed

- Memory-only job limits (`wtm limit --memory`, GUI *Limit memory*) failed
  with "process no longer exists": the limiter tried to disable a CPU cap that
  was never enabled. Errors other than a missing process are now reported as
  such.
- Clicking a process name (or any text) in a table now selects the row; selectable
  text previously swallowed the click.
- Tables in different tabs no longer share layout state.

### Added

- MSI package (WiX 5) attached to every release: installs the service with
  failure recovery, PATH entry and Start Menu shortcut.
- Headless GUI tests (egui_kittest), engine/IPC end-to-end tests and CLI
  integration tests, and real MSI / install-script installs in CI; line coverage
  is about 90%.

## [0.1.0] - 2026-10-19

First public release.

### Added

- **Process monitoring** via `NtQuerySystemInformation`: CPU %, private
  working set, commit, threads, handles, disk I/O rate, keyed by PID and
  creation time so PID reuse never mixes up processes.
- **GPU usage** per process (busiest engine %, dedicated memory) from the
  GPU performance counters.
- **Process control**: end task, end process tree, suspend / resume,
  priority, CPU affinity, efficiency mode (EcoQoS), hard CPU cap and memory
  cap through job objects. Core Windows processes, critical processes and
  RustWTM itself are always refused.
- **CLI `wtm`**: `list`, `top`, `info`, `kill`, `suspend`, `resume`,
  `priority`, `affinity`, `eco`, `limit`, `history`.
- **Service `wtm-service`** (LocalSystem, auto-start, restarts on failure):
  - CPU governor with sustain / recovery thresholds, step cooldown and an
    escalation ladder.
  - Rule engine (name / path wildcards, sustained CPU / memory conditions);
    unconditional rules react within ~250 ms of a process starting.
  - Long-term insights: per-minute per-app history in SQLite, detection of
    chronic CPU use and steady memory growth (slope + R² fit), automatic
    actions with rollback when a finding clears.
  - Per-source tracking (governor, rule, insight, manual) so one source never
    undoes another's limits; everything is rolled back on stop.
  - Dry-run by default, hot-reloaded `config.toml`, rolling log files,
    in-memory event log.
- **IPC** over a local named pipe with an explicit DACL, anti-squatting,
  server identity verification and admin-only control requests.
- **GUI `rustwtm`**: Processes, Performance, Governor, App history, Startup
  apps, Users, Services, Network and Events tabs; tray icon; desktop
  notifications for notable events.
- **Installer**: `scripts/install.ps1` / `scripts/uninstall.ps1`, release
  archives with SHA-256 checksums built by GitHub Actions.

[Unreleased]: https://github.com/rustwtm/rustwtm/compare/v0.30.1...HEAD
[0.30.1]: https://github.com/rustwtm/rustwtm/compare/v0.30.0...v0.30.1
[0.30.0]: https://github.com/rustwtm/rustwtm/compare/v0.29.0...v0.30.0
[0.29.0]: https://github.com/rustwtm/rustwtm/compare/v0.28.0...v0.29.0
[0.28.0]: https://github.com/rustwtm/rustwtm/compare/v0.27.2...v0.28.0
[0.27.2]: https://github.com/rustwtm/rustwtm/compare/v0.27.1...v0.27.2
[0.27.1]: https://github.com/rustwtm/rustwtm/compare/v0.27.0...v0.27.1
[0.27.0]: https://github.com/rustwtm/rustwtm/compare/v0.26.2...v0.27.0
[0.20.2]: https://github.com/rustwtm/rustwtm/compare/v0.20.1...v0.20.2
[0.20.1]: https://github.com/rustwtm/rustwtm/compare/v0.20.0...v0.20.1
[0.20.0]: https://github.com/rustwtm/rustwtm/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/rustwtm/rustwtm/compare/v0.18.1...v0.19.0
[0.18.1]: https://github.com/rustwtm/rustwtm/compare/v0.18.0...v0.18.1
[0.18.0]: https://github.com/rustwtm/rustwtm/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/rustwtm/rustwtm/compare/v0.15.0...v0.17.0
[0.15.0]: https://github.com/rustwtm/rustwtm/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/rustwtm/rustwtm/compare/v0.13.1...v0.14.0
[0.13.1]: https://github.com/rustwtm/rustwtm/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/rustwtm/rustwtm/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/rustwtm/rustwtm/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/rustwtm/rustwtm/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/rustwtm/rustwtm/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/rustwtm/rustwtm/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/rustwtm/rustwtm/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/rustwtm/rustwtm/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/rustwtm/rustwtm/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/rustwtm/rustwtm/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/rustwtm/rustwtm/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/rustwtm/rustwtm/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/rustwtm/rustwtm/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/rustwtm/rustwtm/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/rustwtm/rustwtm/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/rustwtm/rustwtm/releases/tag/v0.1.0
