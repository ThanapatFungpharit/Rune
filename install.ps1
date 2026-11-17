<#
.SYNOPSIS
Install, update or remove the prebuilt rune-tui on Windows.

.DESCRIPTION
    .\install.ps1                          # newest release into %LOCALAPPDATA%\Programs\rune
    .\install.ps1 -Version v0.2.0          # a given release
    .\install.ps1 -Prefix D:\Tools\rune    # somewhere else
    .\install.ps1 -Archive rune-tui-x86_64-pc-windows-msvc.zip   # no download
    .\install.ps1 -Uninstall               # with the same -Prefix as the install

Every archive is checked against the release's SHA256SUMS and the program is
run once before anything is put in place; nothing is left behind in a
temporary directory, whatever happens. Your session, messages, settings and
plugins (%LOCALAPPDATA%\rune, %APPDATA%\rune) are never touched.

If scripts are blocked on this machine, run it as
    powershell -ExecutionPolicy Bypass -File install.ps1 [OPTIONS]

Windows PowerShell 5.1 or PowerShell 7.
#>
[CmdletBinding()]
param(
    # Release to install, e.g. v0.2.0 (default: latest).
    [string]$Version = 'latest',
    # Install under PREFIX\bin (default: %LOCALAPPDATA%\Programs\rune).
    [string]$Prefix = '',
    # Install this rune-tui-<target>.zip instead of downloading; checked against
    # SHA256SUMS or ARCHIVE.sha256 beside it.
    [string]$Archive = '',
    # With -Archive: allow one that has no checksum file.
    [switch]$NoVerify,
    # GitHub repository to download from (default: ThanapatFungpharit/Rune, or $env:RUNE_REPO).
    [string]$Repo = '',
    # Do not add PREFIX\bin to your user PATH.
    [switch]$NoModifyPath,
    # Remove rune-tui from the prefix (your data stays).
    [switch]$Uninstall,
    # Show this help.
    [switch]$Help
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is many times slower with a progress bar

function Say([string]$Text = '') { Write-Host $Text }
function Warn([string]$Text) { Write-Warning "install.ps1: $Text" }
# Thrown, never `exit`: run as a script block (irm ... | iex), exit would close the user's console.
function Fail([string]$Text) { throw "install.ps1: $Text" }

if ($Help) {
    Say @'
Usage: install.ps1 [OPTIONS]

Install the prebuilt rune-tui for Windows (x86_64; also runs on ARM64 Windows
11 under emulation) from a GitHub release.

Options:
  -Version TAG     release to install, e.g. v0.2.0 (default: latest)
  -Prefix DIR      install under DIR\bin (default: %LOCALAPPDATA%\Programs\rune)
  -Archive FILE    install this rune-tui-<target>.zip instead of downloading;
                   checked against SHA256SUMS or FILE.sha256 beside it
  -NoVerify        with -Archive: allow one that has no checksum file
  -Repo OWNER/NAME GitHub repository to download from (default:
                   ThanapatFungpharit/Rune; or $env:RUNE_REPO)
  -NoModifyPath    do not add DIR\bin to your user PATH
  -Uninstall       remove rune-tui from the prefix (your data stays)
  -Help            show this help

A private repository needs the GitHub CLI, signed in (`gh auth login`); it is
used automatically when it is.
'@
    return
}

if ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows) {
    Fail 'this installer is for Windows; on Linux or macOS use install.sh'
}

# ---------------------------------------------------------------- options

if (-not $Repo) { $Repo = if ($env:RUNE_REPO) { $env:RUNE_REPO } else { 'ThanapatFungpharit/Rune' } }
if ($Repo -notmatch '^[^/\s]+/[^/\s]+$') { Fail "-Repo: expected OWNER/NAME, got '$Repo'" }
if ($Version -match '^[0-9]') { $Version = "v$Version" }
if ($Version -ne 'latest' -and $Version -notmatch '^v[0-9]') {
    Fail "-Version: expected a tag like v0.2.0 or latest, got '$Version'"
}
if ($NoVerify -and -not $Archive) { Fail '-NoVerify only applies to -Archive: downloads are always checked' }

# ---------------------------------------------------------------- platform

$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
    'AMD64' { }
    # Windows 11 on ARM runs x86_64 programs under emulation; there is no native build yet.
    'ARM64' { Say 'Note: no native ARM64 build yet; installing the x86_64 one, which Windows 11 runs under emulation.' }
    default { Fail "no prebuilt rune-tui for $arch Windows (x86_64 is supported); see README.md to build it" }
}
$target = 'x86_64-pc-windows-msvc'
$name = "rune-tui-$target"

