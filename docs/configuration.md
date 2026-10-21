# Configuration reference

The service reads `%ProgramData%\RustWTM\config.toml`. It is created with
commented defaults on first start and reloaded automatically when the file
changes. An invalid file is rejected (the previous configuration stays
active) and the error is written to the event log.

Besides editing the file directly, an elevated client can replace it through the
service: the GUI's **Settings** tab or `wtm config apply <file>`. Both validate
the text first and the file is replaced atomically. `wtm config check <file>`
validates without applying; `wtm config default` prints the commented defaults.

All CPU percentages are a share of the **whole machine** (100% = every
logical processor busy). On a 16-thread CPU one fully busy core is 6.25%.

## Top level

| Key | Default | Meaning |
|---|---|---|
| `dry_run` | `true` | Log what would be done without touching processes. Manual commands from the GUI/CLI are always executed. |
| `sample_interval_ms` | `1000` | Sampling period (minimum 100). |
| `history_days` | `30` | Days of per-minute usage history kept. |
| `block` | `[]` | Programs prevented from starting, see below. |

## Blocking programs

```toml
block = ["xmrig.exe", "SomeGame.exe"]
```

Listed programs never start: the service registers them under *Image File
Execution Options*, so Windows runs `rustwtm.exe --blocked` instead, which
shows a notification and exits. Unlike a `kill` rule, the program never runs
at all, not even for a moment.

- Plain file names only (`app.exe`); Windows matches by name, so every copy
  with that name is blocked, wherever it lives. Use a `path` rule with
  `kill` to target one location.
- Applied only when `dry_run = false`. Removed again when the service stops
  or the entry is deleted from the list.
- Entries another program already uses (an existing `Debugger` value) are left
  alone and reported in the event log.
- Core Windows processes, `explorer.exe` and RustWTM itself cannot be
  blocked.

## `[governor]`

Acts when the whole system is overloaded.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | |
| `high_cpu_percent` | `90.0` | System CPU at or above this counts as overloaded. |
| `sustain_secs` | `5` | Overload must last this long before acting. |
| `recover_cpu_percent` | `70.0` | At or below this counts as recovered (must be below `high_cpu_percent`). |
| `recover_secs` | `15` | Recovery must last this long before limits are rolled back. |
| `offender_min_cpu_percent` | `2.0` | Ignore processes below this. |
| `max_offenders` | `3` | Act on at most this many top consumers per tick. |
| `step_cooldown_secs` | `5` | Minimum time between two ladder steps for one process. |
| `ladder` | see below | Escalation steps, applied one at a time. |
| `exempt` | `["explorer.exe", "Taskmgr.exe"]` | Image-name wildcards never touched. |

Default ladder:

```toml
ladder = [
    { priority = "below_normal" },
    "eco_qos",
    { cpu_cap_relative = 0.5 },
    { cpu_cap_relative = 0.25 },
]
```

## Actions

Used by `ladder`, rules and insights.

| Action | Effect |
|---|---|
| `{ priority = "idle" \| "below_normal" \| "normal" \| "above_normal" }` | Priority class. |
| `"eco_qos"` | Efficiency mode (power throttling). |
| `{ cpu_cap = 10 }` | Hard cap at 10% of the machine (job object). |
| `{ cpu_cap_relative = 0.5 }` | Cap at half of the usage observed when first acted on (governor and rules only). |
| `{ memory_cap_mb = 4096 }` | Committed memory limit per process. |
| `"suspend"` | Freeze all threads. |
| `"kill"` | Terminate. |

When several sources act on one process, the most restrictive state wins
(lowest priority, tightest caps). When a source rolls back, only its own
actions are removed.

## `[[rule]]`

```toml
[[rule]]
name = "Keep Chrome renderers in check"
match = { name = "chrome.exe" }            # and/or path = "C:\\Games\\*\\*.exe"
when = { cpu_above = 20, for_secs = 10 }   # optional; also memory_above_mb, gpu_above
actions = ["eco_qos", { cpu_cap_relative = 0.5 }]
```

