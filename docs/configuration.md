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
| `exempt` | `["explorer.exe", "Taskmgr.exe"]` | |

`cpu_cap_relative` is not allowed in insights; use `cpu_cap`.

## Files

| Path | Content |
|---|---|
| `%ProgramData%\RustWTM\config.toml` | Configuration |
| `%ProgramData%\RustWTM\history.sqlite` | Per-minute usage history |
| `%ProgramData%\RustWTM\logs\` | Daily rolling service logs |
