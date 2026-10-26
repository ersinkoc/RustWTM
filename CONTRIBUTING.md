# Contributing

Thanks for your interest in RustWTM!

## Setup

- Windows 10/11, 64-bit.
- [rustup](https://rustup.rs/); the toolchain is pinned in
  `rust-toolchain.toml` and installed automatically.

```powershell
cargo build --workspace
cargo test --workspace
```

For sensors, install the .NET 8 SDK or newer and run
`./scripts/build-hardware.ps1`. It uses the committed NuGet lockfile and runs
fake-control lease/range checks without changing physical fan speeds.
See [hardware setup](docs/hardware.md) for packaging and live validation limits.

Some tests spawn short-lived child processes (`cmd.exe`, `ping.exe`) and
apply limits to them; they never touch other processes.

## Before opening a pull request

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks.

## Guidelines

- All `unsafe` code lives in `wtm-sys`. Every `unsafe` block needs a
  `// SAFETY:` comment (`clippy::undocumented_unsafe_blocks` is enabled).
- Decision logic belongs in `wtm-policy` and must stay pure (no system calls)
  so it can be unit-tested.
- Anything that changes a process must go through `wtm_core::control`, which
  enforces the protection rules and PID-reuse checks.
- New behaviour that can affect running processes must respect `dry_run`.
- Add an entry under **Unreleased** in `CHANGELOG.md`.

## Releasing

1. Update the root `Cargo.toml`, the hardware provider's project version,
   `Cargo.lock` workspace entries and the README's MSI examples. Move the
   **Unreleased** changelog entries under a dated version heading.
2. Run `scripts/ci/verify-release.ps1 -Tag v0.21.0`, substituting the intended
   tag. It refuses a tag that disagrees with the workspace, hardware provider
   or release notes.
3. Run the format, Clippy, documentation, workspace test and release build
   checks. Push the release commit and wait for CI, including the disposable-VM
   installer and service checks, before tagging that commit.
4. Tag and push: `git tag v0.21.0` and `git push origin v0.21.0`.
5. Wait for the release workflow. It gates publication on release identity,
   format, Clippy, documentation, tests and locked builds, then publishes the
   portable ZIP and MSI with their `.sha256` files. Download the published
   files, verify both digests and smoke-test the extracted CLI and service
   versions before calling the release verified.

Machine-changing installer tests belong on disposable VMs. Never enable them
on the developer's machine as part of a routine local release check.
