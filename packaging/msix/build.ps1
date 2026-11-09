<#
.SYNOPSIS
  Builds the MSIX package of rigtop for the Microsoft Store.

.DESCRIPTION
  Lays out rigtop-gui.exe and rigtop.exe, the images and AppxManifest.xml (with the
  identity and the version filled in), indexes the images with makepri
  (for their scale-* and targetsize-* variants) and packs them with
  makeappx. Needs the Windows SDK (GitHub's windows-latest has it).

  The package is not signed: Partner Center signs what it publishes. With
  -TestSign, a copy signed with a new self-signed certificate is made too,
  with that certificate (.cer) to trust before installing the copy on a
  test PC (Add-AppxPackage).

.EXAMPLE
  ./packaging/msix/build.ps1 -BinDir target/release -Version 0.32.0 -OutDir dist -TestSign
#>
param(
  # The folder with rigtop-gui.exe and rigtop.exe.
  [Parameter(Mandatory)] [string] $BinDir,
  # The app's version (x.y.z); the package's is x.y.z.0 (the Store wants 0 last).
  [Parameter(Mandatory)] [string] $Version,
  # Where the package (and the test copy) go.
  [Parameter(Mandatory)] [string] $OutDir,
  # Partner Center > the app > Product identity: Package/Identity/Name.
  [string] $IdentityName = "C2B504BA.rigtop",
  # Product identity: Package/Identity/Publisher.
  [string] $Publisher = "CN=14FA0ABF-60AB-41EA-B780-C6A9B9C7147D",
  # Product identity: Package/Properties/PublisherDisplayName.
  [string] $PublisherDisplayName = "Akiva Miura",
  # Also make a copy signed with a self-signed certificate, for testing.
  [switch] $TestSign
)

$ErrorActionPreference = "Stop"

# Runs a tool of the SDK and stops on failure.
function Invoke-Tool([string] $Tool, [string[]] $Arguments) {
  & $Tool @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$(Split-Path -Leaf $Tool) failed ($LASTEXITCODE)" }
}

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "version $Version is not x.y.z" }

# The newest SDK's tools.
$bin = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" |
  Sort-Object { [version]$_.Directory.Parent.Name } | Select-Object -Last 1
if (-not $bin) { throw "the Windows SDK (makeappx.exe) is not installed" }
$sdk = $bin.Directory.FullName
Write-Host "Windows SDK tools: $sdk"

$here = $PSScriptRoot
$work = Join-Path ([System.IO.Path]::GetTempPath()) "rigtop-msix-$PID"
$layout = Join-Path $work "layout"
New-Item -ItemType Directory -Force $layout, $OutDir | Out-Null

# The files of the package.
foreach ($exe in "rigtop-gui.exe", "rigtop.exe") {
  Copy-Item (Join-Path $BinDir $exe) (Join-Path $layout $exe)
}
Copy-Item (Join-Path $here "Assets") $layout -Recurse
$manifest = (Get-Content -Raw (Join-Path $here "AppxManifest.xml")).
  Replace("@IDENTITY_NAME@", $IdentityName).
  Replace("@PUBLISHER@", [System.Security.SecurityElement]::Escape($Publisher)).
  Replace("@PUBLISHER_DISPLAY_NAME@", [System.Security.SecurityElement]::Escape($PublisherDisplayName)).
  Replace("@VERSION@", "$Version.0")
Set-Content -Encoding utf8 (Join-Path $layout "AppxManifest.xml") $manifest

# resources.pri: lets Windows pick Assets\Square44x44Logo.scale-200.png and
# the like for the names in the manifest.
$priconfig = Join-Path $work "priconfig.xml"
Invoke-Tool "$sdk\makepri.exe" @("createconfig", "/cf", $priconfig, "/dq", "en-US", "/pv", "10.0.0", "/o")
# The default configuration splits the scaled images into resources.scale-*.pri
# for resource packages (bundles); one package needs them all in resources.pri.
[xml] $config = Get-Content -Raw $priconfig
$packaging = $config.SelectSingleNode("/resources/packaging")
if ($packaging) { [void] $packaging.ParentNode.RemoveChild($packaging) }
$config.Save($priconfig)
Invoke-Tool "$sdk\makepri.exe" @("new", "/pr", $layout, "/cf", $priconfig,
  "/mn", (Join-Path $layout "AppxManifest.xml"), "/of", (Join-Path $layout "resources.pri"), "/o")

# The package for the Store.
$name = "rigtop-v$Version-x64"
$msix = Join-Path $OutDir "$name.msix"
Invoke-Tool "$sdk\makeappx.exe" @("pack", "/d", $layout, "/p", $msix, "/o")
Write-Host "Package: $msix"

if ($TestSign) {
  # A self-signed code signing certificate with the package's publisher as
  # its subject (they must match), used once and removed.
  $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher `
    -KeyUsage DigitalSignature -FriendlyName "rigtop test signing" `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
  try {
    $password = ConvertTo-SecureString -String "test" -Force -AsPlainText
    $pfx = Join-Path $work "test.pfx"
    Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $password | Out-Null
    Export-Certificate -Cert $cert -FilePath (Join-Path $OutDir "$name-test.cer") | Out-Null
    $test = Join-Path $OutDir "$name-test.msix"
    Copy-Item $msix $test
    Invoke-Tool "$sdk\signtool.exe" @("sign", "/fd", "SHA256", "/f", $pfx, "/p", "test", $test)
    Write-Host "Test copy: $test (trust $name-test.cer first)"
  } finally {
    Remove-Item "Cert:\CurrentUser\My\$($cert.Thumbprint)"
  }
}

Remove-Item -Recurse -Force $work
