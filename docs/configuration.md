# Configuration reference

The service reads `%ProgramData%\RustWTM\config.toml`. It is created with
commented defaults on first start and reloaded automatically when the file
changes. An invalid file is rejected (the previous configuration stays
active) and the error is written to the event log.

All CPU percentages are a share of the **whole machine** (100% = every
logical processor busy). On a 16-thread CPU one fully busy core is 6.25%.

## Top level

| Key | Default | Meaning |
|---|---|---|
| `dry_run` | `true` | Log what would be done without touching processes. Manual commands from the GUI/CLI are always executed. |
| `sample_interval_ms` | `1000` | Sampling period (minimum 100). |
| `history_days` | `30` | Days of per-minute usage history kept. |

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
when = { cpu_above = 20, for_secs = 10 }   # optional; also memory_above_mb
actions = ["eco_qos", { cpu_cap_relative = 0.5 }]
```

- `match.name` / `match.path`: case-insensitive wildcards (`*`, `?`).
- `when`: all given thresholds must hold for `for_secs`. Without `when` the
  rule fires as soon as the process is seen (within ~250 ms of start).
- A rule fires once per process instance.

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
| `exempt` | `["explorer.exe", "Taskmgr.exe"]` | |

`cpu_cap_relative` is not allowed in insights; use `cpu_cap`.

## Files

| Path | Content |
|---|---|
| `%ProgramData%\RustWTM\config.toml` | Configuration |
| `%ProgramData%\RustWTM\history.sqlite` | Per-minute usage history |
| `%ProgramData%\RustWTM\logs\` | Daily rolling service logs |
