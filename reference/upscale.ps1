#Requires -Version 7.4
<#
.SYNOPSIS
    Batch-upscale anime to 4K (vs-mlrt) and encode to AV1 (SVT-AV1), with per-file analysis,
    language filtering, EAC3 companion audio and Japanese-audio / English-subs defaults.

.DESCRIPTION
    Keep upscale.ps1, upscale.vpy and analyze.py in one folder. From a folder of rips:

        upscale.ps1 -Plan     # what it would do, nothing encoded
        upscale.ps1           # do it

    Missing tools are installed on first run. Output goes to <rips folder>\upscaled, with logs,
    analysis cache and comparison images in subfolders. See README.md for every option.

    CG / 3D films: put { "Content": "cg" } in upscale-settings.json in the folder (or a parent
    folder). Those files are upscaled by Topaz Video AI 7.x (Proteus, piped straight into the
    AV1 encoder, no intermediate file), with English audio and no subtitles on by default.

.EXAMPLE
    upscale.ps1 -Plan
.EXAMPLE
    upscale.ps1 -CompareGrain
.EXAMPLE
    upscale.ps1 -Path .\ep01.mkv -Start -1 -Frames 720
.EXAMPLE
    upscale.ps1 -UpdateTools    # install or upgrade all tools; run this first
#>
param(
    [string]$Path = '.',
    [string]$OutDir,          # default: <rips folder>\upscaled

    # anime = 2D anime (vs-mlrt, analysis, Japanese audio / English subs by default)
    # cg    = 3D / CG animation films (Topaz Proteus, English audio, no default subtitles)
    # Usually set once per folder: upscale-settings.json with { "Content": "cg" }
    [ValidateSet('anime', 'cg')][string]$Content = 'anime',
    # auto = vs-mlrt for anime, Topaz for cg
    [ValidateSet('auto', 'vs', 'topaz')][string]$Upscaler = 'auto',
    # Topaz tvai_up settings (output size is added by the script). Default = the "A Prores"
    # preset: Proteus 4, relative to auto, as logged by Topaz Video AI 7.1.5
    [string]$TopazFilter = 'model=prob-4:scale=0:preblur=0.25:noise=0.75:details=0.75:halo=0.15:blur=0.5:compression=0.5:estimate=8:blend=0.75:device=0:vram=1:instances=1',
    [string]$TopazPath,       # Topaz Video AI install folder; default C:\Program Files\Topaz Labs LLC\Topaz Video AI

    [ValidateSet('cugan', 'onnx')][string]$Model,
    [string]$Onnx,
    [ValidateSet('trt', 'cuda', 'dml', 'ncnn')][string]$Backend,
    [ValidateSet(-1, 0, 3)][int]$Noise = -1,
    # Source matrix; empty = detect (output is always converted to BT.709)
    [ValidateSet('', '709', '170m', '470bg')][string]$Matrix = '',
    [int]$Height = 2160,

    # Quality options. Any of these (and Model/Onnx/Noise/Crf/Preset/SvtParams) can also be
    # set per show in an upscale-settings.json file in the rips folder.
    # auto = analyse each file (native-resolution descale, denoise strength) and use the
    # folder's majority so a season looks consistent; episode = per file only; off = none
    [ValidateSet('auto', 'episode', 'off')][string]$Analysis = 'auto',
    # auto = detect and crop black bars (pillarbox / letterbox / overscan) before upscaling
    [ValidateSet('auto', 'off')][string]$Crop = 'auto',
    [ValidateSet('auto', 'on', 'off')][string]$Ivtc = 'auto',
    [int]$NativeHeight = 0,
    [ValidateSet('bilinear', 'bicubic', 'lanczos', 'spline36')][string]$NativeKernel = 'bicubic',
    [double]$NativeB = 0,
    [double]$NativeC = 0.5,
    [int]$NativeTaps = 3,
    [switch]$Temporal,
    [int]$TemporalStrength = 0,
    [switch]$Deband,

    [int]$Crf = 14,
    [int]$Preset = 5,
    # Anime encoder settings, in svt-av1-params (key=value:key=value) form
    [string]$SvtParams = 'enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:adaptive-film-grain=1:film-grain=2',

    [string[]]$KeepLanguages = @('eng', 'jpn', 'und'),
    [string]$DefaultAudioLanguage = 'jpn',
    [string]$DefaultSubtitleLanguage = 'eng',   # 'none' = no subtitle track on by default

    [int]$Start = 0,          # test clips: first frame; -1 = middle of the episode
    [int]$Frames = 0,         # test clips: number of frames (0 = whole episode)
    [int]$Screenshots = 4,    # side-by-side comparison images per upscaled file (0 = none)
    [switch]$Plan,            # analyse and report what would happen; encode nothing
    [switch]$CompareGrain,    # test clip encoded with and without film grain, plus screenshots
    [string]$Tag,             # suffix for output names (used by -CompareGrain)
    [switch]$KeepIvf,         # keep the intermediate video stream after muxing
    [switch]$UpdateTools      # install / upgrade every tool, then stop (no files processed)
)

$ErrorActionPreference = 'Stop'
$VideoExtensions = '.mkv', '.mp4', '.m2ts', '.m4v', '.avi', '.webm'
$ToolsDir = Join-Path $PSScriptRoot 'tools'
$vpy = Join-Path $PSScriptRoot 'upscale.vpy'
if (-not (Test-Path -LiteralPath $vpy)) { throw "upscale.vpy not found next to this script ($vpy)" }
$analyzePy = Join-Path $PSScriptRoot 'analyze.py'
if (-not (Test-Path -LiteralPath $analyzePy)) { throw "analyze.py not found next to this script ($analyzePy)" }
$AnalysisVersion = 2   # bump to invalidate cached analysis results

# ── per-show settings: upscale-settings.json in the rips folder or any parent ─
# Command-line parameters win over the files; the nearest file wins over files further up
# (C:\Tools\upscale-animation\upscale-settings.json covers every subfolder under it);
# the files win over the defaults.

$settingsDir = if (Test-Path -LiteralPath $Path -PathType Container) {
    (Resolve-Path -LiteralPath $Path).Path
} else {
    Split-Path -Parent (Resolve-Path -LiteralPath $Path).Path
}
if (-not $OutDir) { $OutDir = Join-Path $settingsDir 'upscaled' }
$settingsFiles = @()
for ($d = Get-Item -LiteralPath $settingsDir; $d; $d = $d.Parent) {
    $candidate = Join-Path $d.FullName 'upscale-settings.json'
    if (Test-Path -LiteralPath $candidate) { $settingsFiles = @($candidate) + $settingsFiles }   # farthest first
}
$settingsFile = ($settingsFiles -join ', ')
$appliedSettings = [ordered]@{}
$allowedSettings = 'Content', 'Upscaler', 'TopazFilter', 'TopazPath', 'Model', 'Onnx', 'Noise', 'Matrix', 'Analysis', 'Crop',
                   'Screenshots', 'Ivtc', 'NativeHeight', 'NativeKernel', 'NativeB', 'NativeC', 'NativeTaps', 'Temporal',
                   'TemporalStrength', 'Deband', 'Crf', 'Preset', 'SvtParams', 'KeepLanguages', 'DefaultAudioLanguage',
                   'DefaultSubtitleLanguage'
foreach ($sf in $settingsFiles) {
    $sfDir = Split-Path -Parent $sf
    $json = Get-Content -LiteralPath $sf -Raw | ConvertFrom-Json
    foreach ($prop in $json.PSObject.Properties) {
        if ($allowedSettings -notcontains $prop.Name) { Write-Warning "${sf}: unknown setting '$($prop.Name)' ignored"; continue }
        if ($PSBoundParameters.ContainsKey($prop.Name)) { continue }
        $value = $prop.Value
        if ($prop.Name -eq 'Onnx' -and $value -and -not [IO.Path]::IsPathRooted($value)) { $value = Join-Path $sfDir $value }
        Set-Variable -Name $prop.Name -Value $value
        $appliedSettings[$prop.Name] = $value -join ','
    }
}
# Settings chosen by the user (command line or settings file) are never overridden by analysis
$explicit = @($PSBoundParameters.Keys) + @($appliedSettings.Keys)

# ── content profile: defaults that differ for CG films ───────────────────────

if ($Content -eq 'cg') {
    $cgDefaults = [ordered]@{
        Analysis                = 'off'    # native-resolution / denoise detection is for 2D anime
        DefaultAudioLanguage    = 'eng'
        DefaultSubtitleLanguage = 'none'   # forced subtitles keep their own flag
    }
    foreach ($k in $cgDefaults.Keys) {
        if ($explicit -notcontains $k) { Set-Variable -Name $k -Value $cgDefaults[$k] }
    }
}
$useTopaz = $Upscaler -eq 'topaz' -or ($Upscaler -eq 'auto' -and $Content -eq 'cg')
if (-not $TopazPath) { $TopazPath = 'C:\Program Files\Topaz Labs LLC\Topaz Video AI' }

# ── -CompareGrain: the same test clip with and without film-grain synthesis ──

if ($CompareGrain) {
    if ($Frames -le 0) {
        $Frames = 720
        if (-not $PSBoundParameters.ContainsKey('Start')) { $Start = -1 }
    }
    $noGrain = (@($SvtParams -split ':' | Where-Object { $_ -and $_ -notmatch '^(adaptive-)?film-grain(-denoise)?=' }) + 'film-grain=0') -join ':'
    $common = @{}
    foreach ($k in $PSBoundParameters.Keys) { if ($k -ne 'CompareGrain') { $common[$k] = $PSBoundParameters[$k] } }
    $common['Frames'] = $Frames
    $common['Start'] = $Start
    $common['OutDir'] = $OutDir
    # One episode is enough for a grain comparison
    if (Test-Path -LiteralPath $Path -PathType Container) {
        $first = Get-ChildItem -LiteralPath $Path -File | Where-Object { $VideoExtensions -contains $_.Extension.ToLower() } |
                 Sort-Object Name | Select-Object -First 1
        if (-not $first) { Write-Host "No video files found in '$Path'."; exit 0 }
        $common['Path'] = $first.FullName
        Write-Host "Grain A/B uses the first episode: $($first.Name)" -ForegroundColor Cyan
    }
    Write-Host "Grain A/B: test clip of $Frames frames, with grain ($SvtParams) and without ($noGrain)" -ForegroundColor Cyan
    & $PSCommandPath @common -Tag 'grain'
    & $PSCommandPath @common -SvtParams $noGrain -Tag 'nograin'
    Write-Host "Compare the *_grain_* and *_nograin_* images in the output's compare folder." -ForegroundColor Cyan
    exit
}

# ── logging ──────────────────────────────────────────────────────────────────

$script:BatchLog = $null
$script:EpisodeLog = $null

function Write-Log([string]$Message, [string]$Color = 'Gray') {
    Write-Host $Message -ForegroundColor $Color
    $line = '[{0:yyyy-MM-dd HH:mm:ss}] {1}' -f (Get-Date), $Message
    foreach ($file in $script:BatchLog, $script:EpisodeLog) {
        if ($file) { Add-Content -LiteralPath $file -Value $line -Encoding utf8 }
    }
}

function Write-LogWarning([string]$Message) { Write-Log "WARNING: $Message" 'Yellow' }

# ── language helpers ─────────────────────────────────────────────────────────

$IetfToIso = @{ en = 'eng'; ja = 'jpn'; fr = 'fre'; de = 'ger'; es = 'spa'; it = 'ita'; zh = 'chi'; ko = 'kor' }

# Normalise a track's language to ISO 639-2 (eng/jpn/und ...), using the IETF tag when needed
function Get-IsoLang([string]$Lang, [string]$Ietf) {
    $l = if ($Lang) { $Lang.ToLower().Trim() } else { '' }
    if ($l -and $l -ne 'und') { return $l }
    $i = if ($Ietf) { $Ietf.ToLower().Trim() } else { '' }
    if ($i) {
        $prefix = ($i -split '-')[0]
        if ($IetfToIso.ContainsKey($prefix)) { return $IetfToIso[$prefix] }
        if ($prefix -and $prefix -ne 'und') { return $prefix }
    }
    return 'und'
}

