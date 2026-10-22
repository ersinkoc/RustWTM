"""PreToolUse guard: refuses code or commands that starve the host of a
system resource. A test that reserved nearly the whole address space as
thread stacks crashed Windows three times (bugcheck 0x1A / 0x3453), so such
code is stopped before it is written or run. See CLAUDE.md.

Reads the hook payload on stdin, scans every string in `tool_input`, and
exits 2 (block, reason on stderr) on a match; 0 otherwise.
"""

import json
import re
import sys

# Each pattern names one way to starve the machine.
PATTERNS = [
    (
        r"\bexhaust\w*[\s_]*(the[\s_]+)?(address[\s_]*space|memory|handles?|threads?|commit|pool|stack)",
        "exhausting a system resource",
    ),
    (
        r"stack_size\s*\(\s*(1\s*<<\s*(2[6-9]|[3-6]\d)|\d{9,}|[a-z_]\w*\s*\))",
        "a thread stack of 64 MiB or more (or of a computed size)",
    ),
    (
        r"(VirtualAlloc\w*|reserve|with_capacity|vec!\s*\[[^\]]*;)\s*\(?[^)\n]*1\s*<<\s*(3[4-9]|[4-6]\d)",
        "a reservation of 16 GiB or more",
    ),
    (
        r"(?s)loop\s*\{.{0,400}?(thread::(Builder|spawn)|CreateThread|OpenProcess|CreateEvent\w*|DuplicateHandle|VirtualAlloc).{0,400}?Err\(\s*_?\w*\s*\)\s*=>\s*(break|size\s*/=)",
        "a loop that takes threads, handles or memory until the system refuses",
    ),
    (
        r"\b(EmptyWorkingSet|SetProcessWorkingSetSize\w*)\s*\([^)]*GetCurrentProcess",
        "squeezing this process's own working set",
    ),
]


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings(v)


def main():
    try:
        payload = json.load(sys.stdin)
    except ValueError:
        return 0
    for text in strings(payload.get("tool_input", {})):
        for pattern, what in PATTERNS:
            m = re.search(pattern, text, re.IGNORECASE)
            if m:
                snippet = " ".join(m.group(0).split())[:120]
                print(
                    f"Blocked: {what} ({snippet!r}). Code like this crashed "
                    "Windows three times on this machine. Prove failure paths "
                    "through a seam (a spawn/alloc function passed in, as "
                    "run_client in crates/wtm-service/src/ipc_server.rs does); "
                    "see CLAUDE.md. If real exhaustion is unavoidable, ask the "
                    "user: it may only run in a VM.",
                    file=sys.stderr,
                )
                return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
