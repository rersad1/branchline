# Backend

Environment-local Rust/Axum service for Git, worktrees, terminals and provider adapters.
Desktop uses typed IPC and private worker bootstrap; browser development uses the
loopback HTTP service. Windows repositories use Windows Git; WSL repositories use
Linux Git inside the selected distro.

From `backend/` in Ubuntu WSL, with Rust and Git on PATH:

```sh
export AWM_TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export AWM_CONFIG="$(mktemp -d)/config.json"
cargo run --locked
```

In PowerShell 7 on Windows:

```powershell
$env:AWM_TOKEN = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
$env:AWM_CONFIG = Join-Path ([IO.Path]::GetTempPath()) ('branchline-dev-' + [Guid]::NewGuid() + '/config.json')
cargo run --locked
```

The default address is `127.0.0.1:43871`; set `AWM_PORT` to override it. Give the
runtime token to the browser UI through its password field without writing it into
source files, URLs, logs or frontend environment variables. Ctrl+C stops the server.
Use a different config from the desktop app: each registry has an exclusive lock.
Default runtime configuration lives in the current user's local application config
directory. These runtime files are not source assets and must not be committed.

Run `cargo fmt --check`, `cargo test --locked`,
`cargo clippy --locked --all-targets -- -D warnings` and `cargo build --locked`.
The Python HTTP/worker checks are optional Linux integration harnesses that use
temporary fixtures and support `CARGO_TARGET_DIR`.

The embedded adapter manifest in `../scripts/d7-candidates.json`, terminal shell
scripts in `src/shell/` and fixtures in `tests/` are required source inputs.
See the [root README](../README.md) for platform/build limitations.