# Full English language name for track titles ("English - TrueHD 5.1")
function Get-LanguageName([string]$tag) {
    $map = @{
        eng = 'English'; jpn = 'Japanese'; und = 'Unknown'
        fre = 'French'; fra = 'French'; ger = 'German'; deu = 'German'; spa = 'Spanish'; ita = 'Italian'
        por = 'Portuguese'; chi = 'Chinese'; zho = 'Chinese'; kor = 'Korean'; rus = 'Russian'; ara = 'Arabic'
        dut = 'Dutch'; nld = 'Dutch'; pol = 'Polish'; tur = 'Turkish'; swe = 'Swedish'; nor = 'Norwegian'
        dan = 'Danish'; fin = 'Finnish'; hun = 'Hungarian'; cze = 'Czech'; ces = 'Czech'; ron = 'Romanian'
        rum = 'Romanian'; gre = 'Greek'; ell = 'Greek'; heb = 'Hebrew'; hin = 'Hindi'; tha = 'Thai'
        vie = 'Vietnamese'; ind = 'Indonesian'; may = 'Malay'; msa = 'Malay'; ukr = 'Ukrainian'
        fil = 'Filipino'; tgl = 'Tagalog'; cat = 'Catalan'; hrv = 'Croatian'; slo = 'Slovak'; slk = 'Slovak'
    }
    if (-not $tag) { return 'Unknown' }
    $key = $tag.ToLower().Trim()
    if ($map.ContainsKey($key)) { return $map[$key] }
    return $tag
}

# Short format name from mkvmerge's codec description
function Get-AudioFormatLabel([string]$Codec) {
    switch -Regex ($Codec) {
        'TrueHD.*Atmos'         { return 'TrueHD Atmos' }
        'TrueHD'                { return 'TrueHD' }
        'DTS-HD Master'         { return 'DTS-HD MA' }
        'DTS-HD High'           { return 'DTS-HD HRA' }
        'DTS-ES'                { return 'DTS-ES' }
        'DTS'                   { return 'DTS' }
        'E-AC-3|EAC3|E-AC3'     { return 'DD+' }
        'AC-3|AC3'              { return 'DD' }
        'AAC'                   { return 'AAC' }
        'FLAC'                  { return 'FLAC' }
        'PCM'                   { return 'PCM' }
        'Opus'                  { return 'Opus' }
        'MP3'                   { return 'MP3' }
        'MP2'                   { return 'MP2' }
        'Vorbis'                { return 'Vorbis' }
        default                 { return $Codec }
    }
}

# "Language - Format Channels", keeping a commentary marker from the original title
function Get-AudioTrackName([string]$Lang, [string]$Format, [int]$Channels, [string]$OriginalTitle) {
    $name = "$(Get-LanguageName $Lang) - $Format $(Get-ChannelLabel $Channels)"
    if ($OriginalTitle -match 'comment') { $name += ' (Commentary)' }
    return $name
}

function Get-ChannelLabel([int]$ch) {
    switch ($ch) {
        1 { '1.0' } 2 { '2.0' } 3 { '2.1' } 4 { '4.0' }
        5 { '5.0' } 6 { '5.1' } 7 { '6.1' } 8 { '7.1' }
        default { "${ch}ch" }
    }
}

# ── source video / colour helpers ────────────────────────────────────────────

# FFmpeg colour names -> CICP codes (what SVT-AV1 and mkvmerge expect)
$CicpPrimaries = @{ bt709 = 1; bt470m = 4; bt470bg = 5; smpte170m = 6; smpte240m = 7; bt2020 = 9 }
$CicpTransfer  = @{ bt709 = 1; smpte170m = 6; 'bt2020-10' = 14; 'bt2020-12' = 15; smpte2084 = 16; 'arib-std-b67' = 18 }
$CicpMatrix    = @{ bt709 = 1; bt470bg = 5; smpte170m = 6; bt2020nc = 9; bt2020c = 10 }
$Inv = [cultureinfo]::InvariantCulture

function ConvertFrom-Rational($Value) {
    $s = "$Value"
    if ($s -match '^\s*(-?[\d.]+)\s*/\s*([\d.]+)\s*$') {
        $den = [double]::Parse($Matches[2], $Inv)
        if ($den -eq 0) { return 0 }   # ffprobe reports unknown rates as 0/0
        return [double]::Parse($Matches[1], $Inv) / $den
    }
    return [double]::Parse($s, $Inv)
}

function Format-Inv([double]$Value, [string]$Fmt = '0.####') { $Value.ToString($Fmt, $Inv) }

# Codec, height, colour tags and HDR metadata of the first video stream
function Get-SourceVideoInfo([string]$Ffprobe, [string]$File) {
    $probe = & $Ffprobe -v error -select_streams v:0 -show_streams -show_format -of json $File | ConvertFrom-Json
    $stream = $probe.streams | Select-Object -First 1
    # HDR10 mastering / light-level data usually lives on the frames, so read the first one too
    $frame = (& $Ffprobe -v error -select_streams v:0 -read_intervals '%+#1' -show_frames -of json $File | ConvertFrom-Json).frames |
             Select-Object -First 1
    $side = @(@($stream.side_data_list) + @($frame.side_data_list) | Where-Object { $_ })

    $dovi = $side | Where-Object side_data_type -eq 'DOVI configuration record' | Select-Object -First 1
    [pscustomobject]@{
        Codec      = $stream.codec_name
        Width      = [int]$stream.width
        Height     = [int]$stream.height
        Sar        = $stream.sample_aspect_ratio
        FieldOrder = $stream.field_order
        FrameRate  = $(try { ConvertFrom-Rational $stream.r_frame_rate } catch { 0 })
        Duration   = $(try { [double]::Parse($probe.format.duration, $Inv) } catch { 0 })
        Primaries = $stream.color_primaries
        Transfer  = $stream.color_transfer
        Matrix    = $stream.color_space
        Mdm       = $side | Where-Object { $_.side_data_type -eq 'Mastering display metadata' -and $_.max_luminance } | Select-Object -First 1
        Cll       = $side | Where-Object side_data_type -eq 'Content light level metadata' | Select-Object -First 1
        DvProfile = if ($dovi) { [int]$dovi.dv_profile } else { $null }
        Hdr10Plus = [bool]($side | Where-Object { $_.side_data_type -match 'HDR10\+|2094-40' })
    }
}

# Colour settings for one encode: CICP codes, FFmpeg names, extra SVT params, mkvmerge options
function Get-VideoColor($Src, [bool]$Passthrough) {
    $c = [ordered]@{}
    if ($Passthrough) {
        $p = if ($Src.Primaries -and $CicpPrimaries.ContainsKey($Src.Primaries)) { $Src.Primaries } else { 'bt709' }
        $t = if ($Src.Transfer -and $CicpTransfer.ContainsKey($Src.Transfer)) { $Src.Transfer } else { 'bt709' }
        $m = if ($Src.Matrix -and $CicpMatrix.ContainsKey($Src.Matrix)) { $Src.Matrix } else { 'bt709' }
    } else {
        $p = $t = $m = 'bt709'   # upscaled output is converted to BT.709
    }
    $c.PrimName = $p; $c.PrimCode = $CicpPrimaries[$p]
    $c.TrcName = $t;  $c.TrcCode = $CicpTransfer[$t]
    $c.MatName = $m;  $c.MatCode = $CicpMatrix[$m]
    $c.Label = switch ($c.TrcCode) { 16 { 'HDR10' } 18 { 'HLG' } default { 'SDR' } }

    $c.SvtExtra = @()
    $c.MkvOpts = @(
        '--colour-primaries', "0:$($c.PrimCode)"
        '--colour-transfer-characteristics', "0:$($c.TrcCode)"
        '--colour-matrix-coefficients', "0:$($c.MatCode)"
    )

    if ($Passthrough -and $Src.Mdm) {
        $d = $Src.Mdm
        $rx = Format-Inv (ConvertFrom-Rational $d.red_x);   $ry = Format-Inv (ConvertFrom-Rational $d.red_y)
        $gx = Format-Inv (ConvertFrom-Rational $d.green_x); $gy = Format-Inv (ConvertFrom-Rational $d.green_y)
        $bx = Format-Inv (ConvertFrom-Rational $d.blue_x);  $by = Format-Inv (ConvertFrom-Rational $d.blue_y)
        $wx = Format-Inv (ConvertFrom-Rational $d.white_point_x); $wy = Format-Inv (ConvertFrom-Rational $d.white_point_y)
        $lmax = Format-Inv (ConvertFrom-Rational $d.max_luminance)
        $lmin = Format-Inv (ConvertFrom-Rational $d.min_luminance)
        $c.SvtExtra += "mastering-display=G($gx,$gy)B($bx,$by)R($rx,$ry)WP($wx,$wy)L($lmax,$lmin)"
        $c.MkvOpts += @(
            '--chromaticity-coordinates', "0:$rx,$ry,$gx,$gy,$bx,$by"
            '--white-colour-coordinates', "0:$wx,$wy"
            '--max-luminance', "0:$lmax"
            '--min-luminance', "0:$lmin"
        )
    }
    if ($Passthrough -and $Src.Cll -and $Src.Cll.max_content) {
        $c.SvtExtra += "content-light=$([int]$Src.Cll.max_content),$([int]$Src.Cll.max_average)"
        $c.MkvOpts += @(
            '--max-content-light', "0:$([int]$Src.Cll.max_content)"
            '--max-frame-light', "0:$([int]$Src.Cll.max_average)"
        )
    }
    return [pscustomobject]$c
}

# ── tool management ──────────────────────────────────────────────────────────

function Update-SessionPath {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = "$machine;$user;$env:Path"
}

function Find-Exe([string]$Name, [string[]]$Candidates = @()) {
    $cmd = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    foreach ($c in $Candidates) {
        if ($c -and (Test-Path -LiteralPath $c)) { return $c }
    }
    return $null
}

function Invoke-Winget([string]$Id, [switch]$Upgrade) {
    $winget = Find-Exe 'winget'
    if (-not $winget) {
        throw "winget is not available. Install 'App Installer' from the Microsoft Store, or install $Id manually."
    }
    $verb = if ($Upgrade) { 'upgrade' } else { 'install' }
    Write-Log "  winget $verb $Id" 'Yellow'
    & $winget $verb --id $Id --exact --silent --accept-source-agreements --accept-package-agreements --disable-interactivity | Out-Host
    Update-SessionPath
}

# Find a winget-managed tool; install it if missing, upgrade it with -UpdateTools
function Initialize-WingetTool([string]$Id, [string]$Exe, [string[]]$Candidates = @()) {
    $path = Find-Exe $Exe $Candidates
    if (-not $path) {
        Invoke-Winget $Id
        $path = Find-Exe $Exe $Candidates
    } elseif ($UpdateTools) {
        Invoke-Winget $Id -Upgrade
        $path = Find-Exe $Exe $Candidates
    }
    if (-not $path) { throw "$Exe still not found after installing $Id. Open a new PowerShell window and try again." }
    return $path
}

function Get-Python {
    $probe = 'import sys; print(sys.executable if sys.version_info >= (3, 12) else "")'
    $py = Find-Exe 'py'
    if ($py) {
        $exe = & $py -3 -c $probe 2>$null | Select-Object -Last 1
        if ($exe) { return $exe.Trim() }
    }
    $python = Find-Exe 'python'
    if ($python -and $python -notlike '*WindowsApps*') {
        $exe = & $python -c $probe 2>$null | Select-Object -Last 1
        if ($exe) { return $exe.Trim() }
    }
    return $null
}

function Get-PythonScriptDirs([string]$Python) {
    if (-not $Python) { return @() }
    $dirs = & $Python -c "import sysconfig; print(sysconfig.get_path('scripts')); print(sysconfig.get_path('scripts', 'nt_user'))" 2>$null
    return @($dirs | Where-Object { $_ } | ForEach-Object { $_.Trim() })
}

function Find-Vspipe([string]$Python) {
    $candidates = @(Get-PythonScriptDirs $Python | ForEach-Object { Join-Path $_ 'vspipe.exe' })
    $candidates += Join-Path $env:ProgramFiles 'VapourSynth\core\vspipe.exe'
    return Find-Exe 'vspipe' $candidates
}

function Invoke-PipInstall([string]$Python, [string[]]$Packages) {
    Write-Log "  pip install --upgrade $($Packages -join ' ')" 'Yellow'
    & $Python -m pip install --upgrade --disable-pip-version-check @Packages | Out-Host
    $ok = $LASTEXITCODE -eq 0
    # VapourSynth installed through pip needs a one-time "vapoursynth config"
    $vsCli = Find-Exe 'vapoursynth' @(Get-PythonScriptDirs $Python | ForEach-Object { Join-Path $_ 'vapoursynth.exe' })
    if ($vsCli) { & $vsCli config *> $null }
    return $ok
}

