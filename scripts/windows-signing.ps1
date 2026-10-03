param(
    [ValidateSet('prepare', 'sign', 'verify')]
    [string]$Operation,
    [string[]]$FilePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RequiredSigningEnvironment([string]$Name) {
    $value = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($value)) {
        throw "Windows signing requires $Name."
    }
    return $value.Trim()
}

function Install-SigningPackage([string]$Name, [string]$Version, [string]$Sha256, [string]$Directory) {
    $archive = Join-Path $Directory "$Name.zip"
    $destination = Join-Path $Directory $Name
    Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/$Name/$Version/$Name.$Version.nupkg" -OutFile $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $Sha256) {
        throw "Downloaded signing package $Name $Version has an unexpected SHA256 digest."
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $destination
    Remove-Item -LiteralPath $archive
}

function Get-SigningToolPath {
    $directory = Get-RequiredSigningEnvironment 'REPOLA_WINDOWS_SIGNING_DIRECTORY'
    return Join-Path $directory 'microsoft.windows.sdk.buildtools/bin/10.0.26100.0/x64/signtool.exe'
}

function Invoke-SigningTool([string[]]$Arguments) {
    & (Get-SigningToolPath) @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "SignTool failed with exit code $LASTEXITCODE."
    }
}

function Assert-WindowsSignature([string]$Path) {
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $signature = Get-AuthenticodeSignature -LiteralPath $resolved
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode') {
        throw "Invalid embedded Authenticode signature on $resolved`: $($signature.Status)."
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Missing Authenticode timestamp on $resolved."
    }
    Invoke-SigningTool -Arguments @('verify', '/pa', '/all', '/tw', $resolved)
}

function Initialize-WindowsSigning {
    $endpoint = [Uri](Get-RequiredSigningEnvironment 'AZURE_TRUSTED_SIGNING_ENDPOINT')
    if ($endpoint.Scheme -ne 'https' -or -not $endpoint.Host.EndsWith('.codesigning.azure.net') -or
        -not $endpoint.IsDefaultPort -or $endpoint.UserInfo -or $endpoint.Query -or $endpoint.Fragment -or $endpoint.AbsolutePath -ne '/') {
        throw 'Windows signing requires an Azure Artifact Signing regional HTTPS endpoint.'
    }
    $metadata = @{
        Endpoint = $endpoint.AbsoluteUri
        CodeSigningAccountName = Get-RequiredSigningEnvironment 'AZURE_TRUSTED_SIGNING_ACCOUNT_NAME'
        CertificateProfileName = Get-RequiredSigningEnvironment 'AZURE_TRUSTED_SIGNING_CERTIFICATE_PROFILE_NAME'
        ExcludeCredentials = @(
            'EnvironmentCredential', 'WorkloadIdentityCredential', 'ManagedIdentityCredential',
            'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential',
            'AzurePowerShellCredential', 'AzureDeveloperCliCredential', 'InteractiveBrowserCredential'
        )
    }
    $directory = Join-Path (Get-RequiredSigningEnvironment 'RUNNER_TEMP') "repola-windows-signing-$([Guid]::NewGuid())"
    New-Item -Path $directory -ItemType Directory | Out-Null
    Install-SigningPackage 'microsoft.windows.sdk.buildtools' '10.0.26100.4188' '180deb372659029864c10a0c04787833234d64aacd1d2c0661d2c00295d8e022' $directory
    Install-SigningPackage 'microsoft.artifactsigning.client' '1.0.128' '74bd7d27e6ce1051409c38d9b46bc8df0400ecd643d51ffbf2ac00869061e40b' $directory
    $metadata | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'metadata.json') -Encoding utf8NoBOM
    "REPOLA_WINDOWS_SIGNING_DIRECTORY=$directory" | Out-File -LiteralPath (Get-RequiredSigningEnvironment 'GITHUB_ENV') -Append -Encoding utf8
}

function Sign-WindowsFile([string]$Path) {
    $directory = Get-RequiredSigningEnvironment 'REPOLA_WINDOWS_SIGNING_DIRECTORY'
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $dlib = Join-Path $directory 'microsoft.artifactsigning.client/bin/x64/Azure.CodeSigning.Dlib.dll'
    $metadata = Join-Path $directory 'metadata.json'
    Invoke-SigningTool -Arguments @('sign', '/fd', 'SHA256', '/tr', 'http://timestamp.acs.microsoft.com', '/td', 'SHA256', '/dlib', $dlib, '/dmdf', $metadata, '/d', 'Repola', $resolved)
    Assert-WindowsSignature $resolved
}

if ($MyInvocation.InvocationName -ne '.') {
    if (-not $IsWindows) { throw 'Windows signing must run on Windows.' }
    if (-not $Operation) { throw 'Specify a Windows signing operation.' }
    if ($Operation -eq 'prepare') {
        Initialize-WindowsSigning
    } else {
        if (-not $FilePath -or $FilePath.Count -eq 0) { throw 'Specify at least one file to sign or verify.' }
        foreach ($file in $FilePath) {
            if ($Operation -eq 'sign') { Sign-WindowsFile $file }
            else { Assert-WindowsSignature $file }
        }
    }
}
