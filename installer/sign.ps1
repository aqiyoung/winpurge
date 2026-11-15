# Signs a WinPurge binary with a code-signing certificate.
#
# Default: a throw-away, self-signed certificate generated on the runner.
#   ./sign.ps1 -Path <file>
#
# With a bought certificate (OV/EV), pass the PFX instead:
#   ./sign.ps1 -Path <file> -PfxPath C:\certs\winpurge.pfx -PfxPassword hunter2
#
# WHAT A SELF-SIGNED SIGNATURE IS GOOD FOR
#   Proves the file has not been modified since CI built it, and shows a
#   consistent publisher name instead of "Unknown publisher".
#
# WHAT IT DOES NOT DO
#   It does NOT silence the Windows SmartScreen warning. SmartScreen only
#   trusts certificates issued by a CA that is in the Microsoft root store, and
#   it additionally needs reputation earned over time. Use an OV/EV certificate
#   from a trusted CA for that.
#
# -Optional makes a signing failure a warning instead of a build failure.

param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string]$PfxPath,
    [string]$PfxPassword,
    [switch]$Optional
)

$ErrorActionPreference = "Stop"

function Sign-File {
    param([string]$Target)

    $password = if ($PfxPassword) { $PfxPassword } else { "winpurge-selfsigned" }

    if ($PfxPath -and (Test-Path $PfxPath)) {
        $pfx = $PfxPath
        Write-Host "using certificate $pfx"
    }
    else {
        $pfx = Join-Path $env:TEMP "winpurge-selfsigned.pfx"
        if (-not (Test-Path $pfx)) {
            $cert = New-SelfSignedCertificate `
                -DnsName "WinPurge Contributors" `
                -CertStoreLocation "Cert:\CurrentUser\My" `
                -Type CodeSigningCert `
                -HashAlgorithm SHA256 `
                -KeyLength 2048 `
                -NotAfter (Get-Date).AddYears(10)
            $secure = ConvertTo-SecureString -String $password -Force -AsPlainText
            Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $secure | Out-Null
            Write-Host "created self-signed code-signing certificate"
        }
    }

    $signtool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName

    if ($signtool) {
        Write-Host "signing $Target with $signtool"
        & $signtool sign /f $pfx /p $password /fd SHA256 /td SHA256 /a /tr http://timestamp.digicert.com $Target
        if ($LASTEXITCODE -ne 0) {
            throw "signtool failed with exit code $LASTEXITCODE"
        }
    }
    else {
        Write-Host "signtool not found; using Set-AuthenticodeSignature for $Target"
        $secure = ConvertTo-SecureString -String $password -Force -AsPlainText
        $cert = Get-PfxCertificate -FilePath $pfx -Password $secure
        $result = Set-AuthenticodeSignature -FilePath $Target -Certificate $cert `
            -HashAlgorithm SHA256 -TimestampServer http://timestamp.digicert.com
        if ($result.Status -ne "Valid") {
            throw "signing status: $($result.Status) - $($result.StatusMessage)"
        }
    }

    Write-Host "signed $Target"
}

try {
    Sign-File -Target $Path
}
catch {
    if ($Optional) {
        Write-Host "warning: could not sign $Path - $($_.Exception.Message)"
        exit 0
    }
    throw
}
