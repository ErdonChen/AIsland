[CmdletBinding()]
param(
  [Parameter(Mandatory = $true, Position = 0)]
  [string]$ArtifactPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Resolve-SignTool {
  $command = Get-Command signtool.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($null -ne $command) {
    return $command.Source
  }

  $candidatePaths = @(
    "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\signtool.exe"
  )
  foreach ($candidatePath in $candidatePaths) {
    if (Test-Path -LiteralPath $candidatePath -PathType Leaf) {
      return $candidatePath
    }
  }

  $sdkBin = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
  if (Test-Path -LiteralPath $sdkBin -PathType Container) {
    $sdkSignTool = Get-ChildItem -LiteralPath $sdkBin -Filter signtool.exe -File -Recurse |
      Where-Object { $_.FullName -match '\\(?:x64|x86)\\signtool\.exe$' } |
      Sort-Object FullName -Descending |
      Select-Object -First 1
    if ($null -ne $sdkSignTool) {
      return $sdkSignTool.FullName
    }
  }

  throw 'SignTool was not found on this Windows runner.'
}

$resolvedArtifact = (Resolve-Path -LiteralPath $ArtifactPath).Path
if ([System.IO.Path]::GetExtension($resolvedArtifact) -notin @('.exe', '.dll', '.msi')) {
  throw "Refusing to Authenticode-sign unsupported artifact '$resolvedArtifact'."
}

$thumbprint = ($env:AUTHENTICODE_CERTIFICATE_SHA1 -replace '\s', '').ToUpperInvariant()
if ($thumbprint -notmatch '^[0-9A-F]{40}$') {
  throw 'AUTHENTICODE_CERTIFICATE_SHA1 must be a 40-character SHA-1 certificate thumbprint.'
}

try {
  $timestampUri = [uri]$env:AUTHENTICODE_TIMESTAMP_URL
} catch {
  throw 'AUTHENTICODE_TIMESTAMP_URL must be a valid RFC 3161 timestamp URL.'
}
if ($timestampUri.Scheme -notin @('http', 'https')) {
  throw 'AUTHENTICODE_TIMESTAMP_URL must use http or https.'
}

$certificate = Get-ChildItem -Path Cert:\CurrentUser\My -CodeSigningCert |
  Where-Object { ($_.Thumbprint -replace '\s', '').ToUpperInvariant() -eq $thumbprint } |
  Select-Object -First 1
if ($null -eq $certificate) {
  throw "The code-signing certificate '$thumbprint' is not loaded in Cert:\CurrentUser\My. Provision the approved cloud signer before running Tauri."
}
if (-not $certificate.HasPrivateKey) {
  throw "The certificate '$thumbprint' is visible but its cloud-backed private key is unavailable."
}

$signTool = Resolve-SignTool
& $signTool sign /fd SHA256 /tr $timestampUri.AbsoluteUri /td SHA256 /sha1 $thumbprint /v $resolvedArtifact
if ($LASTEXITCODE -ne 0) {
  throw "SignTool failed for '$resolvedArtifact' with exit code $LASTEXITCODE."
}

$signature = Get-AuthenticodeSignature -LiteralPath $resolvedArtifact
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
  throw "The signed artifact '$resolvedArtifact' is not trusted: $($signature.Status)."
}
if ($null -eq $signature.SignerCertificate) {
  throw "The signed artifact '$resolvedArtifact' has no signer certificate."
}
if ($null -eq $signature.TimeStamperCertificate) {
  throw "The signed artifact '$resolvedArtifact' has no RFC 3161 timestamp certificate."
}

Write-Host "Verified trusted Authenticode signature and timestamp for '$resolvedArtifact'."