# VapourSynth is pinned to R79: vs-mlrt's plugins still use VapourSynth's API 3, which R80
# removed (github.com/AmusementClub/vs-mlrt/issues/171). BestSource 22+ requires R80, so it
# is held below 22. Raise both once vs-mlrt publishes API 4 builds.
$VsVersionPin = 79
# akarin provides the Expr filter vs-mlrt uses internally (e.g. in Real-CUGAN)
$RequiredPlugins = 'bs', 'vivtc', 'descale', 'mv', 'akarin'
$VsPackages = "vapoursynth==$VsVersionPin", 'vapoursynth-bestsource<22', 'vapoursynth-vivtc', 'vapoursynth-descale',
              'vapoursynth-mvtools', 'vapoursynth-akarin'

# Installed VapourSynth release number (e.g. 79), or $null
function Get-VapourSynthVersion([string]$Python) {
    if (-not $Python) { return $null }
    $probe = 'import vapoursynth as v; c = v.core; cv = getattr(c, "core_version", None); print(cv.release_major if cv else c.version_number())'
    $out = & $Python -W ignore -c $probe 2>$null | Select-Object -Last 1
    $n = 0
    if ($out -and [int]::TryParse($out.Trim(), [ref]$n)) { return $n }
    return $null
}

# True when vspipe runs and every listed plugin namespace is available
function Test-VapourSynth([string]$Vspipe, [string[]]$Plugins = $RequiredPlugins) {
    if (-not $Vspipe) { return $false }
    $test = Join-Path $env:TEMP 'upscale-vstest.vpy'
    Set-Content -LiteralPath $test -Encoding utf8 -Value (
        @('from vapoursynth import core') +
        @($Plugins | ForEach-Object { "core.$_" }) +
        @('core.std.BlankClip(length=1).set_output()')
    )
    & $Vspipe --info $test - *> $null
    return $LASTEXITCODE -eq 0
}

function Initialize-VsMlrt([string]$Flavor) {
    $root = Join-Path $ToolsDir 'vs-mlrt'
    $verFile = Join-Path $root 'VERSION.txt'
    $installed = if (Test-Path -LiteralPath $verFile) { (Get-Content -LiteralPath $verFile -Raw).Trim() } else { '' }
    $existing = Get-ChildItem -LiteralPath $root -Recurse -Filter 'vsmlrt.py' -ErrorAction SilentlyContinue | Select-Object -First 1

    if ($existing -and $installed -like "*-$Flavor" -and -not $UpdateTools) { return $existing.DirectoryName }

    $rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/AmusementClub/vs-mlrt/releases/latest' -TimeoutSec 30
    $want = "$($rel.tag_name)-$Flavor"
    if ($existing -and $installed -eq $want) {
        Write-Log "  vs-mlrt $($rel.tag_name) ($Flavor) is current" 'DarkCyan'
        return $existing.DirectoryName
    }

    $assets = @($rel.assets |
        Where-Object { $_.name -match "^vsmlrt-windows-x64-$Flavor\.v[\d.]+\.7z(\.\d{3})?$" } |
        Sort-Object name)
    if (-not $assets) {
        throw "vs-mlrt $($rel.tag_name) has no '$Flavor' package. Assets: $($rel.assets.name -join ', ')"
    }

    $dl = Join-Path $env:TEMP "vsmlrt-$($rel.tag_name)-$Flavor"
    New-Item -ItemType Directory -Force -Path $dl | Out-Null
    $sizeGB = ($assets | Measure-Object -Property size -Sum).Sum / 1GB
    Write-Log ('  Downloading vs-mlrt {0} ({1} package, {2:N1} GB) ...' -f $rel.tag_name, $Flavor, $sizeGB) 'Yellow'
    foreach ($a in $assets) {
        $out = Join-Path $dl $a.name
        if ((Test-Path -LiteralPath $out) -and (Get-Item -LiteralPath $out).Length -eq $a.size) { continue }
        curl.exe -L --fail --retry 3 -o $out $a.browser_download_url
        if ($LASTEXITCODE -ne 0) { throw "Download failed: $($a.name)" }
    }

    $SevenZip = Initialize-WingetTool '7zip.7zip' '7z' @(Join-Path $env:ProgramFiles '7-Zip\7z.exe')
    $staging = Join-Path $ToolsDir 'vs-mlrt.new'
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    Write-Log '  Extracting vs-mlrt ...' 'Yellow'
    & $SevenZip x (Join-Path $dl $assets[0].name) "-o$staging" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Extracting vs-mlrt failed.' }

    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    Move-Item -LiteralPath $staging -Destination $root
    Set-Content -LiteralPath $verFile -Value $want
    Remove-Item -LiteralPath $dl -Recurse -Force -ErrorAction SilentlyContinue

    $existing = Get-ChildItem -LiteralPath $root -Recurse -Filter 'vsmlrt.py' | Select-Object -First 1
    if (-not $existing) { throw 'vsmlrt.py not found in the vs-mlrt package.' }
    Write-Log "  vs-mlrt $($rel.tag_name) ($Flavor) installed" 'Green'
    return $existing.DirectoryName
}

# Loads the vs-mlrt plugins through upscale.vpy (mode=selftest) so a broken AI runtime is
# reported once, up front, with the real error, instead of failing every episode.
function Test-VsMlrt {
    $root = Join-Path $ToolsDir 'vs-mlrt'
    $out = & $Tools.vspipe --info -a 'mode=selftest' -a "backend=$Backend" -a "mlrt=$($Tools.mlrt)" -a "mlrt_root=$root" $vpy - 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $reason = ($out -split "`r?`n" | Where-Object { $_ -match 'Error' } | Select-Object -Last 1)
        if (-not $reason) { $reason = $out.Trim() }
        if ($script:BatchLog) { Add-Content -LiteralPath $script:BatchLog -Value $out -Encoding utf8 }
        throw "vs-mlrt (the AI upscaling runtime) failed its self-test for backend '$Backend': $($reason.Trim())"
    }
    Write-Log "  vs-mlrt self-test passed (backend $Backend)" 'DarkCyan'
}

# ── Topaz Video AI (CG films) ────────────────────────────────────────────────
# Uses the ffmpeg.exe that ships with Topaz Video AI 7.x and its tvai_up filter (the same
# command the app runs for an export). The newer "Topaz Video" app (1.1+) removed this
# command-line ffmpeg, so stay on Video AI 7.x for this route.

function Initialize-Topaz {
    $exe = Join-Path $TopazPath 'ffmpeg.exe'
    if (-not (Test-Path -LiteralPath $exe)) {
        throw ("Topaz Video AI's ffmpeg.exe was not found in '$TopazPath'. CG mode upscales with Topaz Video AI 7.x. " +
               'Install it there, set "TopazPath" in upscale-settings.json, or run with -Upscaler vs to use the built-in upscaler.')
    }
    # The app sets these for its own exports; a script run has to set them itself
    $models = Join-Path $env:ProgramData 'Topaz Labs LLC\Topaz Video AI\models'
    if (-not $env:TVAI_MODEL_DIR) { $env:TVAI_MODEL_DIR = $models }
    if (-not $env:TVAI_MODEL_DATA_DIR) { $env:TVAI_MODEL_DATA_DIR = $models }
    if (-not (Test-Path -LiteralPath (Join-Path $env:TVAI_MODEL_DIR 'auth.tpz'))) {
        Write-LogWarning "Topaz sign-in file not found in $($env:TVAI_MODEL_DIR). If the self-test fails, open Topaz Video AI once and sign in."
    }
    $appExe = Join-Path $TopazPath 'Topaz Video AI.exe'
    $ver = if (Test-Path -LiteralPath $appExe) { (Get-Item -LiteralPath $appExe).VersionInfo.ProductVersion } else { $null }
    $script:TopazVersion = if ($ver) { $ver.Trim() } else { 'unknown version' }
    return $exe
}

# Upscales a short test pattern with the configured model, so a missing sign-in, model or
# GPU problem stops the batch up front with Topaz's own error instead of failing every file.
function Get-TopazChain([int]$W, [int]$H) {
    # y4m carries no colour info: tag the frames BT.709 (upscale.vpy prep converts to it), run
    # the model at the output size, then resize exactly to it as the app does (also corrects
    # anamorphic sources, since W x H is the display size).
    return "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709," +
           "${TopazFilter}:w=${W}:h=${H},scale=w=${W}:h=${H}:flags=lanczos,setsar=1,format=yuv420p10le"
}

