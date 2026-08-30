Set below like env vars... run once. Replace exe with actual built dir

(make exe from root git clone)
```
cargo build --release
```

in, vert, horz, and midl become full paths like "C:\Users\me\some\folder\file.png" <-- (right click, copy as path, to get this) quotes included

```
Example: $in="C:\path\to\file1.png"; $vert="C:\path\to\file2.png"; $horz="C:\path\to\file3.png"; $midl="C:\path\to\file4.png"; `
```

Here is deterministic, (optionally, just use the included workflow)
...
This whole block is safe to paste in powershell as-is:

Manual Paste: 

```powershell
$in=; $vert=; $horz=; $midl=; $ratioW=; $ratioH=; echo "Set ratio vals to 1024 & 1024 if final dimms are 8192x8192"; `
[int]$midy=[int]$ratioW * 4; [int]$midx=[int]$ratioH * 4; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$fin1 = "$dir\$prefix$num$ext"; `
& "~\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" --x $midy --y $midx --in $in --out $fin1; `
$in=$vert; `
$mid="4096"; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$vert1 = "$dir\$prefix$num$ext"; `
& "~\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" --y $midx --in $in --out $vert1; `
$in=$horz; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$horz1 = "$dir\$prefix$num$ext"; `
& "~\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" --x $midy --in $in --out $horz1; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$fin2 = "$dir\$prefix$num$ext"; `
& "~\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" strucfix --x $midy --y $midx --in $fin1 --out $fin2 --xcross $horz1 --ycross $vert1; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$fin3 = "$dir\$prefix$num$ext"; `
& "~\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" centerfix --x $midy --y $midx --in $fin2 --out $fin3 --center $midl
```

The workflow does a more automated style: (enumerating the expected folder location)

`$temp1` x `$temp2` are set to `1024` x `1024` in a perfect ratio 8192x8192. So, 640:1024 is perfectly valid for this project. (`ratioVal/8` is what is used)

The workflow:

```powershell
$fs=Get-ChildItem "D:\Comfy\ComfyUI_windows_portable\ComfyUI\output\8k" -Filter krea*.png | % { $img=[System.Drawing.Image]::FromFile($_.FullName); [PSCustomObject]@{P=$_.FullName;W=$img.Width;H=$img.Height;A=$img.Width*$img.Height}; $img.Dispose() }; $midl=($fs|?{$_.W-eq4096 -and $_.H-eq4096}).P; $horz=($fs|?{$_.H-eq4096 -and $_.W-ne4096}).P; $vert=($fs|?{$_.W-eq4096 -and $_.H-ne4096}).P; $in=($fs|sort A -desc|select -f 1).P; `
$ratioW=$temp1; $ratioH=$temp2; echo "Set ratio vals to 1024 & 1024 if final dimms are 8192x8192"; `
$in1=$in; `
[int]$midy=[int]$ratioW * 4; [int]$midx=[int]$ratioH * 4; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$fin1 = "$dir\$prefix$num$ext"; `
& "C:\Users\alienuser\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" --x $midy --y $midx --in $in --out $fin1; `
$in=$vert; `
$mid="4096"; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$vert1 = "$dir\$prefix$num$ext"; `
& "C:\Users\alienuser\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" --y $midx --in $in --out $vert1; `
$in=$horz; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$horz1 = "$dir\$prefix$num$ext"; `
& "C:\Users\alienuser\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" --x $midy --in $in --out $horz1; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$fin2 = "$dir\$prefix$num$ext"; `
& "C:\Users\alienuser\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" strucfix --x $midy --y $midx --in $fin1 --out $fin2 --xcross $horz1 --ycross $vert1; `
$dir = [IO.Path]::GetDirectoryName($in); $base = [IO.Path]::GetFileNameWithoutExtension($in); $ext = [IO.Path]::GetExtension($in); if($base -match '(.*-)(\d+)$'){ $prefix=$Matches[1]; $num=[int]$Matches[2] } else { $prefix=$base+'-'; $num=1 }; do { $num++ } while (Test-Path "$dir\$prefix$num$ext"); `
$fin3 = "$dir\$prefix$num$ext"; `
& "C:\Users\alienuser\Documents\Dev\seemingly-epic\target\release\seamingly-epic.exe" centerfix --x $midy --y $midx --in $fin2 --out $fin3 --center $midl; Remove-Item $fin1, $vert1, $horz1, $fin2, $in1, $horz, $vert, $midl; Move-Item $fin3 -Destination ($fin4="D:\Comfy\ComfyUI_windows_portable\ComfyUI\output\8k\{0}.png" -f (Get-Random -Minimum 1000000000 -Maximum 9999999999)); `
cwebp -q 90 -alpha_q 100 -m 6 -noalpha $fin4 -o ($fin4 -replace '\.png$','.webp')
```