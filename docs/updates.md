# Updates and GitHub releases

Starting with 0.2.0, Branchline checks stable GitHub releases eight seconds after
workspace startup and then every six hours while open. The toolbar's
**Обновления Branchline** button also provides a manual check. An offline check
does not interrupt work; it can be retried later.

When a newer version exists, choose **Скачать обновление**. Once it has been
downloaded and verified, choose **Сохранить и перезапустить**. Changes and the
workspace layout must save successfully before installation can start. Running
app-owned agents, terminals and WSL workers are closed through normal cleanup;
their processes are not automatically relaunched after updating.

The fixed source is
`https://github.com/rersad1/branchline/releases/latest/download/latest.json`.
The updater accepts only the announced Windows x64 installer in this repository.
HTTPS, the embedded public key and a signed version number are required; downgrades
are disabled. Unsigned, altered or differently versioned packages are rejected.
The browser UI cannot supply another endpoint, executable path or signing key.

The first upgrade from 0.1.1 is manual, because that version has no updater.
Download the installer from GitHub Releases and install it over the existing copy.
Workspace settings are retained. Do not distribute raw executables as replacements
for the installer: resources and installation identity must also match.

## Publish the next version

1. Run `node scripts/set-version.mjs 0.2.1` and add `docs/releases/0.2.1.md`.
2. Commit and push the reviewed source to the public repository.
3. Create and push the matching stable tag: `git tag v0.2.1` followed by
   `git push origin v0.2.1`.
4. Wait for **Windows release** in GitHub Actions. It builds the Linux helper on
   Ubuntu 24.04, tests the application, builds/signs the Windows NSIS installer,
   verifies its signature and signed version, uploads all assets to a draft,
   and publishes the completed release as latest.

The release contains the installer, its `.sig`, `latest.json` and
`SHA256SUMS.txt`. A release is not published when a build or verification fails.
Retry a failed workflow against the same tag; an existing draft can be completed.
Published releases are treated as immutable: fixes require a new version and tag.
For a manual workflow retry, select the matching version tag rather than main.

`TAURI_SIGNING_PRIVATE_KEY` is a repository Actions secret. Keep this private key
outside source control and retain a secure backup. Replacing the public key would
break the update trust of existing installations. Only `desktop/updater.pub` and
the matching public key in `tauri.conf.json` belong in the repository. Standard
builds use Tauri CLI 2.12.4, which binds the application's version into the signature.

The updater signature is independent of Windows Authenticode. These installers
do not currently have a Windows publisher certificate.