function Test-Topaz {
    # Same chain and output format as a real run, on a short test pattern
    # Output goes to stdout exactly as in a real run, and is checked to be a clean y4m stream
    # (Topaz's own messages must go to stderr, or they would corrupt the frames sent to SVT-AV1).
    $vf = 'format=yuv420p10le,' + (Get-TopazChain 1280 720)
    $y4m = Join-Path ([IO.Path]::GetTempPath()) "topaz-selftest-$PID.y4m"
    $errFile = "$y4m.log"
    & $Tools.topaz -hide_banner -nostdin -v error -sws_flags 'spline+accurate_rnd+full_chroma_int' `
        -f lavfi -i 'testsrc2=s=640x360:r=24:d=0.5' -vf $vf -f yuv4mpegpipe -strict -1 - > $y4m 2> $errFile
    $exit = $LASTEXITCODE
    $out = if (Test-Path -LiteralPath $errFile) { Get-Content -LiteralPath $errFile -Raw } else { '' }
    $head = ''
    if (Test-Path -LiteralPath $y4m) {
        $fs = [IO.File]::OpenRead($y4m)
        try { $buf = [byte[]]::new(9); $n = $fs.Read($buf, 0, 9); $head = [Text.Encoding]::ASCII.GetString($buf, 0, $n) }
        finally { $fs.Dispose() }
    }
    Remove-Item -LiteralPath $y4m, $errFile -Force -ErrorAction SilentlyContinue
    if ($exit -ne 0) {
        if ($script:BatchLog) { Add-Content -LiteralPath $script:BatchLog -Value $out -Encoding utf8 }
        $reason = ($out -split "`r?`n" | Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | '
        throw "Topaz Video AI failed its self-test ($TopazFilter): $reason. Open Topaz Video AI, check you're signed in, and export a few seconds with this model once so it downloads."
    }
    if ($head -ne 'YUV4MPEG2') {
        throw "Topaz Video AI's ffmpeg wrote something other than video to its output (starts with '$head'), so it can't be piped to the encoder. Please send the batch log."
    }
    Write-Log "  Topaz self-test passed (Video AI $($script:TopazVersion), $(($TopazFilter -split ':')[0]))" 'DarkCyan'
}

function Get-SvtAppVersion([string]$App) {
    if (-not $App) { return $null }
    $text = & $App --version 2>&1 | Out-String
    if ($text -match 'v?(\d+\.\d+\.\d+)') { return [version]$Matches[1] }
    return $null
}

function Get-FfmpegSvtVersion([string]$Ffmpeg) {
    $text = & $Ffmpeg -hide_banner -f lavfi -i 'color=c=black:s=320x240:d=0.1' -frames:v 1 -c:v libsvtav1 -f null - 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { return $null }
    if ($text -match 'Lib v?(\d+\.\d+\.\d+)') { return [version]$Matches[1] }
    return [version]'0.0.0'
}

# FFmpeg from BtbN's daily GitHub builds (linked from ffmpeg.org), which carry a current
# SVT-AV1. Downloaded into tools\ffmpeg by -UpdateTools and used for AV1 encoding when its
# SVT-AV1 is newer than the other encoders found. Returns the ffmpeg.exe path or $null.
function Initialize-BtbnFfmpeg {
    $root = Join-Path $ToolsDir 'ffmpeg'
    $existing = Get-ChildItem -LiteralPath $root -Recurse -Filter 'ffmpeg.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $UpdateTools) { return $(if ($existing) { $existing.FullName } else { $null }) }

    try {
        $rel = Invoke-RestMethod -Uri 'https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest' -TimeoutSec 30
        # Prefer the newest FFmpeg *release* branch build (e.g. ffmpeg-n8.0-latest-win64-gpl-8.0.zip);
        # all variants carry the same SVT-AV1. Fall back to the master build.
        $asset = $rel.assets |
            Where-Object { $_.name -match '^ffmpeg-n[\d.]+-latest-win64-gpl-[\d.]+\.zip$' } |
            Sort-Object { [version](($_.name -replace '^ffmpeg-n([\d.]+)-.*$', '$1') + '.0') } -Descending |
            Select-Object -First 1
        if (-not $asset) { $asset = $rel.assets | Where-Object name -eq 'ffmpeg-master-latest-win64-gpl.zip' | Select-Object -First 1 }
        if (-not $asset) { throw "no Windows build in the latest release" }

        $stamp = Join-Path $root 'SOURCE.txt'
        $want = "$($asset.name) $($asset.updated_at)"
        if ($existing -and (Test-Path -LiteralPath $stamp) -and (Get-Content -LiteralPath $stamp -Raw).Trim() -eq $want) {
            Write-Log "  FFmpeg (BtbN) $($asset.name) is current" 'DarkCyan'
            return $existing.FullName
        }

        $zip = Join-Path $env:TEMP $asset.name
        Write-Log ("  Downloading FFmpeg (BtbN) {0} ({1:N0} MB) ..." -f $asset.name, ($asset.size / 1MB)) 'Yellow'
        curl.exe -L --fail --retry 3 -o $zip $asset.browser_download_url
        if ($LASTEXITCODE -ne 0) { throw "download failed" }

        $staging = Join-Path $ToolsDir 'ffmpeg.new'
        Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
        Expand-Archive -LiteralPath $zip -DestinationPath $staging -Force
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $staging -Destination $root
        Set-Content -LiteralPath $stamp -Value $want
        Remove-Item -LiteralPath $zip -Force -ErrorAction SilentlyContinue

        $exe = Get-ChildItem -LiteralPath $root -Recurse -Filter 'ffmpeg.exe' | Select-Object -First 1
        if (-not $exe) { throw 'ffmpeg.exe not found in the download' }
        Write-Log "  FFmpeg (BtbN) installed: $($asset.name)" 'Green'
        return $exe.FullName
    } catch {
        Write-LogWarning "Couldn't update the BtbN FFmpeg build ($_); using the FFmpeg already installed."
        return $(if ($existing) { $existing.FullName } else { $null })
    }
}

function Get-LatestSvtVersion {
    try {
        $rel = Invoke-RestMethod -TimeoutSec 10 -Uri 'https://gitlab.com/api/v4/projects/AOMediaCodec%2FSVT-AV1/releases?per_page=20'
        return $rel |
            ForEach-Object { if ($_.tag_name -match '^v(\d+\.\d+\.\d+)$') { [version]$Matches[1] } } |
            Sort-Object -Descending | Select-Object -First 1
    } catch { return $null }
}

function Initialize-Tools([string]$MlrtFlavor) {
    Write-Log 'Checking tools ...' 'DarkCyan'
    $Tools = @{}

    $Tools.mkvmerge = Initialize-WingetTool 'MoritzBunkus.MKVToolNix' 'mkvmerge' @(Join-Path $env:ProgramFiles 'MKVToolNix\mkvmerge.exe')
    $wingetLinks = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links'
    $Tools.ffmpeg = Initialize-WingetTool 'Gyan.FFmpeg' 'ffmpeg' @(Join-Path $wingetLinks 'ffmpeg.exe')
    $Tools.ffprobe = Find-Exe 'ffprobe' @(Join-Path $wingetLinks 'ffprobe.exe')
    if (-not $Tools.ffprobe) { throw 'ffprobe not found next to ffmpeg.' }

    # Python 3.12+ (VapourSynth R75 and later require it)
    $Tools.python = Get-Python
    if (-not $Tools.python) {
        Invoke-Winget 'Python.Python.3.13'
        $Tools.python = Get-Python
        if (-not $Tools.python) { throw 'Python 3.12+ not found after install. Open a new PowerShell window and try again.' }
    } elseif ($UpdateTools) {
        Invoke-Winget 'Python.Python.3.13' -Upgrade
    }

    # VapourSynth (pinned, see $VsVersionPin) + plugins (BestSource, VIVTC, descale, MVTools)
    $Tools.vspipe = Find-Vspipe $Tools.python
    $vsVer = Get-VapourSynthVersion $Tools.python
    if ($null -ne $vsVer -and $vsVer -ne $VsVersionPin) {
        Write-Log "  VapourSynth R$vsVer is installed; switching to R$VsVersionPin (the newest release vs-mlrt's plugins work with)" 'Yellow'
    }
    if ($UpdateTools -or $vsVer -ne $VsVersionPin -or -not (Test-VapourSynth $Tools.vspipe)) {
        [void](Invoke-PipInstall $Tools.python $VsPackages)
        $Tools.vspipe = Find-Vspipe $Tools.python
        $vsVer = Get-VapourSynthVersion $Tools.python
        if ($vsVer -ne $VsVersionPin -or -not (Test-VapourSynth $Tools.vspipe)) {
            throw "VapourSynth R$VsVersionPin or one of its plugins ($($RequiredPlugins -join ', ')) is not working (found R$vsVer at '$($Tools.vspipe)'). Run 'vspipe --version' to diagnose."
        }
    }

    # Optional deband plugin (only installed when -Deband is used)
    $Tools.deband = (Test-VapourSynth $Tools.vspipe @('neo_f3kdb')) -or (Test-VapourSynth $Tools.vspipe @('f3kdb'))
    if (-not $Tools.deband -and $Deband) {
        foreach ($pkg in 'vapoursynth-neo-f3kdb', 'vapoursynth-f3kdb') {
            if (Invoke-PipInstall $Tools.python @($pkg)) { break }
        }
        $Tools.deband = (Test-VapourSynth $Tools.vspipe @('neo_f3kdb')) -or (Test-VapourSynth $Tools.vspipe @('f3kdb'))
        if (-not $Tools.deband) { Write-LogWarning 'No f3kdb deband plugin could be installed; debanding is off for this run.' }
    }

    if (-not $Plan) { $Tools.mlrt = Initialize-VsMlrt $MlrtFlavor }

    # SVT-AV1: use the newest of SvtAv1EncApp, the BtbN FFmpeg build and the winget FFmpeg
    $encoders = @()
    $svtApp = Find-Exe 'SvtAv1EncApp' @(Join-Path $ToolsDir 'svt-av1\SvtAv1EncApp.exe')
    $appVer = Get-SvtAppVersion $svtApp
    if ($svtApp -and $appVer) {
        $encoders += [pscustomobject]@{ Kind = 'svtapp'; Path = $svtApp; Version = $appVer; Label = 'SvtAv1EncApp'; Rank = 0 }
    }
    $btbn = Initialize-BtbnFfmpeg
    foreach ($ff in @($btbn, $Tools.ffmpeg) | Where-Object { $_ } | Select-Object -Unique) {
        $v = Get-FfmpegSvtVersion $ff
        if ($v) {
            $isBtbn = $ff -eq $btbn
            $encoders += [pscustomobject]@{
                Kind = 'ffmpeg'; Path = $ff; Version = $v; Rank = $(if ($isBtbn) { 1 } else { 2 })
                Label = $(if ($isBtbn) { 'FFmpeg (BtbN build) libsvtav1' } else { 'FFmpeg libsvtav1' })
            }
        }
    }
    if (-not $encoders) { throw 'No SVT-AV1 encoder found: install SvtAv1EncApp, or an FFmpeg build with libsvtav1.' }
    # Newest version wins; on a tie prefer SvtAv1EncApp, then the BtbN build
    $best = $encoders | Sort-Object @{ Expression = 'Version'; Descending = $true }, @{ Expression = 'Rank' } | Select-Object -First 1
    $Tools.encoder = $best.Kind
    $Tools.svtVersion = $best.Version
    if ($best.Kind -eq 'svtapp') { $Tools.svtapp = $best.Path } else { $Tools.encFfmpeg = $best.Path }

    $latest = Get-LatestSvtVersion
    $source = $best.Label
    if ($latest -and $Tools.svtVersion -lt $latest) {
        # Not an error: encoding works fine; it just isn't the newest encoder release
        Write-Log "  SVT-AV1 v$($Tools.svtVersion) via $source (newest release is v$latest; fine to use; -UpdateTools fetches a newer build when one is available)" 'DarkCyan'
    } else {
        $note = if ($latest) { '(latest)' } else { '(could not check for updates)' }
        Write-Log "  SVT-AV1 v$($Tools.svtVersion) via $source $note" 'DarkCyan'
    }

    Write-Log '  Tools ready.' 'DarkCyan'
    return $Tools
}

# ── per-file analysis (native resolution, noise), cached in logs\analysis ────

function Invoke-SourceAnalysis($File, [bool]$DoNative, [bool]$DoNoise, [string]$IvtcMode, [string]$FieldOrder, [string]$CropArg) {
    $cacheDir = Join-Path $LogDir 'analysis'
    New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
    $stem = [IO.Path]::GetFileNameWithoutExtension($File.Name)
    $cacheFile = Join-Path $cacheDir "$stem.json"
    $errFile = Join-Path $cacheDir "$stem.stderr.log"
    $key = '{0}|{1}|v{2}|native{3}|noise{4}|{5}|crop{6}' -f $File.Length, $File.LastWriteTimeUtc.Ticks, $AnalysisVersion,
           [int]$DoNative, [int]$DoNoise, $IvtcMode, $CropArg

    if (Test-Path -LiteralPath $cacheFile) {
        try {
            $cached = Get-Content -LiteralPath $cacheFile -Raw | ConvertFrom-Json
            if ($cached.key -eq $key) { return $cached.result }
        } catch { }
    }

    Write-Log "  Analyzing $($File.Name) (native resolution / noise; about a minute) ..." 'DarkGray'
    $pyArgs = @($analyzePy, '--src', $File.FullName, '--ivtc', $IvtcMode, '--field', $FieldOrder, '--crop', $CropArg)
    if ($DoNative) { $pyArgs += '--native' }
    if ($DoNoise) { $pyArgs += '--noise' }
    $out = & $Tools.python @pyArgs 2> $errFile
    $result = $null
    try { $result = ($out | Select-Object -Last 1) | ConvertFrom-Json } catch { }
    if (-not $result -or $result.error) {
        $why = if ($result) { $result.error } else { "no output; see $errFile" }
        Write-LogWarning "Analysis failed for $($File.Name) ($why); using default settings for it."
        return $null
    }
    @{ key = $key; result = $result } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $cacheFile -Encoding utf8
    Write-Log "    done in $($result.seconds)s" 'DarkGray'
    return $result
}

# ── black-border detection (pillarbox / letterbox / DVD overscan) ────────────

function Get-BlackBorders($File, $Src) {
    if ($Src.Duration -le 0) { return $null }
    $found = @()
    foreach ($frac in 0.2, 0.35, 0.5, 0.65, 0.8) {
        $t = (Format-Inv ($Src.Duration * $frac) '0.###')
        # reset=0 accumulates: the result covers every non-black pixel seen in the 24 frames
        $txt = & $Tools.ffmpeg -hide_banner -nostats -ss $t -i $File.FullName -an -sn -dn `
               -vf 'cropdetect=limit=0.094:round=2:reset=0' -frames:v 24 -f null - 2>&1 | Out-String
        $m = [regex]::Matches($txt, 'crop=(\d+):(\d+):(\d+):(\d+)')
        if ($m.Count -gt 0) {
            $g = $m[$m.Count - 1].Groups
            $w = [int]$g[1].Value; $h = [int]$g[2].Value; $x = [int]$g[3].Value; $y = [int]$g[4].Value
            $found += [pscustomobject]@{ L = $x; T = $y; R = $Src.Width - $x - $w; B = $Src.Height - $y - $h }
        }
    }
    if ($found.Count -lt 3) { return $null }

    # Least aggressive crop across all samples, so a dark scene can't eat real picture
    $even = { param($v) [int]([Math]::Floor([Math]::Max(0, $v) / 2) * 2) }
    $c = [ordered]@{
        L = & $even ($found.L | Measure-Object -Minimum).Minimum
        R = & $even ($found.R | Measure-Object -Minimum).Minimum
        T = & $even ($found.T | Measure-Object -Minimum).Minimum
        B = & $even ($found.B | Measure-Object -Minimum).Minimum
    }
    foreach ($k in @($c.Keys)) { if ($c[$k] -lt 4) { $c[$k] = 0 } }   # ignore 1-3 px noise
    if (($c.L + $c.R + $c.T + $c.B) -eq 0) { return $null }
    # Never crop away more than 40% in either direction (protects mostly-black title cards)
    if (($c.L + $c.R) -gt $Src.Width * 0.4 -or ($c.T + $c.B) -gt $Src.Height * 0.4) { return $null }
    return [pscustomobject]$c
}

# ── interlace / telecine detection from the picture itself ──────────────────
# Metadata is unreliable: hard-telecined anime is often flagged "progressive".
# FFmpeg's idet filter looks at the fields; telecine shows up as mostly TFF/BFF frames.

