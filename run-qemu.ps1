param(
    [ValidateSet("bios", "uefi")]
    [string]$Mode = "bios"
)

$ErrorActionPreference = "Stop"

function Find-Qemu {
    $cmd = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        "$env:ProgramFiles\qemu\qemu-system-x86_64.exe",
        "$env:LOCALAPPDATA\Programs\qemu\qemu-system-x86_64.exe"
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { return $c }
    }
    throw "QEMU bulunamadı! Lütfen https://qemu.weilnetz.de/w64/ adresinden kurun."
}

function Find-Image {
    param([string]$Name)
    $img = Get-ChildItem -Path "target" -Recurse -Filter $Name -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $img) { throw "'$Name' bulunamadı. Önce 'cargo build' çalıştırın." }
    return $img.FullName
}

$qemu = Find-Qemu
$img  = Find-Image -Name "solaros-$Mode.img"

$diskPath = Join-Path $PSScriptRoot "disk.img"
$diskSize = 64 * 1024 * 1024
if (-not (Test-Path $diskPath)) {
    $diskBytes = New-Object byte[] $diskSize
    [System.IO.File]::WriteAllBytes($diskPath, $diskBytes)
    Write-Host "Data disk     : $diskPath (yeni, 64 MB)"
} elseif ((Get-Item $diskPath).Length -lt $diskSize) {
    $f = [System.IO.File]::Open($diskPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Write)
    $f.SetLength($diskSize)
    $f.Close()
    Write-Host "Data disk     : $diskPath (64 MB'e büyütüldü)"
} else {
    Write-Host "Data disk     : $diskPath"
}

Write-Host "QEMU   : $qemu"
Write-Host "Imaj   : $img"
Write-Host "Mod    : $Mode (QEMU'da BIOS modu; UEFI modu gerçek donanım içindir)"
Write-Host ""

$args = @(
    "-drive", "format=raw,file=$img",
    "-drive", "format=raw,file=$diskPath",
    "-serial", "stdio",
    "-no-reboot",
    "-accel", "whpx", "-accel", "tcg",
    "-machine", "pc",
    "-m", "2G",
    "-smp", "4"
)

if ($Mode -eq "uefi") {
    $ovmf = Get-ChildItem -Path "$env:ProgramFiles\qemu" -Recurse -Filter "OVMF*.fd" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $ovmf) {
        Write-Warning "OVMF bulunamadı, UEFI testi yapılamıyor. BIOS moduna geçiliyor."
        $Mode = "bios"
    } else {
        $args += @("-bios", $ovmf.FullName)
    }
}

& $qemu @args
