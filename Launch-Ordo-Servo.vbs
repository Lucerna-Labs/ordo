Option Explicit

Dim fso, shell, scriptDir, command, args, i
Set fso = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("WScript.Shell")

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File " & _
    Chr(34) & fso.BuildPath(scriptDir, "Launch-Ordo-Servo.ps1") & Chr(34)

For i = 0 To WScript.Arguments.Count - 1
    command = command & " " & Chr(34) & Replace(WScript.Arguments(i), Chr(34), "\" & Chr(34)) & Chr(34)
Next

shell.Run command, 0, False