function Get-InterlaceDetect($File, $Src) {
    $tff = 0; $bff = 0; $prog = 0
    foreach ($frac in 0.3, 0.6) {
        $t = Format-Inv ([Math]::Max(0, $Src.Duration * $frac)) '0.###'
        $txt = & $Tools.ffmpeg -hide_banner -nostats -ss $t -i $File.FullName -an -sn -dn -vf idet -frames:v 200 -f null - 2>&1 | Out-String
        $m = [regex]::Matches($txt, 'Multi frame detection:\s*TFF:\s*(\d+)\s*BFF:\s*(\d+)\s*Progressive:\s*(\d+)')
        if ($m.Count -gt 0) {
            $g = $m[$m.Count - 1].Groups
            $tff += [int]$g[1].Value; $bff += [int]$g[2].Value; $prog += [int]$g[3].Value
        }
    }
    $total = $tff + $bff + $prog
    if ($total -lt 50) { return $null }
    return [pscustomobject]@{
        Interlaced = (($tff + $bff) / $total) -gt 0.3
        Field      = $(if ($bff -gt $tff) { 'bff' } else { 'tff' })
        Detail     = "idet TFF $tff, BFF $bff, progressive $prog"
    }
}

# ── tracks: what to keep, which EAC3 companions to make, which are default ───

function Get-TrackPlan($File) {
    $info = & $Tools.mkvmerge -J $File.FullName | ConvertFrom-Json
    foreach ($t in $info.tracks) {
        $t | Add-Member -NotePropertyName iso -NotePropertyValue (Get-IsoLang $t.properties.language $t.properties.language_ietf) -Force
        if ($t.type -eq 'audio') {
            $newName = Get-AudioTrackName $t.iso (Get-AudioFormatLabel $t.codec) ([int]$t.properties.audio_channels) $t.properties.track_name
            $t | Add-Member -NotePropertyName NewName -NotePropertyValue $newName -Force
        }
    }
    $audio = @($info.tracks | Where-Object type -eq 'audio')
    $subs  = @($info.tracks | Where-Object type -eq 'subtitles')
    $keepAudio = @($audio | Where-Object { $KeepLanguages -contains $_.iso })
    $keepSubs  = @($subs  | Where-Object { $KeepLanguages -contains $_.iso })
    $fallback = $audio.Count -gt 0 -and $keepAudio.Count -eq 0
    if ($fallback) { $keepAudio = $audio }

    # EAC3 companions (same rules as audioencode.ps1)
    $eac3 = @()
    $probe = & $Tools.ffprobe -v quiet -print_format json -show_streams -select_streams a $File.FullName 2>$null | ConvertFrom-Json
    foreach ($s in @($probe.streams)) {
        if (-not $s) { continue }
        $codec = $s.codec_name.ToLower()
        if (-not ($codec -eq 'truehd' -or $codec -eq 'flac' -or $codec -like 'pcm*' -or $codec -eq 'dts')) { continue }
        $lang = Get-IsoLang $s.tags.language $null
        if (-not $fallback -and $KeepLanguages -notcontains $lang) { continue }
        $ch = [int]$s.channels
        $outCh = [Math]::Min($ch, 6)   # FFmpeg's EAC3 encoder tops out at 5.1
        $label = $codec.ToUpper(); if ($s.profile) { $label += " ($($s.profile))" }
        $eac3 += [pscustomobject]@{
            StreamIndex = [int]$s.index; Codec = $label; Channels = $ch; OutChannels = $outCh
            Bitrate = $(if ($outCh -ge 6) { '960k' } else { '320k' }); LangCode = $lang
            Name = Get-AudioTrackName $lang 'DD+' $outCh $s.tags.title
        }
    }

    return [pscustomobject]@{
        AudioTracks = $audio; SubTracks = $subs; KeepAudio = $keepAudio; KeepSubs = $keepSubs
        AudioFallback = $fallback; Eac3 = $eac3
    }
}

# Default audio: the default-language EAC3 companion (plays everywhere), else the first
# default-language source track. Default subs: first full track in the subtitle language,
# skipping forced and "Signs & Songs" tracks when there's a choice.
function Get-DefaultTracks($KeepAudio, $Eac3, $KeepSubs) {
    $d = [ordered]@{ Eac3 = $null; AudioId = $null; Sub = $null }
    $d.Eac3 = $Eac3 | Where-Object { $_.LangCode -eq $DefaultAudioLanguage } | Select-Object -First 1
    if (-not $d.Eac3) {
        $t = $KeepAudio | Where-Object { $_.iso -eq $DefaultAudioLanguage } | Select-Object -First 1
        if ($t) { $d.AudioId = $t.id }
    }
    if ($DefaultSubtitleLanguage -eq 'none') { return [pscustomobject]$d }
    $eng = @($KeepSubs | Where-Object { $_.iso -eq $DefaultSubtitleLanguage })
    $d.Sub = $eng | Where-Object { -not $_.properties.forced_track -and $_.properties.track_name -notmatch 'sign|song' } |
             Select-Object -First 1
    if (-not $d.Sub) { $d.Sub = $eng | Select-Object -First 1 }
    return [pscustomobject]$d
}

# ── per-file profile: every decision made before encoding ────────────────────

function Get-FileProfile($File) {
    $src = Get-SourceVideoInfo $Tools.ffprobe $File.FullName
    $p = [ordered]@{
        File = $File; Src = $src; Route = 'upscale'; Note = ''
        MatrixIn = '709'; Sar = '1:1'; IvtcMode = 'off'; IvtcNote = ''; FieldOrder = 'tff'; Crop = $null
        AutoNoise = $false; AutoNative = $false; Analysis = $null
        Noise = $Noise; NoiseNote = 'default'
        Native = [ordered]@{ Height = $NativeHeight; Width = 0; Kernel = $NativeKernel; B = $NativeB; C = $NativeC; Taps = $NativeTaps; Name = $NativeKernel }
        NativeNote = $(if ($NativeHeight -gt 0) { 'set by you' } else { 'off' })
        OutW = 0; OutH = $Height; Start = $Start; Tracks = $null
    }

    # Test clip in the middle of the episode when -Start -1 (used by -CompareGrain)
    $middle = { param($fps) [int][Math]::Max(0, $src.Duration * $fps / 2 - $Frames / 2) }
    if ($Start -lt 0) { $p.Start = & $middle $src.FrameRate }

    if ($src.Height -ge $Height) {
        $label = (Get-VideoColor $src $true).Label
        if ($null -ne $src.DvProfile) { $label += " + Dolby Vision $($src.DvProfile)" }
        if ($src.Hdr10Plus) { $label += ' + HDR10+' }
        if ($src.Codec -eq 'av1') { $p.Route = 'skip'; $p.Note = "already $($src.Height)p AV1" }
        else { $p.Route = 'handbrake'; $p.Note = "$($src.Height)p $($src.Codec) $label" }
        return [pscustomobject]$p
    }

    $p.MatrixIn = if ($Matrix) { $Matrix } else {
        switch ($src.Matrix) {
            'bt709'     { '709' }
            'smpte170m' { '170m' }
            'bt470bg'   { '470bg' }
            default     { if ($src.Height -lt 720) { '170m' } else { '709' } }
        }
    }
    $p.Sar = if ($src.Sar -match '^[1-9]\d*:[1-9]\d*$') { $src.Sar } else { '1:1' }
    $interlaced = $src.FieldOrder -in 'tt', 'bb', 'tb', 'bt'
    $p.FieldOrder = if ($src.FieldOrder -in 'bb', 'bt') { 'bff' } else { 'tff' }
    $p.IvtcNote = if ($interlaced) { "flagged $($src.FieldOrder)" } else { 'progressive' }
    $ntsc = [Math]::Abs($src.FrameRate - 29.97) -lt 0.05
    $pal = [Math]::Abs($src.FrameRate - 25) -lt 0.05
    # Only 29.97 / 25 fps material can be telecined or interlaced anime; check the picture
    if ($Ivtc -eq 'auto' -and -not $interlaced -and ($ntsc -or $pal)) {
        $det = Get-InterlaceDetect $File $src
        if ($det) {
            $p.IvtcNote = "flagged progressive; $($det.Detail)"
            if ($det.Interlaced) { $interlaced = $true; $p.FieldOrder = $det.Field }
        }
    }
    $p.IvtcMode = switch ($Ivtc) {
        'off'   { 'off' }
        'on'    { if ($ntsc) { 'decimate' } else { 'match' } }
        default { if ($interlaced) { if ($ntsc) { 'decimate' } else { 'match' } } else { 'off' } }
    }

    if ($Crop -eq 'auto') { $p.Crop = Get-BlackBorders $File $src }
    $cw = $src.Width; $ch = $src.Height
    if ($p.Crop) { $cw -= $p.Crop.L + $p.Crop.R; $ch -= $p.Crop.T + $p.Crop.B }
    $sarParts = $p.Sar -split ':'
    $p.OutW = [int]([Math]::Round($cw * [double]$sarParts[0] / [double]$sarParts[1] * $Height / $ch / 2) * 2)

    # After decimation there are 4 frames for every 5 in the source
    if ($Start -lt 0 -and $p.IvtcMode -eq 'decimate') { $p.Start = & $middle ($src.FrameRate * 0.8) }

    # Topaz does its own denoise and works from the full frame: no analysis on that route
    $p.AutoNoise = $Analysis -ne 'off' -and -not $useTopaz -and $Model -eq 'cugan' -and $explicit -notcontains 'Noise'
    # Native-resolution detection only makes sense for progressive 1080p (anime Blu-rays)
    $p.AutoNative = $Analysis -ne 'off' -and -not $useTopaz -and $explicit -notcontains 'NativeHeight' -and
                    $ch -ge 700 -and $src.Height -ge 1000 -and $src.Height -le 1100 -and $p.IvtcMode -eq 'off'
    return [pscustomobject]$p
}

function Get-CropArg($P) {
    if ($P.Crop) { return "$($P.Crop.L),$($P.Crop.R),$($P.Crop.T),$($P.Crop.B)" }
    return '0,0,0,0'
}

function Get-CropLabel($P) {
    if (-not $P.Crop) { return 'none' }
    $c = $P.Crop
    $kind = if (($c.L + $c.R) -gt 0 -and ($c.T + $c.B) -eq 0) { 'pillarbox' }
            elseif (($c.T + $c.B) -gt 0 -and ($c.L + $c.R) -eq 0) { 'letterbox' } else { 'borders' }
    $w = $P.Src.Width - $c.L - $c.R; $h = $P.Src.Height - $c.T - $c.B
    return "$kind L$($c.L) R$($c.R) T$($c.T) B$($c.B) -> ${w}x$h"
}

# ── analysis for every file, then season (folder) consensus ──────────────────

# A native-resolution match must be this clear to be used by a single episode on its own.
# Weaker matches (analyze.py's 0.06 up to this) are only used when the season agrees on them:
# on real discs, isolated weak matches turned out to be false (different answers per episode).
$NativeStrong = 0.12

function New-NativeSetting($N) {
    return [ordered]@{ Height = [int]$N.height; Width = [int]$N.width; Kernel = $N.kernel; B = [double]$N.b
                       C = [double]$N.c; Taps = [int]$N.taps; Name = $N.kernel_name }
}

