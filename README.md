# Branchline

Branchline is a local desktop workspace for Git repositories and worktrees. It has a
Rust backend, a React/TypeScript interface and a Tauri 2 Windows shell. The application
still uses the internal package and executable names `agent-workspace-*`.

Features include:

- Separate Windows and explicitly selected WSL environments, project registration
  and discovery of real Git worktrees.
- Worktree creation/removal, Git status, staged/unstaged/branch diffs and a bundled
  Monaco editor with scoped working-file writes and autosave conflict handling.
- PowerShell/Bash terminal tabs, retained terminal screens, coding-agent launchers
  and version-gated provider/ACP integrations.
- Workspace persistence, external editor integration, attention notifications and
  process/system memory diagnostics.

## Platform scope and limitations

The desktop target is **Windows 11 x64** with WebView2 and Git for Windows on PATH.
WSL projects use an explicitly named Linux x86_64 environment with GNU glibc >=2.39,
Python >=3.10 at `/usr/bin/python3`, Git at `/usr/bin/git`, Bash at `/bin/bash`, the
GNU loader at `/lib64/ld-linux-x86-64.so.2` and an executable temporary directory.
Publication builds/checks use **WSL2 Ubuntu 26.04 x86_64**. The runtime does not ban
other distro names or release numbers; full compatibility with other installations,
older glibc, musl and other architectures is not established. There is no supported
native Linux/macOS GUI build.
WSL is optional at runtime for Windows-only projects, but required to build the
embedded Linux helper for the desktop package.

This is early, unsigned software. Windows local repository roots must fit the
application's path limit (less than 260 UTF-16 units); UNC/SMB roots are unsupported.
Working-file editing is limited to supported UTF-8 regular files up to 1 MiB.
Provider integrations check exact compatible versions and may refuse other versions;
the candidate manifest is not a claim that every provider/platform combination works.
Providers require their own local installation and authentication. Windows notification
click handling, Ubuntu Remote WSL editor acceptance and additional provider combinations
still have validation gaps. Native folder picker selection has not been revalidated.
Memory diagnostics are observations, not an automatic memory reclamation tool.

## Build prerequisites

- Rust/Cargo with `rustfmt` and `clippy` (Rust 1.99.0 used for publication checks).
- Visual Studio C++ build tools and a Windows SDK for the MSVC desktop target.
- Node.js 22.12+ and npm (Node 24.21.0 used for publication checks).
- Git for Windows, WebView2 and WSL2 Ubuntu as described above.
- Rust/Cargo and Git on PATH **inside the selected Ubuntu distro**.
- Network access for locked Cargo/npm dependencies and first-use Tauri/NSIS tools.

Use ordinary tool installations on PATH. No private repository, developer-specific
directory, pre-existing cache or checked-in binary is required. All four Cargo/npm
dependency lockfiles are retained.

## Build and run the Windows desktop app

From the repository root in PowerShell, with Ubuntu's exact installed distro name:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop/scripts/package.ps1 -Distro Ubuntu
```

This builds the Linux helper from `backend/`, the Windows terminal host, the frontend
and the NSIS installer. The output is under `desktop/target/release/bundle/nsis/`
unless you explicitly set `CARGO_TARGET_DIR`. Run the generated installer, or run
`desktop/target/release/agent-workspace-desktop.exe` with its generated
`terminal-host-windows.exe` resource alongside it. The package contains the UI and
helper; Node, Cargo and a manually started backend are development dependencies.
The installer may download WebView2 if it is missing. No existing releases/assets
are included in this source repository.

For a development build, generate both helper inputs and the UI before compiling:

```powershell
wsl --distribution Ubuntu --exec bash -l scripts/build-d14-worker.sh
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-d4-terminal-host.ps1
npm --prefix frontend ci
npm --prefix frontend run build
cargo build --locked --manifest-path desktop/Cargo.toml
```

The shell scripts compute paths from the checkout. The Linux helper defaults to
`backend/target-linux/`, separately from native Windows output. `CARGO_TARGET_DIR`,
when set, is a build output override; use separate Windows and Linux directories.
An absent `desktop/assets/worker-linux-x86_64` intentionally fails desktop compilation:
it is embedded with `include_bytes!` and must be rebuilt after backend changes.

## Browser development

See [backend](backend/README.md) for local server setup and
[frontend](frontend/README.md) for Vite. Use an isolated `AWM_CONFIG` and a fresh
`AWM_TOKEN`; the desktop handles its own private worker authentication. Vite's
development gateway is loopback-only and is not a hosted production web service.

## Checks

```powershell
cargo fmt --manifest-path backend/Cargo.toml --check
cargo test --locked --manifest-path backend/Cargo.toml
cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path desktop/Cargo.toml --check
cargo test --locked --manifest-path desktop/Cargo.toml
cargo clippy --locked --manifest-path desktop/Cargo.toml --all-targets -- -D warnings
npm --prefix frontend run typecheck
npm --prefix frontend test -- --maxWorkers=2
npm --prefix frontend run build
node scripts/tests/d7-acp.test.mjs
```

After building the helper, `python3 desktop/tests/helper_assets.py .` runs Linux
bootstrap/authentication/cleanup fixtures. Its noexec case requires a writable
`/run/lock` mounted with `noexec`; it changes no mount or system settings. Simulated
distro metadata in that test does not establish a separate distro installation pass.

Backend integration tests create temporary repositories and anonymous fixtures.
Run Linux checks inside the selected Ubuntu WSL environment. CI's Linux backend job
uses a synthetic distro identity and does not establish real desktop/WSL acceptance.
The `d7-candidates.json` file is required product runtime configuration; the adjacent
ACP probe/tests exercise protocol fixtures without contacting an authenticated provider.

No project license has been added to this publication. Dependency licenses remain
those of their respective owners; see [third-party information](THIRD_PARTY_NOTICES.md).
