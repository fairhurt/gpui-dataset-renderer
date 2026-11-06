# Prints the full path of the newest x64 fxc.exe from the Windows SDK, if any.
$fxc = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\fxc.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1
if ($fxc) { $fxc.FullName }