function Resolve-Analysis($Profiles) {
    $up = @($Profiles | Where-Object { $_.Route -eq 'upscale' -and ($_.AutoNoise -or $_.AutoNative) })
    foreach ($p in $up) {
        $p.Analysis = Invoke-SourceAnalysis $p.File $p.AutoNative $p.AutoNoise $p.IvtcMode $p.FieldOrder (Get-CropArg $p)
        $a = $p.Analysis
        if ($a -and $p.AutoNoise -and $a.noise -and $null -ne $a.noise.metric) {
            $p.Noise = [int]$a.noise.level
            $p.NoiseNote = "episode noise $($a.noise.metric)"
        }
        if ($a -and $p.AutoNative -and $a.native) {
            $n = $a.native
            if ($n.confident -and [double]$n.prominence -ge $NativeStrong) {
                $p.Native = New-NativeSetting $n
                $p.NativeNote = "episode: strong match, prominence $($n.prominence), $($n.agree)/$($n.frames) frames"
            } elseif ($n.confident) {
                $p.NativeNote = "weak match $($n.height)p $($n.kernel_name) (prominence $($n.prominence) < $NativeStrong); not used unless the season agrees"
            } else {
                $p.NativeNote = "not detected ($($n.reason))"
            }
        }
    }

    if ($Analysis -ne 'auto' -or $up.Count -lt 3) { return }

    # Native resolution: if 60%+ of episodes found the same match (weak or strong), use it for
    # every episode except one with a strong match of its own that differs (a special or recap).
    $nat = @($up | Where-Object { $_.AutoNative })
    $hits = @($nat | Where-Object { $_.Analysis -and $_.Analysis.native -and $_.Analysis.native.confident })
    if ($nat.Count -ge 3 -and $hits.Count -gt 0) {
        $top = $hits | Group-Object { "$($_.Analysis.native.height)|$($_.Analysis.native.kernel_name)" } |
               Sort-Object Count -Descending | Select-Object -First 1
        if ($top.Count -ge [Math]::Ceiling($nat.Count * 0.6)) {
            $ref = New-NativeSetting $top.Group[0].Analysis.native
            Write-Log "Season consensus: native $($ref.Width)x$($ref.Height) $($ref.Name) ($($top.Count)/$($nat.Count) episodes)" 'Green'
            foreach ($p in $nat) {
                $own = if ($p.Analysis -and $p.Analysis.native -and $p.Analysis.native.confident) { $p.Analysis.native } else { $null }
                $differs = $own -and ($own.height -ne $ref.Height -or $own.kernel_name -ne $ref.Name)
                if ($differs -and [double]$own.prominence -ge $NativeStrong) {
                    $p.NativeNote += "; DIFFERS from season $($ref.Height)p $($ref.Name) -- kept episode result"
                    Write-LogWarning "$($p.File.Name): native $($p.Native.Height)p $($p.Native.Name) differs from the season ($($ref.Height)p $($ref.Name))."
                } else {
                    $p.Native = $ref
                    $p.NativeNote = "season consensus ($($top.Count)/$($nat.Count) episodes); episode $($p.NativeNote)"
                }
            }
        } else {
            Write-Log "Season consensus: no native resolution shared by most episodes ($($top.Count)/$($nat.Count) agree); only strong per-episode matches are used." 'DarkGray'
        }
    }

    # Denoise: one level for the whole folder, from the median measured noise
    $noisy = @($up | Where-Object { $_.AutoNoise -and $_.Analysis -and $_.Analysis.noise -and $null -ne $_.Analysis.noise.metric })
    if ($noisy.Count -ge 3) {
        $metrics = @($noisy | ForEach-Object { [double]$_.Analysis.noise.metric } | Sort-Object)
        $median = $metrics[[int][Math]::Floor(($metrics.Count - 1) / 2)]
        $level = if ($median -lt 0.8) { -1 } elseif ($median -lt 1.6) { 0 } else { 3 }
        Write-Log ("Season consensus: denoise {0} from median noise {1:N2} ({2} episodes)" -f (Get-NoiseLabel $level), $median, $noisy.Count) 'Green'
        foreach ($p in $noisy) {
            $p.Noise = $level
            $p.NoiseNote = "season median {0:N2}; episode {1}" -f $median, $p.Analysis.noise.metric
        }
    }
}

function Get-NoiseLabel([int]$Level) {
    switch ($Level) { -1 { 'none' } 0 { 'light' } 3 { 'strong' } default { "$Level" } }
}

# ── plan report ──────────────────────────────────────────────────────────────

function Write-Plan($Profiles) {
    $file = Join-Path $LogDir ('plan-{0:yyyyMMdd-HHmmss}.txt' -f (Get-Date))
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add("Plan for $($Profiles.Count) file(s) -> $OutDir   [content: $Content]")
    $lines.Add("Model: $modelLabel | backend: $Backend | CRF $Crf, preset $Preset | $SvtParams")
    $lines.Add('')
    foreach ($p in $Profiles) {
        $s = $p.Src
        $lines.Add($p.File.Name)
        switch ($p.Route) {
            'skip'      { $lines.Add("  Route:     skip ($($p.Note))") }
            'handbrake' { $lines.Add("  Route:     encode only with HandBrake ($($p.Note))") }
            default {
                $lines.Add(("  Route:     upscale  {0}x{1} {2} {3:N3} fps, SAR {4}, matrix {5}" -f $s.Width, $s.Height, $s.Codec, $s.FrameRate, $p.Sar, $p.MatrixIn))
                $lines.Add("  Crop:      $(Get-CropLabel $p)")
                $lines.Add("  IVTC:      $($p.IvtcMode)  [$($p.IvtcNote)]")
                if ($useTopaz) {
                    $lines.Add("  Upscale:   Topaz $(($TopazFilter -split ':')[0]) (denoise/detail handled by Topaz)")
                    $lines.Add("  Output:    $($p.OutW)x$($p.OutH)")
                } else {
                    $nat = if ($p.Native.Height -gt 0) { "$($p.Native.Height)p $($p.Native.Name)" } else { 'none' }
                    $lines.Add("  Native:    $nat  [$($p.NativeNote)]")
                    if ($Model -eq 'cugan') { $lines.Add("  Denoise:   $(Get-NoiseLabel $p.Noise)  [$($p.NoiseNote)]") }
                    $lines.Add("  Output:    $($p.OutW)x$($p.OutH)$(if ($Temporal) { ', temporal' })$(if ($Deband -and $Tools.deband) { ', deband' })")
                }
            }
        }
        if ($p.Route -ne 'skip' -and $p.Tracks) {
            $tp = $p.Tracks
            $d = Get-DefaultTracks $tp.KeepAudio $tp.Eac3 $tp.KeepSubs
            $fmt = { param($t) "#$($t.id) $($t.codec) [$($t.iso)]$(if ($t.properties.track_name) { " $($t.properties.track_name)" })" }
            $keepA = @($tp.KeepAudio | ForEach-Object { "#$($_.id) '$($_.NewName)'" }) -join '; '
            $dropA = @($tp.AudioTracks | Where-Object { $tp.KeepAudio -notcontains $_ } | ForEach-Object { & $fmt $_ }) -join '; '
            $lines.Add("  Audio:     keep $keepA$(if ($dropA) { " | drop $dropA" })")
            foreach ($e in $tp.Eac3) {
                $lines.Add("  EAC3:      #$($e.StreamIndex) $($e.Codec) $($e.Channels)ch -> '$($e.Name)' $($e.Bitrate)$(if ($d.Eac3 -eq $e) { '  (default)' })")
            }
            if ($null -ne $d.AudioId) { $lines.Add("  Default A: source #$($d.AudioId)") }
            $keepS = @($tp.KeepSubs | ForEach-Object { & $fmt $_ }) -join '; '
            $dropS = @($tp.SubTracks | Where-Object { $tp.KeepSubs -notcontains $_ } | ForEach-Object { & $fmt $_ }) -join '; '
            $lines.Add("  Subs:      keep $(if ($keepS) { $keepS } else { 'none' })$(if ($dropS) { " | drop $dropS" })")
            if ($d.Sub) { $lines.Add("  Default S: #$($d.Sub.id)$(if ($d.Sub.properties.track_name) { " $($d.Sub.properties.track_name)" })") }
            elseif ($DefaultSubtitleLanguage -eq 'none' -and $tp.KeepSubs.Count -gt 0) { $lines.Add('  Default S: none (forced tracks keep their flag)') }
        }
        $lines.Add('')
    }
    $need = ($Profiles | Where-Object { $_.Route -ne 'skip' } | ForEach-Object { Get-SpaceNeeded $_ } | Measure-Object -Sum).Sum
    $free = Get-FreeSpace $OutDir
    $lines.Add(("Disk: about {0:N0} GB needed in total (peak per episode is much less); {1}" -f ($need / 1GB),
        $(if ($null -ne $free) { '{0:N0} GB free' -f ($free / 1GB) } else { 'free space unknown' })))
    Set-Content -LiteralPath $file -Value $lines -Encoding utf8
    foreach ($l in $lines) { Write-Host $l }
    Write-Log "Plan written to $file" 'Cyan'
}

# ── disk space ───────────────────────────────────────────────────────────────

function Get-FreeSpace([string]$Dir) {
    try { return [IO.DriveInfo]::new([IO.Path]::GetPathRoot((Resolve-Path -LiteralPath $Dir).Path)).AvailableFreeSpace }
    catch { return $null }   # network shares etc.
}

# Peak space for one episode: the video stream and the final MKV exist together at mux time.
# Upscaled 4K AV1 at CRF 14 runs up to ~2x a 1080p Blu-ray source; HandBrake re-encodes of
# 4K sources come out smaller than the source.
function Get-SpaceNeeded($P) {
    $size = $P.File.Length
    $outEstimate = if ($P.Route -eq 'handbrake') { $size } else { [Math]::Max($size * 2, 4GB) }
    return [long]($outEstimate * 2 + 2GB)
}

# ── comparison screenshots: source (scaled) | final encode ───────────────────

