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

1. Update the version in the root `Cargo.toml` and move the **Unreleased**
   changelog entries under the new version.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. The release workflow builds the binaries, packages
   `rustwtm-x86_64-pc-windows-msvc.zip` with a `.sha256` file and publishes
   a GitHub release.