- `match.name` / `match.path`: case-insensitive wildcards (`*`, `?`).
- `when`: all given thresholds must hold for `for_secs`. `cpu_above` is a
  share of the whole machine, `memory_above_mb` is private committed memory,
  `gpu_above` is the utilization of the process's busiest GPU engine. Without `when` the
  rule fires as soon as the process is seen (within ~250 ms of start).
- A rule fires once per process instance.

## `[memory_guard]`

When physical memory stays above `high_percent`, the biggest background apps
have their working sets trimmed: their pages move to the standby list, so the
app in front of you gets the RAM. Nothing is closed and no data is lost; a
trimmed app reloads pages as it needs them. The foreground app is never
trimmed. Manual equivalent: *Free memory* in the GUI, `wtm trim <target>`.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | |
| `high_percent` | `90.0` | Memory in use that counts as pressure... |
| `sustain_secs` | `15` | ...for this long. |
| `max_processes` | `3` | Trim at most this many processes per round, largest first. |
| `min_working_set_mb` | `300` | Ignore smaller processes. |
| `cooldown_secs` | `300` | Do not trim the same process again within this time. |
| `exempt` | `[]` | Image-name wildcards never trimmed. |

Applied only when `dry_run = false`.

## `[reserve]`

CPU and memory kept for RustWTM itself, so it stays usable on a machine that
is pinned at 100% CPU or out of RAM.

**Rescue core.** While system CPU stays at or above `trigger_cpu_percent` for
`sustain_secs`, every other program is moved off one core with CPU Sets (the
last physical core of the fastest tier; a single logical processor on machines
with fewer than 8). RustWTM, core Windows processes (including `dwm.exe`, which
draws the screen) and `exempt` keep every core. Programs that chose their own
CPU Sets are left alone. Once CPU stays at or below `release_cpu_percent` for
`release_secs`, the core is handed back; it is also handed back when the
service stops. The GUI shows *Rescue core* in the status bar and on the
Overview while it is engaged. In dry run it is only reported.

**Always on.** The service runs at high priority, opted out of EcoQoS, with
`working_set_mb` of memory that Windows never pages out. The window runs above
normal priority with 64 MB kept resident and its UI thread ahead of its
background workers.

| Key | Default | Meaning |
|---|---|---|
| `rescue_core` | `true` | Keep one core free while overloaded. |
| `trigger_cpu_percent` | `95.0` | System CPU that engages the rescue core... |
| `sustain_secs` | `3` | ...when it lasts this long. |
| `release_cpu_percent` | `75.0` | System CPU that hands the core back... |
| `release_secs` | `10` | ...when it lasts this long. |
| `working_set_mb` | `48` | Service memory never paged out; `0` = none, at most `1024`. |
| `exempt` | `[]` | Image-name wildcards that keep every core. |

Windows has no hard CPU or memory reservation for ordinary programs; these
are the strongest guarantees it offers. A machine that has run out of commit
(RAM plus page file) can still refuse new allocations.

## `[game_mode]`

While a full-screen app is in front (a game, a full-screen video or
presentation, as reported by the RustWTM window), background apps using at
least `background_cpu_percent` of the machine get `actions`; everything is
rolled back when the full-screen app leaves the foreground, and when the
service stops. Only apps in the full-screen app's own session are calmed,
never services (session 0) or other signed-in users' apps. The Overview and
the status bar show *Game mode* meanwhile.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | |
| `background_cpu_percent` | `2.0` | Background apps using at least this share of the machine are calmed. |
| `actions` | below-normal priority + efficiency mode | `cpu_cap_relative` is not allowed. |
| `power_plan` | `false` | Switch to the High performance power plan meanwhile and back to the previous plan afterwards. |
| `exempt` | `["obs*.exe", "discord.exe", "audiodg.exe", "explorer.exe"]` | Image-name wildcards never calmed. |

Applied only when `dry_run = false`.

## `[[limit]]` — app limits

```toml
[[limit]]
name = "node.exe"      # image-name wildcard
cpu_cores = 2          # all node.exe processes together: at most 2 cores
memory_gb = 8          # ...and 8 GB of committed memory

[[limit]]
name = "zen.exe"
memory_gb = 8
scope = "each"         # every top-level zen.exe with its child processes: 8 GB each
```