# Full path, without a trailing separator. Relative paths are taken from the
# current PowerShell location, not the process's working directory.
function Full-Path([string]$Path) {
    $p = [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location -PSProvider FileSystem).ProviderPath, $Path))
    $root = [IO.Path]::GetPathRoot($p)
    if ($p.Length -gt $root.Length) { $p = $p.TrimEnd('\', '/') }
    return $p
}
# Is $Path the same as, or inside, $Dir (case-insensitive, like Windows)?
function Is-Within([string]$Path, [string]$Dir) {
    return $Path.Equals($Dir, [StringComparison]::OrdinalIgnoreCase) -or
        $Path.StartsWith($Dir.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
}

if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is not set; cannot tell where your user programs go' }
if (-not $Prefix) { $Prefix = Join-Path $env:LOCALAPPDATA 'Programs\rune' }
$Prefix = Full-Path $Prefix
if ($Prefix -eq [IO.Path]::GetPathRoot($Prefix)) { Fail "-Prefix $Prefix (a drive root) is not allowed" }

# rune-tui's own data: never installed into, never removed.
$dataDirs = @()
foreach ($d in @($env:LOCALAPPDATA, $env:APPDATA)) {
    if ($d) { $dataDirs += (Full-Path (Join-Path $d 'rune')) }
}
foreach ($d in $dataDirs) {
    if (Is-Within $Prefix $d) {
        Fail "-Prefix $Prefix is rune-tui's data folder ($d) or inside it; choose another prefix"
    }
}

# What gets installed, relative to the prefix.
$Files = @('bin\rune-tui.exe', 'LICENSE.txt', 'README-windows.txt')
# Present when this installer added PREFIX\bin to the user PATH (so -Uninstall takes it out again).
$PathMark = '.rune-path-added'
$binDir = Join-Path $Prefix 'bin'
$exe = Join-Path $binDir 'rune-tui.exe'

function Data-Note {
    Say 'Your login, messages, settings and plugins were not touched:'
    Say "  $($dataDirs -join ', ')"
}

# Older copies renamed away while they were running (see Put-File); removable once they are not.
function Remove-Leftovers {
    if (Test-Path -LiteralPath $binDir -PathType Container) {
        Get-ChildItem -LiteralPath $binDir -Force -Filter 'rune-tui.exe.rune-*' -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
    }
}

# ---------------------------------------------------------------- user PATH

# Read and written in the registry as stored: [Environment]::GetEnvironmentVariable
# would expand %VARIABLES% in the other entries and write them back expanded.
function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    if (-not $key) { return @{ Value = ''; Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString } }
    try {
        $value = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = if ($null -ne $key.GetValue('Path')) { $key.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
        return @{ Value = [string]$value; Kind = $kind }
    } finally { $key.Close() }
}
function Set-UserPath([string]$Value, $Kind) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        if ($Value) { $key.SetValue('Path', $Value, $Kind) } else { $key.DeleteValue('Path', $false) }
    } finally { $key.Close() }
    # Tell Explorer, so newly opened terminals see the change without signing out.
    try {
        if (-not ('RuneInstall.Native' -as [type])) {
            Add-Type -Namespace RuneInstall -Name Native -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        [void][RuneInstall.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
    } catch {
        Warn 'could not announce the PATH change: open a new terminal (or sign out and in) to use it'
    }
}
function Path-Entries([string]$Value) { return @($Value -split ';' | Where-Object { $_ -ne '' }) }
function Same-Dir([string]$Entry, [string]$Dir) {
    $e = [Environment]::ExpandEnvironmentVariables($Entry).Trim().Trim('"').TrimEnd('\')
    return $e.Equals($Dir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
}

# ---------------------------------------------------------------- uninstall

if ($Uninstall) {
    $removed = $false
    foreach ($f in $Files) {
        $p = Join-Path $Prefix $f
        if (Test-Path -LiteralPath $p) {
            try { Remove-Item -LiteralPath $p -Force }
            catch { Fail "cannot remove $p (is rune-tui still running? close it first): $($_.Exception.Message)" }
            Say "removed $p"
            $removed = $true
        }
    }
    Remove-Leftovers
    $mark = Join-Path $Prefix $PathMark
    if (Test-Path -LiteralPath $mark) {
        $up = Get-UserPath
        # Only our entry goes; the others stay as they are written.
        $entries = @($up.Value -split ';')
        $kept = @($entries | Where-Object { -not ($_ -ne '' -and (Same-Dir $_ $binDir)) })
        if ($kept.Count -ne $entries.Count) {
            Set-UserPath ($kept -join ';') $up.Kind
            Say "removed $binDir from your user PATH"
        }
        Remove-Item -LiteralPath $mark -Force
    }
    # The folders too, when nothing else is in them.
    foreach ($d in @($binDir, $Prefix)) {
        if ((Test-Path -LiteralPath $d -PathType Container) -and
            -not (Get-ChildItem -LiteralPath $d -Force | Select-Object -First 1)) {
            Remove-Item -LiteralPath $d -Force
        }
    }
    if (-not $removed) { Say "rune-tui is not installed under $Prefix (give the -Prefix it was installed with)" }
    if ($removed) {
        $other = Get-Command rune-tui -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($other -and -not (Is-Within $other.Source $Prefix)) {
            Warn "another rune-tui is still on your PATH: $($other.Source)"
        }
    }
    Data-Note
    return
}

# ---------------------------------------------------------------- helpers

# Runs a program and returns its exit code and output (native stderr must not
# become a PowerShell error under $ErrorActionPreference = 'Stop').
function Run-Program([string]$File, [string[]]$Arguments) {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $File
    $psi.Arguments = ($Arguments | ForEach-Object { '"' + ($_ -replace '"', '\"') + '"' }) -join ' '
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $proc = [Diagnostics.Process]::Start($psi)
    $outTask = $proc.StandardOutput.ReadToEndAsync()
    $errTask = $proc.StandardError.ReadToEndAsync()
    if (-not $proc.WaitForExit(60000)) {
        try { $proc.Kill() } catch { }
        return @{ Code = -1; Out = ''; Err = 'did not finish within 60 s' }
    }
    $proc.WaitForExit() # flushes the redirected output
    return @{ Code = $proc.ExitCode; Out = $outTask.Result.Trim(); Err = $errTask.Result.Trim() }
}

function Sha256([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Have([string]$Command) { return [bool](Get-Command $Command -CommandType Application -ErrorAction SilentlyContinue) }

# ---------------------------------------------------------------- workspace

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("rune-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
$staged = New-Object 'System.Collections.Generic.List[string]'
try {

    # ------------------------------------------------------------ fetch

    $useGh = $false
    if (-not $Archive -and (Have 'gh')) {
        $useGh = (Run-Program (Get-Command gh -CommandType Application | Select-Object -First 1).Source @('auth', 'status', '--hostname', 'github.com')).Code -eq 0
    }
    # Windows PowerShell 5.1 may still default to TLS 1.0.
    try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }

    function Fetch([string]$File) {
        if ($Version -eq 'latest') { $url = "https://github.com/$Repo/releases/latest/download/$File" }
        else { $url = "https://github.com/$Repo/releases/download/$Version/$File" }
        if ($useGh) {
            $gh = (Get-Command gh -CommandType Application | Select-Object -First 1).Source
            $ghArgs = @('release', 'download')
            if ($Version -ne 'latest') { $ghArgs += $Version }
            $ghArgs += @('--repo', $Repo, '--pattern', $File, '--dir', $tmp)
            $r = Run-Program $gh $ghArgs
            if ($r.Code -ne 0) {
                if ($r.Err) { Write-Host ('  ' + ($r.Err -replace "`n", "`n  ")) }
                Fail "cannot download $File from release $Version of $Repo"
            }
            return
        }
        try {
            Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $File) -UseBasicParsing -MaximumRedirection 10
        } catch {
            Fail ("cannot download $url`n  ($($_.Exception.Message))`n  (no such release or file for $target, or the repository is private:`n" +
                "  install the GitHub CLI and run 'gh auth login', or download the archive`n  yourself and pass -Archive)")
        }
    }

    $sums = Join-Path $tmp 'sums'
    if ($Archive) {
        $ArchivePath = Full-Path $Archive
        if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) { Fail "no such file: $Archive" }
        $archiveName = Split-Path -Leaf $ArchivePath
        Copy-Item -LiteralPath $ArchivePath -Destination (Join-Path $tmp $archiveName)
        $dir = Split-Path -Parent $ArchivePath
        if (Test-Path -LiteralPath (Join-Path $dir 'SHA256SUMS') -PathType Leaf) {
            Copy-Item -LiteralPath (Join-Path $dir 'SHA256SUMS') -Destination $sums
        } elseif (Test-Path -LiteralPath "$ArchivePath.sha256" -PathType Leaf) {
            Copy-Item -LiteralPath "$ArchivePath.sha256" -Destination $sums
        } elseif (-not $NoVerify) {
            Fail "no SHA256SUMS or $archiveName.sha256 beside $Archive to check it against (-NoVerify skips the check)"
        }
        Say "Installing rune-tui from $Archive"
    } else {
        $archiveName = "$name.zip"
        Say "Downloading rune-tui ($Version) for $target"
        Fetch $archiveName
        Fetch 'SHA256SUMS'
        Move-Item -LiteralPath (Join-Path $tmp 'SHA256SUMS') -Destination $sums
    }
    $zipPath = Join-Path $tmp $archiveName

    # ------------------------------------------------------------ verify

    if (Test-Path -LiteralPath $sums) {
        # Lines are "<hash>  <file>" (or "<hash> *<file>"): find ours by name.
        $want = $null
        foreach ($line in Get-Content -LiteralPath $sums) {
            $parts = $line.Trim() -split '\s+', 2
            if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -ceq $archiveName) { $want = $parts[0].ToLowerInvariant(); break }
        }
        if (-not $want) { Fail "the checksum list has no entry for $archiveName" }
        $got = Sha256 $zipPath
        if ($want -ne $got) {
            Fail "checksum mismatch for ${archiveName}: expected $want, got $got (corrupted or tampered download; nothing was installed)"
        }
        Say 'Checksum OK'
    } else {
        Warn "not checking ${archiveName}: no checksum (-NoVerify)"
    }

    # Only rune-tui-<target>\ and the files we install inside: no absolute
    # paths, no "..", nothing else.
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $expected = @{}
    foreach ($f in @('bin/rune-tui.exe', 'LICENSE.txt', 'README.md', 'README-windows.txt')) { $expected[$f] = $true }
    $zip = $null
    try {
        try { $zip = [IO.Compression.ZipFile]::OpenRead($zipPath) }
        catch { Fail "$archiveName does not look like a rune-tui archive (not a zip file)" }
        $entries = @($zip.Entries)
        if ($entries.Count -eq 0) { Fail "$archiveName is empty" }
        $top = ($entries[0].FullName -replace '\\', '/').Split('/')[0]
        if ($top -notmatch '^rune-tui-[^/]+-[^/]+$') { Fail "$archiveName does not look like a rune-tui archive" }
        if ($top -ne $name) { Fail "$archiveName is built for $($top.Substring(9)), but this machine needs $target" }
        $wanted = @{}
        foreach ($e in $entries) {
            $p = $e.FullName -replace '\\', '/'
            $rel = if ($p.StartsWith("$top/")) { $p.Substring($top.Length + 1) } else { $null }
            $isDir = $p.EndsWith('/')
            if ($p -eq $top -or $p -eq "$top/") { continue }
            if ($null -eq $rel -or $p -match '(^|/)\.\.(/|$)' -or $p -match ':' -or $p.StartsWith('/') -or
                ($isDir -and $rel -ne 'bin/') -or (-not $isDir -and -not $expected.ContainsKey($rel))) {
                Fail "$archiveName holds an unexpected path ($($e.FullName)); nothing was installed"
            }
            if (-not $isDir) {
                if ($wanted.ContainsKey($rel)) { Fail "$archiveName holds $rel twice; nothing was installed" }
                $wanted[$rel] = $e
            }
        }
        $src = Join-Path $tmp 'x'
        New-Item -ItemType Directory -Path (Join-Path $src 'bin') | Out-Null
        foreach ($f in $Files) {
            $rel = $f -replace '\\', '/'
            if (-not $wanted.ContainsKey($rel)) { Fail "$archiveName is missing $rel" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($wanted[$rel], (Join-Path $src $f), $false)
        }
    } finally {
        if ($zip) { $zip.Dispose() }
    }

    # Run it once before installing: catches a damaged or wrong-architecture
    # build, with the old install still in place.
    $r = Run-Program (Join-Path $src 'bin\rune-tui.exe') @('--version')
    if ($r.Code -ne 0) {
        if ($r.Err) { Write-Host ('  ' + ($r.Err -replace "`n", "`n  ")) }
        Fail "the prebuilt rune-tui does not run here (exit code $($r.Code)); build it from source (README.md)"
    }
    $newVersion = $r.Out
    if ($newVersion -notmatch '^rune-tui [0-9]') { Fail "unexpected output from rune-tui --version: $newVersion" }
    if ($Version -ne 'latest' -and "rune-tui $($Version.Substring(1))" -ne $newVersion) {
        Fail "release $Version holds $newVersion; nothing was installed"
    }

    # ------------------------------------------------------------ install

    $oldVersion = $null
    if (Test-Path -LiteralPath $exe -PathType Leaf) {
        try { $o = Run-Program $exe @('--version'); if ($o.Code -eq 0) { $oldVersion = $o.Out } } catch { }
    }

    try { New-Item -ItemType Directory -Force -Path $binDir | Out-Null }
    catch { Fail "cannot create ${binDir}: $($_.Exception.Message) (choose another -Prefix)" }
    Remove-Leftovers

    # Copy beside the target, then rename over it. A running rune-tui.exe cannot
    # be replaced or deleted, but it can be renamed away: it is removed on the
    # next install or uninstall once it has quit.
    $stamp = [Guid]::NewGuid().ToString('N').Substring(0, 8)
    function Put-File([string]$Rel) {
        $dest = Join-Path $Prefix $Rel
        $new = "$dest.rune-new.$stamp"
        $staged.Add($new)
        try {
            Copy-Item -LiteralPath (Join-Path $src $Rel) -Destination $new -Force
            if (Test-Path -LiteralPath $dest) {
                try { [IO.File]::Delete($dest) }
                catch { Move-Item -LiteralPath $dest -Destination "$dest.rune-old.$stamp" -Force }
            }
            Move-Item -LiteralPath $new -Destination $dest
        } catch {
            Fail "cannot install ${dest}: $($_.Exception.Message)"
        }
    }
    # The program last, so it never runs beside older files.
    foreach ($f in $Files) { if ($f -ne 'bin\rune-tui.exe') { Put-File $f } }
    Put-File 'bin\rune-tui.exe'
    # A "downloaded from the internet" mark would make Windows ask before every start.
    try { Unblock-File -LiteralPath $exe } catch { }

    # ------------------------------------------------------------ check

    $r = Run-Program $exe @('--version')
    if ($r.Code -ne 0) { Fail "$exe does not run after installing" }
    if ($r.Out -ne $newVersion) { Fail "$exe reports '$($r.Out)', expected '$newVersion'" }

    if ($oldVersion -and $oldVersion -ne $newVersion) { Say "Updated $oldVersion -> $newVersion in $Prefix" }
    else { Say "Installed $newVersion in $Prefix" }
    foreach ($f in $Files) { Say "  $(Join-Path $Prefix $f)" }

    # ------------------------------------------------------------ PATH

    $up = Get-UserPath
    $onUserPath = @(Path-Entries $up.Value | Where-Object { Same-Dir $_ $binDir }).Count -gt 0
    if (-not $onUserPath) {
        $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
        $onUserPath = @(Path-Entries $machine | Where-Object { Same-Dir $_ $binDir }).Count -gt 0
    }
    if (-not $onUserPath) {
        if ($NoModifyPath) {
            Say ''
            Say "$binDir is not on your PATH; add it to run rune-tui by name (or install without -NoModifyPath)."
        } else {
            $value = if (-not $up.Value) { $binDir } elseif ($up.Value.EndsWith(';')) { $up.Value + $binDir } else { $up.Value + ';' + $binDir }
            Set-UserPath $value $up.Kind
            New-Item -ItemType File -Force -Path (Join-Path $Prefix $PathMark) | Out-Null
            Say ''
            Say "Added $binDir to your user PATH; open a new terminal to run rune-tui by name."
        }
    }
    # This session too.
    if (-not (@(Path-Entries $env:Path | Where-Object { Same-Dir $_ $binDir }).Count -gt 0) -and -not $NoModifyPath) {
        $env:Path = $env:Path.TrimEnd(';') + ';' + $binDir
    }
    $first = Get-Command rune-tui -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($first -and -not $first.Source.Equals($exe, [StringComparison]::OrdinalIgnoreCase)) {
        Say ''
        Warn "'rune-tui' runs $($first.Source), which comes earlier on your PATH than $binDir"
    }

    # Optional programs rune-tui uses when they are there.
    $missing = @()
    if (-not (Have 'mpv')) { $missing += 'mpv (video and audio)' }
    if (-not (Have 'git')) { $missing += 'git (plugins from Git)' }
    if ($missing.Count -gt 0) {
        Say ''
        Say 'Optional, not found:'
        foreach ($m in $missing) { Say "  $m" }
    }

    Say ''
    Say "rune-tui also needs LINE's ltsm.wasm (not distributed with it): it is found in"
    Say 'Chrome, Edge, Brave, Chromium or Vivaldi when the LINE extension is installed'
    Say "there, or put it in $(Join-Path $env:LOCALAPPDATA 'rune\ltsm.wasm'); see 'Getting"
    Say "ltsm.wasm' in README.md. Run rune-tui (in Windows Terminal) to log in."
} finally {
    # Half-copied files from an interrupted install (see Put-File), and the workspace.
    foreach ($s in $staged) { Remove-Item -LiteralPath $s -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
