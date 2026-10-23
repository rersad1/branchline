# Windows desktop

Tauri 2 embeds the frontend and version-matched Linux helper, bundles the Windows
terminal host and routes operations to each project's explicit environment.
It manages worker startup/authentication privately; desktop users do not enter a token.

See the [root README](../README.md) for prerequisites, source builds, packaging,
runtime requirements and validation limitations. `scripts/package.ps1` builds all
inputs from this checkout. Generated helpers, schemas and installers are not committed.

`src/bootstrap.py` and `src/helper_assets.py` are embedded runtime source assets.
The allowlisted commands in `capabilities/` and `permissions/` are product security
configuration, not coding-assistant instructions. Keep them when packaging.
