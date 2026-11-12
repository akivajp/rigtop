# MSIX package (Microsoft Store)

rigtop's package for the Microsoft Store. The same `rigtop-gui.exe` and `rigtop.exe` as in the zip; the Start menu shows the GUI, and the terminal version is the command `rigtop` in a terminal through an app execution alias (the Store takes one application per package, and an application has one alias, so there is no `rigtop-gui` command). From a package, rigtop:

- keeps the configuration file, the alert histories and the snoozed alerts in the package's folder, `%LOCALAPPDATA%\Packages\<package family name>\LocalState` (Windows redirects a packaged app's writes to `%APPDATA%` and `%LOCALAPPDATA%` into a private copy, which an editor opened from "config…" would not see). Uninstalling removes them;
- does not look for new versions or update itself (the Store brings them). Updating rigtop on remote hosts over SSH still works;
- does not install LibreHardwareMonitor: "set up…" explains the steps and links to the README (Store apps must not install other software that brings a driver of its own).

The package family name is `C2B504BA.rigtop_1raghr5p0ryyw`.

## Building

The release workflow (`.github/workflows/release.yml`) builds the package on every tag, and when run by hand. The artifact `store-msix` holds:

- `rigtop-vX.Y.Z-x64.msix` and `-arm64.msix`: for Partner Center, one per architecture (unsigned; the Store signs what it publishes);
- `rigtop-vX.Y.Z-<arch>-test.msix` and `-test.cer`: copies signed with a throwaway self-signed certificate, for trying the package;
- `wack-report-x64.xml`: the Windows App Certification Kit's report on the x64 package (the checks Partner Center runs). GitHub's Arm runner has no kit; there the workflow only installs the test copy and runs `rigtop` through its alias (as it does on x64).

On Windows with the Windows SDK, the same by hand:

```powershell
cargo build --release -p rigtop -p rigtop-gui
./packaging/msix/build.ps1 -BinDir target/release -Version 0.32.0 -OutDir dist -TestSign
```

`-Arch arm64` names and marks the package for Windows on Arm (with executables built for `aarch64-pc-windows-msvc`).

The images in `Assets/` are rendered from `crates/rigtop-gui/assets/rigtop.svg`, each size on its own (scale-100 to scale-400, and exact sizes for the taskbar). `build.ps1` indexes them with makepri so that Windows picks the right one.

## Trying the test copy

1. Trust the certificate (as an administrator): `Import-Certificate -FilePath rigtop-vX.Y.Z-x64-test.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople`
2. Install, from a signed-in session (over SSH it fails with 0x80070005, access denied): `Add-AppxPackage rigtop-vX.Y.Z-x64-test.msix`
3. Start it from the Start menu, `explorer.exe "shell:AppsFolder\C2B504BA.rigtop_1raghr5p0ryyw!rigtop"`, and the terminal version with `rigtop` in a new terminal.
4. Remove it: `Get-AppxPackage *rigtop* | Remove-AppxPackage`, then the certificate from `Cert:\LocalMachine\TrustedPeople`.

## Publishing on the Store

rigtop is on the Store as [9N6JMV1QNDCK](https://apps.microsoft.com/detail/9n6jmv1qndck). The package's identity (Partner Center > rigtop > Product identity) is in the repository variables that the workflow reads: `MSIX_IDENTITY_NAME`, `MSIX_PUBLISHER`, `MSIX_PUBLISHER_DISPLAY_NAME`.

Submissions go through the Store's submission API, by the shared workflow [akivajp/msstore-submit](https://github.com/akivajp/msstore-submit) (it signs in with GitHub OIDC through this repository's `msstore` environment; no secret is stored):

1. Rewrite `release_notes` ("What's new in this version") in every listing file, [`store/en-us.toml`](store/en-us.toml) and [`store/ja.toml`](store/ja.toml). They hold the Store's texts (description, features, search terms, captions) and are what the Store shows: edit them here, not in Partner Center.
2. Tag the release. The release workflow builds the packages (x64 and Arm64) and makes a **draft** submission with them and the listing files; the log of its "Microsoft Store submission" job shows every field that changes.
3. Look at that log, then submit the draft: `gh workflow run msstore.yml -f command=commit` (or delete it with `-f command=discard`). Partner Center shows the changes of an API submission only once it is committed, and a draft edited or submitted there can no longer be handled through the API.

For the listing alone (no new package): `gh workflow run msstore.yml -f command=submit`, with `-f images=true` to replace the screenshots too (`store/screenshots/`), then `commit` as above. `check` and `show` read the app's submissions and change nothing.

What was entered by hand in Partner Center for the first submission, and cannot be set through the API (pricing, properties, the age rating, the reason for `runFullTrust`), is recorded in [`store-listing.ja.md`](store-listing.ja.md) (in Japanese).

Each version needs a new package version (the workflow takes it from `Cargo.toml`); the Store's last number stays 0.