function Save-Comparisons($P, [string]$Mkv, [string]$Base) {
    if ($Screenshots -le 0) { return }
    $dir = Join-Path $OutDir 'compare'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null

    $o = (& $Tools.ffprobe -v error -select_streams v:0 -show_entries 'stream=r_frame_rate:format=duration' -of json $Mkv | ConvertFrom-Json)
    $outDur = [double]::Parse($o.format.duration, $Inv)
    $outFps = ConvertFrom-Rational $o.streams[0].r_frame_rate
    # Where the encoded clip starts in the source (test clips start part-way in)
    $offset = if ($Frames -gt 0 -and $outFps -gt 0) { $P.Start / $outFps } else { 0 }

    $cropFilter = if ($P.Crop) {
        "crop=$($P.Src.Width - $P.Crop.L - $P.Crop.R):$($P.Src.Height - $P.Crop.T - $P.Crop.B):$($P.Crop.L):$($P.Crop.T),"
    } else { '' }
    $fc = "[0:v]${cropFilter}scale=$($P.OutW):$($P.OutH):flags=lanczos,setsar=1,format=rgb24[a];" +
          "[1:v]setsar=1,format=rgb24[b];[a][b]hstack=inputs=2"

    $saved = 0
    for ($i = 1; $i -le $Screenshots; $i++) {
        $t = $outDur * $i / ($Screenshots + 1)
        $png = Join-Path $dir ('{0}_{1:00}.png' -f $Base, $i)
        & $Tools.ffmpeg -v error -y -ss (Format-Inv ($offset + $t) '0.###') -i $P.File.FullName `
            -ss (Format-Inv $t '0.###') -i $Mkv -filter_complex $fc -frames:v 1 -update 1 $png 2>> $script:EpisodeLog
        if ($LASTEXITCODE -eq 0) { $saved++ }
    }
    if ($saved -gt 0) { Write-Log "  Saved $saved comparison image(s) to $dir (left: source scaled, right: final)" 'Green' }
}

# ── find files ───────────────────────────────────────────────────────────────

$files = if (Test-Path -LiteralPath $Path -PathType Container) {
    Get-ChildItem -LiteralPath $Path -File | Where-Object { $VideoExtensions -contains $_.Extension.ToLower() }
} else {
    Get-Item -LiteralPath $Path
}
if (-not $files -and -not $UpdateTools) { Write-Host "No video files found in '$((Resolve-Path $Path).Path)'."; exit 0 }

if (-not $UpdateTools) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $OutDir = (Resolve-Path $OutDir).Path
    $LogDir = Join-Path $OutDir 'logs'
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $script:BatchLog = Join-Path $LogDir ('{0}-{1:yyyyMMdd-HHmmss}.log' -f $(if ($Plan) { 'plan' } else { 'batch' }), (Get-Date))
}

# ── run at below-normal priority (child processes inherit it) ────────────────

$self = Get-Process -Id $PID
$originalPriority = $self.PriorityClass
try { $self.PriorityClass = 'BelowNormal' } catch { }

$results = [Collections.Generic.List[object]]::new()

try {
    # ── GPU, tools, backend, model ───────────────────────────────────────────

    # Pick the GPU the upscaler will use: NVIDIA first, then a discrete AMD/Intel card.
    # Integrated graphics (e.g. "AMD Radeon(TM) Graphics" on Ryzen CPUs) are listed as not used.
    $gpus = @((Get-CimInstance Win32_VideoController).Name | Where-Object { $_ })
    $gpu = ($gpus | Where-Object { $_ -match 'NVIDIA' } | Select-Object -First 1)
    if (-not $gpu) { $gpu = $gpus | Where-Object { $_ -match 'Radeon RX|Radeon Pro|Arc' } | Select-Object -First 1 }
    if (-not $gpu) { $gpu = $gpus | Select-Object -First 1 }
    $isNvidia = $gpu -match 'NVIDIA'
    $others = @($gpus | Where-Object { $_ -ne $gpu })
    Write-Log "GPU: $gpu$(if ($others) { "  (also present, not used: $($others -join ', '))" })" 'DarkCyan'

    $Tools = Initialize-Tools $(if ($isNvidia) { 'cuda' } else { 'vk' })

    if (-not $Backend) { $Backend = if ($isNvidia) { 'trt' } else { 'ncnn' } }

    if ($UpdateTools) {
        $wingetLinks = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links'
        $Tools.handbrake = Initialize-WingetTool 'HandBrake.HandBrake.CLI' 'HandBrakeCLI' @(Join-Path $wingetLinks 'HandBrakeCLI.exe')
        Test-VsMlrt
        if (Test-Path -LiteralPath (Join-Path $TopazPath 'ffmpeg.exe')) {
            try { $Tools.topaz = Initialize-Topaz; Test-Topaz }
            catch { Write-LogWarning "Topaz (used for CG films): $_" }
        } else {
            Write-Log "  Topaz Video AI not found in $TopazPath (only needed for CG films)" 'DarkGray'
        }
        Write-Log 'All tools are installed and up to date. Now run upscale.ps1 -Plan in a folder of rips.' 'Green'
        return
    }
    if ($useTopaz) {
        $Tools.topaz = Initialize-Topaz
        if (-not $Plan) { Test-Topaz }
    } elseif (-not $Plan) {
        Test-VsMlrt
    }

    if ($useTopaz) {
        $modelLabel = "Topaz $(($TopazFilter -split ':')[0] -replace '^model=', '')"
    } elseif (-not $Model) {
        if ($Onnx) {
            $Model = 'onnx'
        } else {
            $found = Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.onnx' -File |
                     Sort-Object LastWriteTime -Descending
            if ($found) {
                $Model = 'onnx'
                $Onnx = $found[0].FullName
                if ($found.Count -gt 1) { Write-LogWarning "Several .onnx models next to the script; using newest: $($found[0].Name)" }
            } else {
                $Model = 'cugan'
            }
        }
    }
    if (-not $useTopaz) {
        if ($Model -eq 'onnx') {
            if (-not $Onnx -or -not (Test-Path -LiteralPath $Onnx)) { throw "ONNX model not found: $Onnx" }
            $Onnx = (Resolve-Path -LiteralPath $Onnx).Path
        }
        $modelLabel = if ($Model -eq 'onnx') { [IO.Path]::GetFileName($Onnx) } else { 'Real-CUGAN' }
    }

    $testMode = $Frames -gt 0

    if ($appliedSettings.Count -gt 0) {
        $shown = @($appliedSettings.Keys | ForEach-Object { "$_=$($appliedSettings[$_])" }) -join ', '
        Write-Log "Folder settings ($settingsFile): $shown" 'DarkCyan'
    }
    Write-Log "Content: $Content | default audio: $DefaultAudioLanguage | default subtitles: $DefaultSubtitleLanguage" 'DarkCyan'
    if ($useTopaz) {
        Write-Log "Upscaler: Topaz Video AI $($script:TopazVersion) | $TopazFilter | CRF $Crf, preset $Preset" 'DarkCyan'
    } else {
        Write-Log "Model: $modelLabel | backend: $Backend | CRF $Crf, preset $Preset" 'DarkCyan'
    }
    Write-Log "SVT params: $SvtParams" 'DarkCyan'
    Write-Log "Found $(@($files).Count) file(s); output -> $OutDir" 'DarkCyan'

    # ── profile every file, analyse, apply season consensus ─────────────────

    Write-Log 'Reading files ...' 'DarkCyan'
    $profiles = @()
    foreach ($f in $files) {
        $p = Get-FileProfile $f
        if ($p.Route -ne 'skip') { $p.Tracks = Get-TrackPlan $f }
        $profiles += $p
    }
    Resolve-Analysis $profiles

    if ($Plan) {
        Write-Plan $profiles
        return
    }

    # ── process each file ────────────────────────────────────────────────────

    foreach ($p in $profiles) {
        $f = $p.File
        $src = $p.Src
        $passthrough = $p.Route -eq 'handbrake'
        $script:EpisodeLog = $null
        $base = [IO.Path]::GetFileNameWithoutExtension($f.Name)
        if ($testMode) { $base += "_test_$([IO.Path]::GetFileNameWithoutExtension($modelLabel) -replace '\W','_')_$($p.Start)" }
        if ($Tag) { $base += "_$Tag" }
        $mkv = Join-Path $OutDir "$base.mkv"
        $partial = Join-Path $OutDir "$base.partial.mkv"

        if ($p.Route -eq 'skip') {
            Write-Log "Skipping $($f.Name) ($($p.Note))" 'DarkGray'
            $results.Add([pscustomobject]@{ File = $f.Name; Result = "skipped ($($p.Note))"; Time = '' })
            continue
        }
        if (Test-Path -LiteralPath $mkv) {
            Write-Log "Skipping $($f.Name) (output exists)" 'DarkGray'
            $results.Add([pscustomobject]@{ File = $f.Name; Result = 'skipped'; Time = '' })
            continue
        }
        Remove-Item -LiteralPath $partial -Force -ErrorAction SilentlyContinue

        # Free space: stop the batch rather than fail hours into an encode
        $need = Get-SpaceNeeded $p
        $free = Get-FreeSpace $OutDir
        if ($null -ne $free -and $free -lt $need) {
            Write-LogWarning ("Not enough disk space for {0}: about {1:N0} GB needed, {2:N0} GB free on the output drive. Stopping the batch." -f $f.Name, ($need / 1GB), ($free / 1GB))
            $results.Add([pscustomobject]@{ File = $f.Name; Result = 'FAILED: not enough disk space (batch stopped)'; Time = '' })
            break
        }

        $script:EpisodeLog = Join-Path $LogDir "$base.log"
        Remove-Item -LiteralPath $script:EpisodeLog -Force -ErrorAction SilentlyContinue

        Write-Log ''
        Write-Log '==============================================' 'Cyan'
        Write-Log "  $($f.Name)" 'Cyan'
        Write-Log "  CRF $Crf, preset $Preset | $SvtParams" 'Cyan'
        Write-Log '==============================================' 'Cyan'
        $sw = [Diagnostics.Stopwatch]::StartNew()

        $fail = {
            param([string]$why)
            Write-LogWarning "$($f.Name): $why"
            $results.Add([pscustomobject]@{ File = $f.Name; Result = "FAILED: $why"; Time = '{0:hh\:mm\:ss}' -f $sw.Elapsed })
        }

        $vc = Get-VideoColor $src $false   # colour tags for the upscale path
        $videoOut = if ($passthrough) { Join-Path $OutDir "$base.video.mkv" } else { Join-Path $OutDir "$base.ivf" }

        if ($passthrough) {
            # ── 1-2 (4K). HandBrake encode: carries HDR10, HDR10+ and Dolby Vision ──

            Write-Log "  Source is $($p.Note): encoding with HandBrake, no upscaling." 'Cyan'
            if ($src.DvProfile -eq 5) {
                Write-LogWarning 'Dolby Vision profile 5 has no HDR10 fallback: it becomes AV1 profile 10.0 and needs a player that supports Dolby Vision in AV1.'
            } elseif ($src.DvProfile -eq 7) {
                Write-LogWarning 'Dolby Vision profile 7: the enhancement layer is dropped and the RPU converted to profile 8.1 (AV1 profile 10.1).'
            }
            if (-not $Tools.handbrake) {
                $wingetLinks = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links'
                $Tools.handbrake = Initialize-WingetTool 'HandBrake.HandBrake.CLI' 'HandBrakeCLI' @(Join-Path $wingetLinks 'HandBrakeCLI.exe')
                $hbVer = & $Tools.handbrake --version 2>$null | Select-String 'HandBrake' | Select-Object -First 1
                Write-Log "  Using $hbVer for 4K sources" 'DarkCyan'
            }

            if ($testMode) {
                $expectedFrames = $Frames
            } else {
                Write-Log '  Counting source frames ...' 'DarkGray'
                $countText = & $Tools.ffprobe -v error -select_streams v:0 -count_packets -show_entries stream=nb_read_packets -of csv=p=0 $f.FullName
                $expectedFrames = 0
                [void][int]::TryParse(($countText | Select-Object -First 1), [ref]$expectedFrames)
            }
            Write-Log "  Expected frames: $expectedFrames"

            $hbArgs = @(
                '-i', $f.FullName, '-o', $videoOut, '-f', 'av_mkv'
                '-e', 'svt_av1_10bit', '-q', $Crf, '--encoder-preset', $Preset
                '--hdr-dynamic-metadata', 'all'
                '--crop-mode', 'none'
                '--no-comb-detect', '--no-decomb', '--no-deinterlace', '--no-detelecine'
                '-a', 'none', '-s', 'none', '--no-markers'
            )
            if ($SvtParams) { $hbArgs += '-x', $SvtParams }
            if ($testMode) { $hbArgs += '--start-at', "frame:$($p.Start)", '--stop-at', "frame:$Frames" }

            Write-Log '  Encoding with HandBrake ...' 'Yellow'
            Add-Content -LiteralPath $script:EpisodeLog -Value '--- HandBrake log ---' -Encoding utf8
            & $Tools.handbrake @hbArgs 2>> $script:EpisodeLog
            $encExit = $LASTEXITCODE
        } else {
            # ── upscale settings for this file (all decided in the profile) ──

            $debandOn = [bool]($Deband -and $Tools.deband)
            $nat = $p.Native
            Write-Log ("  Source: {0}x{1} {2}, {3:N3} fps, SAR {4}, matrix {5}, field order {6}, IVTC {7}" -f
                $src.Width, $src.Height, $src.Codec, $src.FrameRate, $p.Sar, $p.MatrixIn,
                $(if ($src.FieldOrder) { $src.FieldOrder } else { 'unknown' }), $p.IvtcMode)
            Write-Log "  IVTC: $($p.IvtcMode)  [$($p.IvtcNote)]"
            Write-Log "  Crop: $(Get-CropLabel $p)"
            if ($useTopaz) {
                Write-Log "  Upscale: $modelLabel -> $($p.OutW)x$($p.OutH)"
            } else {
                Write-Log "  Native: $(if ($nat.Height -gt 0) { "$($nat.Height)p $($nat.Name)" } else { 'none' })  [$($p.NativeNote)]"
                if ($Model -eq 'cugan') { Write-Log "  Denoise: $(Get-NoiseLabel $p.Noise)  [$($p.NoiseNote)]" }
                Write-Log "  Upscale: $modelLabel -> $($p.OutW)x$($p.OutH) | deband: $(if ($debandOn) { 'on' } else { 'off' }) | temporal: $(if ($Temporal) { 'on' } else { 'off' })"
            }

            # ── 1. expected frame count ──────────────────────────────────────

            $cropArg = (Get-CropArg $p) -split ','
            $scriptArgs = @(
                '-a', "src=$($f.FullName)"
                '-a', "mode=$(if ($useTopaz) { 'prep' } else { 'upscale' })"
                '-a', "mlrt=$($Tools.mlrt)"
                '-a', "mlrt_root=$(Join-Path $ToolsDir 'vs-mlrt')"
                '-a', "model=$Model"
                '-a', "backend=$Backend"
                '-a', "noise=$($p.Noise)"
                '-a', "matrix=$($p.MatrixIn)"
                '-a', "sar=$($p.Sar)"
                '-a', "ivtc=$($p.IvtcMode)"
                '-a', "field=$($p.FieldOrder)"
                '-a', "crop_l=$($cropArg[0])"
                '-a', "crop_r=$($cropArg[1])"
                '-a', "crop_t=$($cropArg[2])"
                '-a', "crop_b=$($cropArg[3])"
                '-a', "native_height=$($nat.Height)"
                '-a', "native_kernel=$($nat.Kernel)"
                '-a', "native_b=$(Format-Inv $nat.B)"
                '-a', "native_c=$(Format-Inv $nat.C)"
                '-a', "native_taps=$($nat.Taps)"
                '-a', "deband=$([int]$debandOn)"
                '-a', "temporal=$([int][bool]$Temporal)"
                '-a', "temporal_strength=$TemporalStrength"
                '-a', "height=$Height"
                '-a', "start=$($p.Start)"
                '-a', "frames=$Frames"
            )
            if ($Model -eq 'onnx' -and -not $useTopaz) { $scriptArgs += '-a', "onnx=$Onnx" }

            Write-Log '  Reading source (first run of a file builds an index) ...' 'DarkGray'
            $infoText = & $Tools.vspipe --info @scriptArgs $vpy - 2>&1 | Out-String
            if ($LASTEXITCODE -ne 0 -or $infoText -notmatch 'Frames:\s*(\d+)') {
                Add-Content -LiteralPath $script:EpisodeLog -Value $infoText -Encoding utf8
                & $fail 'upscale script failed to load (details in the episode log)'
                continue
            }
            $expectedFrames = [int]$Matches[1]
            Write-Log "  Expected frames: $expectedFrames"

            # ── 2. upscale + AV1 encode ──────────────────────────────────────

            $vsArgs = @('-c', 'y4m') + $scriptArgs + @($vpy, '-')

            if ($Tools.encoder -eq 'svtapp') {
                $encExe = $Tools.svtapp
                $encArgs = @(
                    '-i', 'stdin'
                    '--crf', $Crf
                    '--preset', $Preset
                    '--input-depth', 10
                    '--color-primaries', $vc.PrimCode
                    '--transfer-characteristics', $vc.TrcCode
                    '--matrix-coefficients', $vc.MatCode
                )
                foreach ($kv in ($SvtParams -split ':' | Where-Object { $_ })) {
                    $k, $v = $kv -split '=', 2
                    $encArgs += "--$($k.Trim())", $v.Trim()
                }
                $encArgs += '-b', $videoOut
            } else {
                $encExe = $Tools.encFfmpeg
                $encArgs = @(
                    '-hide_banner', '-v', 'error', '-stats'
                    '-f', 'yuv4mpegpipe', '-i', '-'
                    '-c:v', 'libsvtav1', '-crf', $Crf, '-preset', $Preset
                    '-pix_fmt', 'yuv420p10le'
                    '-color_primaries', $vc.PrimName, '-color_trc', $vc.TrcName, '-colorspace', $vc.MatName
                )
                if ($SvtParams) { $encArgs += '-svtav1-params', $SvtParams }
                $encArgs += '-y', '-f', 'ivf', $videoOut
            }

            Add-Content -LiteralPath $script:EpisodeLog -Value '--- vspipe output ---' -Encoding utf8
            if ($useTopaz) {
                # Source frames (IVTC/crop/trim) -> Topaz tvai_up at 4K -> AV1, all in memory: no MOV
                $tzArgs = @(
                    '-hide_banner', '-nostdin', '-nostats', '-v', 'warning'
                    '-f', 'yuv4mpegpipe', '-i', '-'
                    '-sws_flags', 'spline+accurate_rnd+full_chroma_int'
                    '-vf', (Get-TopazChain $p.OutW $p.OutH)
                    '-f', 'yuv4mpegpipe', '-strict', '-1', '-'
                )
                Add-Content -LiteralPath $script:EpisodeLog -Value "Topaz: $($tzArgs -join ' ')" -Encoding utf8
                Write-Log '  Upscaling with Topaz + encoding ...' 'Yellow'
                & $Tools.vspipe @vsArgs 2>> $script:EpisodeLog | & $Tools.topaz @tzArgs 2>> $script:EpisodeLog | & $encExe @encArgs
            } else {
                Write-Log '  Upscaling + encoding ...' 'Yellow'
                & $Tools.vspipe @vsArgs 2>> $script:EpisodeLog | & $encExe @encArgs
            }
            $encExit = $LASTEXITCODE
        }

        if ($encExit -ne 0 -or -not (Test-Path -LiteralPath $videoOut)) {
            & $fail "encoder exited with $encExit (details in the episode log)"
            continue
        }

        # ── 3. verify every frame made it ────────────────────────────────────

        $countText = & $Tools.ffprobe -v error -select_streams v:0 -count_packets -show_entries stream=nb_read_packets -of csv=p=0 $videoOut
        $actualFrames = 0
        [void][int]::TryParse(($countText | Select-Object -First 1), [ref]$actualFrames)
        # The upscale path must match exactly; HandBrake can legitimately differ by a frame or two
        $tolerance = if ($passthrough) { [Math]::Max(2, [Math]::Ceiling($expectedFrames * 0.001)) } else { 0 }
        if ($expectedFrames -le 0) {
            Write-LogWarning "Couldn't count source frames; skipping the frame check ($actualFrames encoded)."
        } elseif ([Math]::Abs($actualFrames - $expectedFrames) -gt $tolerance) {
            & $fail "frame count mismatch: expected $expectedFrames, got $actualFrames (encode likely crashed; video file kept)"
            continue
        } else {
            Write-Log "  Encoded frames verified: $actualFrames" 'Green'
        }

        $mk = [Collections.Generic.List[string]]::new()
        $mk.AddRange([string[]]@('--output', $partial))
        if ($passthrough) {
            # HandBrake already wrote colour, HDR and Dolby Vision info; take only its video track
            $mk.AddRange([string[]]@('--no-audio', '--no-subtitles', '--no-chapters', '--no-attachments', '--no-global-tags'))
        } else {
            $mk.AddRange([string[]]$vc.MkvOpts)   # colour metadata on the video track
        }
        $mk.Add($videoOut)
        $tempFiles = @()

        if ($testMode) {
            # Test clips are video-only: source audio/subs would be the wrong length
            Write-Log '  Test clip: muxing video only.' 'DarkGray'
        } else {
            # ── 4. tracks to keep (decided in the profile) ───────────────────

            $tp = $p.Tracks
            if ($tp.AudioFallback) {
                Write-LogWarning "No $($KeepLanguages -join '/') audio found; keeping all audio rather than producing a silent file."
            }
            foreach ($t in $tp.AudioTracks) {
                $state = if ($tp.KeepAudio -contains $t) { 'keep' } else { 'drop' }
                $was = if ($t.properties.track_name) { " '$($t.properties.track_name)'" } else { '' }
                $to = if ($state -eq 'keep') { " -> '$($t.NewName)'" } else { '' }
                Write-Log "  Audio #$($t.id) $($t.codec) [$($t.iso)]$was -- $state$to" $(if ($state -eq 'keep') { 'Green' } else { 'DarkGray' })
            }
            foreach ($t in $tp.SubTracks) {
                $state = if ($tp.KeepSubs -contains $t) { 'keep' } else { 'drop' }
                Write-Log "  Subs  #$($t.id) $($t.codec) [$($t.iso)] $($t.properties.track_name) -- $state" $(if ($state -eq 'keep') { 'Green' } else { 'DarkGray' })
            }

            # ── 5. EAC3 companion tracks ─────────────────────────────────────

            $newTracks = @()
            foreach ($e in $tp.Eac3) {
                $safeName = $base -replace '[^\w\-]', '_'
                $tempPath = Join-Path $env:TEMP "${safeName}_stream$($e.StreamIndex).eac3"
                Write-Log "  Stream #$($e.StreamIndex) | $($e.Codec) | $($e.Channels) ch | lang=$($e.LangCode) -> EAC3 $($e.Bitrate)" 'Yellow'
                $ffArgs = @('-y', '-v', 'error', '-stats', '-i', $f.FullName, '-map', "0:$($e.StreamIndex)", '-vn', '-c:a', 'eac3', '-b:a', $e.Bitrate)
                if ($e.Channels -gt 6) { $ffArgs += '-ac', 6 }
                $ffArgs += $tempPath
                & $Tools.ffmpeg @ffArgs
                if ($LASTEXITCODE -ne 0) {
                    Write-LogWarning "ffmpeg failed for stream #$($e.StreamIndex); skipping this track."
                    continue
                }
                $tempFiles += $tempPath
                $newTracks += [pscustomobject]@{ Path = $tempPath; Name = $e.Name; LangCode = $e.LangCode }
            }

            # ── 6. default tracks ────────────────────────────────────────────

            $d = Get-DefaultTracks $tp.KeepAudio $newTracks $tp.KeepSubs
            $setAudioFlags = [bool]($d.Eac3 -or $null -ne $d.AudioId)
            if ($setAudioFlags) {
                $label = if ($d.Eac3) { $d.Eac3.Name } else { "source track #$($d.AudioId)" }
                Write-Log "  Default audio: $label" 'Green'
                foreach ($t in $tp.KeepAudio) {
                    $flag = if ($t.id -eq $d.AudioId) { 'yes' } else { 'no' }
                    $mk.AddRange([string[]]@('--default-track-flag', "$($t.id):$flag"))
                }
            } else {
                Write-LogWarning "No $DefaultAudioLanguage audio; keeping the source's default audio flags."
            }
            if ($d.Sub) {
                Write-Log "  Default subtitles: #$($d.Sub.id) $($d.Sub.properties.track_name)" 'Green'
                foreach ($t in $tp.KeepSubs) {
                    $flag = if ($t.id -eq $d.Sub.id) { 'yes' } else { 'no' }
                    $mk.AddRange([string[]]@('--default-track-flag', "$($t.id):$flag"))
                }
            } elseif ($DefaultSubtitleLanguage -eq 'none') {
                # No subtitles on by default; forced tracks keep the source's flags (players show them anyway)
                $off = @($tp.KeepSubs | Where-Object { -not $_.properties.forced_track })
                foreach ($t in $off) { $mk.AddRange([string[]]@('--default-track-flag', "$($t.id):no")) }
                if ($off.Count -gt 0) { Write-Log '  Default subtitles: none' 'Green' }
            } elseif ($tp.KeepSubs.Count -gt 0) {
                Write-LogWarning "No $DefaultSubtitleLanguage subtitles; keeping the source's default subtitle flags."
            }

            # Source: no video, filtered audio + subs, chapters, attachments
            if ($tp.KeepAudio.Count -gt 0) { $mk.AddRange([string[]]@('--audio-tracks', (($tp.KeepAudio.id) -join ','))) } else { $mk.Add('--no-audio') }
            if ($tp.KeepSubs.Count -gt 0)  { $mk.AddRange([string[]]@('--subtitle-tracks', (($tp.KeepSubs.id) -join ','))) } else { $mk.Add('--no-subtitles') }
            # Name every kept source audio track "Language - Format Channels"
            foreach ($t in $tp.KeepAudio) { $mk.AddRange([string[]]@('--track-name', "$($t.id):$($t.NewName)")) }
            $mk.AddRange([string[]]@('--no-video', $f.FullName))

            foreach ($t in $newTracks) {
                $mk.AddRange([string[]]@('--language', "0:$($t.LangCode)", '--track-name', "0:$($t.Name)"))
                if ($setAudioFlags) {
                    $flag = if ($t -eq $d.Eac3) { 'yes' } else { 'no' }
                    $mk.AddRange([string[]]@('--default-track-flag', "0:$flag"))
                }
                $mk.Add($t.Path)
            }
        }

        # ── 7. mux to .partial.mkv, then rename ──────────────────────────────

        Write-Log '  Muxing ...' 'Yellow'
        & $Tools.mkvmerge @mk *>> $script:EpisodeLog
        $muxExit = $LASTEXITCODE

        foreach ($tmp in $tempFiles) { Remove-Item -Force -ErrorAction SilentlyContinue $tmp }

        if ($muxExit -ge 2) {
            Remove-Item -LiteralPath $partial -Force -ErrorAction SilentlyContinue
            & $fail "mkvmerge failed (exit $muxExit); video file kept"
            continue
        }
        Move-Item -LiteralPath $partial -Destination $mkv -Force
        if (-not $KeepIvf) { Remove-Item -LiteralPath $videoOut }

        # ── 8. comparison screenshots (upscaled files; HDR sources would need tone mapping)

        if (-not $passthrough) {
            try { Save-Comparisons $p $mkv $base } catch { Write-LogWarning "Comparison images failed: $_" }
        }

        $elapsed = '{0:hh\:mm\:ss}' -f $sw.Elapsed
        Write-Log "  Done in $elapsed -> $mkv" 'Green'
        $results.Add([pscustomobject]@{ File = $f.Name; Result = 'done'; Time = $elapsed })
    }
} finally {
    $script:EpisodeLog = $null
    try { $self.PriorityClass = $originalPriority } catch { }
}

# ── summary ──────────────────────────────────────────────────────────────────

if (-not $Plan) {
    Write-Log ''
    Write-Log 'Summary' 'Cyan'
    foreach ($r in $results) {
        $color = switch -Wildcard ($r.Result) { 'done' { 'Green' } 'skipped*' { 'DarkGray' } default { 'Red' } }
        Write-Log ('  {0,-8} {1}  {2}' -f $r.Time, $r.File, $r.Result) $color
    }
    Write-Log "Output: $OutDir   Logs: $LogDir$(if ($Screenshots -gt 0) { "   Comparisons: $(Join-Path $OutDir 'compare')" })" 'Cyan'
}
