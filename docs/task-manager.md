# Windows Task Manager integration

RustWTM can optionally open in place of Windows Task Manager. Enable **Settings
→ This window → Windows Task Manager → Use RustWTM for Ctrl+Shift+Esc** from an
administrator window. The integration applies to all users and is off by
default. A protected installed copy is required; temporary, development and
writable portable paths cannot be registered.

Windows Task Manager entry points, including Ctrl+Shift+Esc, use a fixed
RustWTM launch command. An existing session window is brought forward; otherwise
a visible window starts. Arguments Windows appends for Task Manager are ignored
rather than interpreted as RustWTM startup or process-control switches.

## Restore and recovery

Turn the setting off to return to Windows Task Manager. If the window cannot
open, use an elevated PowerShell terminal:

```powershell
& "$env:ProgramFiles\RustWTM\wtm.exe" task-manager --restore
```

For a custom installation, use that installation's `wtm.exe`. The status command
does not change the machine:

```powershell
wtm task-manager
wtm task-manager --enable
wtm task-manager --restore
```

The commands are mutually exclusive. Enabling and restoring require elevation.
Existing third-party registrations and image-filter settings are not overwritten.
If another application controls the entry, restore it there first and select
**Refresh** in RustWTM. Unknown or unreadable state is reported as an error.

Uninstall removes only the owned debugger command for this installation,
preserving unrelated registry values and third-party replacements. Cleanup also
runs if the service was previously removed. A cleanup failure prevents file
removal so an owned redirect cannot be left pointing to a deleted GUI. Repair
and ordinary upgrades retain the fixed installation path and setting.

## Implementation and validation

The integration uses Windows Image File Execution Options for `taskmgr.exe`,
with a product ownership marker and the quoted command
`"<installation>\rustwtm.exe" --task-manager`. The native 64-bit registry view is
used. Microsoft documents the debugger launch mechanism in
[Debug multi-process applications](https://learn.microsoft.com/en-us/visualstudio/debugger/debug-multiple-processes?view=visualstudio).

Unit tests use scratch HKCU keys and cover ownership, reversal, repeated calls,
foreign entries, image filters, protected-path guards and literal paths. Headless
GUI tests cover setting commands, busy/conflict/error states, forwarded arguments
and compact layouts. Actual system registration, repair and uninstall checks
belong to the explicitly gated disposable Windows CI jobs. Routine local tests
never change the developer host's real Task Manager association.