An app limit is a budget shared by every process of the app **and every
process it starts** (helpers, renderers, child tools), whatever their names.
It is enforced with a Windows job object: new child processes join it
automatically.

| Key | Default | Meaning |
|---|---|---|
| `name` | required | Image-name wildcard (`node.exe`, `chrome*.exe`). |
| `cpu_cores` | none | Hard CPU cap in logical processors (`0.5`, `2`, ...). |
| `memory_gb` | none | Cap on the committed memory of the whole budget. Allocations beyond it fail, so the app may report out-of-memory errors or exit. |
| `scope` | `"group"` | `"group"`: one budget for all matching processes. `"each"`: one budget per top-level instance, shared with its descendants. |

At least one of `cpu_cores` / `memory_gb` is required. Limits are applied
only when `dry_run = false` (dry-run logs what would be limited), and lifted
when an entry is removed or the service stops. Edit them in the GUI's
**Limits** tab (right-click a process → *Limit all <name>…*) or with
`wtm app-limit set node.exe --cores 2 --memory 8G [--each]`.

## `[insights]`

Long-term analysis of the usage history.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | |
| `window_hours` | `24` | Look-back window. |
| `evaluate_every_minutes` | `10` | Analysis period. |
| `min_minutes` | `60` | Ignore apps seen for fewer minutes in the window. |
| `chronic_cpu_percent` | `10.0` | Average CPU over the window that flags a chronic hog. |
| `chronic_actions` | below-normal priority + efficiency mode | Applied to every instance while flagged. |
| `growth_mb_per_hour` | `200.0` | Memory growth rate that flags a leak suspect... |
| `growth_min_mb` | `2048` | ...once the app uses at least this much... |
| `growth_min_fit` | `0.7` | ...and the trend is steady (R² of a linear fit). |
| `growth_actions` | `[]` | Empty = report only (event + notification). |
| `chronic_gpu_percent` | `30.0` | Average GPU (busiest engine, all instances, capped at 100) that flags a chronic GPU user. |
| `chronic_gpu_actions` | `[]` | Empty = report only. Priority and efficiency mode do not limit GPU work. |
| `handle_growth_per_hour` | `1000.0` | Steady handle growth (fit: `growth_min_fit`) that flags a handle leak, report only... |
| `handle_min` | `10000` | ...once the app holds at least this many handles. |
| `thread_growth_per_hour` | `50.0` | Steady thread growth that flags a thread leak, report only... |
| `thread_min` | `500` | ...once the app runs at least this many threads. |
| `exempt` | `["explorer.exe", "Taskmgr.exe"]` | |
| `protect_foreground` | `true` | Never throttle the app in the foreground and the processes it started (reported by the RustWTM window while it runs). |

The GUI's Overview tab offers four ready-made levels that rewrite the
thresholds and the ladder: **Off** (`enabled = false`), **Gentle** (95% for
10 s, top 2 apps: below-normal priority, efficiency mode), **Balanced** (the
defaults above) and **Strict** (80% for 3 s, top 5 apps: idle priority,
efficiency mode, caps down to a tenth).

`cpu_cap_relative` is not allowed in insights; use `cpu_cap`.

## Telegram bot

The bot has its own file, `telegram.toml` next to `config.toml`, because it
holds the bot token: the service limits it to SYSTEM and Administrators.
Edit it in Settings → Telegram (elevated); the service follows changes
within seconds.

```toml
enabled = true
token = "123456789:AA…"     # from @BotFather
chats = [123456789]          # chat IDs allowed to use the bot
notify = true                # forward alerts (CPU overload, swarms, kill list)
control = true               # allow ending, suspending and resuming
```

Messages from other chats are answered once with their chat ID and
otherwise ignored. Ending anything asks for a tap on *End* first; protected
Windows processes and service hosts are never ended. Send `/help` to the bot
for every command.

## Files

| Path | Content |
|---|---|
| `%ProgramData%\RustWTM\config.toml` | Configuration |
| `%ProgramData%\RustWTM\telegram.toml` | Telegram bot settings (administrators only) |
| `%ProgramData%\RustWTM\history.sqlite` | Per-minute usage history |
| `%ProgramData%\RustWTM\logs\` | Daily rolling service logs |
