# MSIX package (Microsoft Store)

rigtop's package for the Microsoft Store. The same `rigtop-gui.exe` and `rigtop.exe` as in the zip; the Start menu shows the GUI, and the terminal version is the command `rigtop` in a terminal through an app execution alias (the Store takes one application per package, and an application has one alias, so there is no `rigtop-gui` command). From a package, rigtop:

- keeps the configuration file, the alert histories and the snoozed alerts in the package's folder, `%LOCALAPPDATA%\Packages\<package family name>\LocalState` (Windows redirects a packaged app's writes to `%APPDATA%` and `%LOCALAPPDATA%` into a private copy, which an editor opened from "config…" would not see). Uninstalling removes them;
- does not look for new versions or update itself (the Store brings them). Updating rigtop on remote hosts over SSH still works;
- does not install LibreHardwareMonitor: "set up…" explains the steps and links to the README (Store apps must not install other software that brings a driver of its own).

The package family name is `C2B504BA.rigtop_1raghr5p0ryyw`.

## Building

The release workflow (`.github/workflows/release.yml`) builds the package on every tag, and when run by hand. The artifact `store-msix` holds:

- `rigtop-vX.Y.Z-x64.msix`: for Partner Center (unsigned; the Store signs what it publishes);
- `rigtop-vX.Y.Z-x64-test.msix` and `-test.cer`: a copy signed with a throwaway self-signed certificate, for trying the package;
- `wack-report.xml`: the Windows App Certification Kit's report on the package (the checks Partner Center runs).

On Windows with the Windows SDK, the same by hand:

```powershell
cargo build --release -p rigtop -p rigtop-gui
./packaging/msix/build.ps1 -BinDir target/release -Version 0.32.0 -OutDir dist -TestSign
```

The images in `Assets/` are rendered from `crates/rigtop-gui/assets/rigtop.svg`, each size on its own (scale-100 to scale-400, and exact sizes for the taskbar). `build.ps1` indexes them with makepri so that Windows picks the right one.

## Trying the test copy

1. Trust the certificate (as an administrator): `Import-Certificate -FilePath rigtop-vX.Y.Z-x64-test.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople`
2. Install, from a signed-in session (over SSH it fails with 0x80070005, access denied): `Add-AppxPackage rigtop-vX.Y.Z-x64-test.msix`
3. Start it from the Start menu, `explorer.exe "shell:AppsFolder\C2B504BA.rigtop_1raghr5p0ryyw!rigtop"`, and the terminal version with `rigtop` in a new terminal.
4. Remove it: `Get-AppxPackage *rigtop* | Remove-AppxPackage`, then the certificate from `Cert:\LocalMachine\TrustedPeople`.

## Publishing on the Store

The identity (Partner Center > rigtop > Product identity) is in the repository variables that the workflow reads: `MSIX_IDENTITY_NAME`, `MSIX_PUBLISHER`, `MSIX_PUBLISHER_DISPLAY_NAME`. Upload `rigtop-vX.Y.Z-x64.msix` from the `store-msix` artifact to the submission's **Packages**; what goes into the rest of the submission is in [`store-listing.ja.md`](store-listing.ja.md) (in Japanese, for the Japanese Partner Center).

Each version needs a new package version (the workflow takes it from `Cargo.toml`); the Store's last number stays 0.
