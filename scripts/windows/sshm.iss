; Inno Setup 6.4+; the packager supplies the version and an isolated payload.
#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef PayloadDir
  #error PayloadDir is required
#endif
#ifndef Trial
  #define Trial "1"
#endif

[Setup]
AppId=sshm.qmtdlt.windows.x64
AppName=sshm
AppVersion={#AppVersion}
AppPublisher=sshm
DefaultDirName={localappdata}\Programs\sshm
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
DisableProgramGroupPage=yes
WizardStyle=modern
UninstallDisplayIcon={app}\sshm.exe
OutputBaseFilename=sshm-{#AppVersion}-windows-x86_64
Compression=lzma2
SolidCompression=yes
ChangesEnvironment=yes
CloseApplications=no
RestartApplications=no
SetupMutex=Local\sshm-inno-install
Uninstallable=yes
VersionInfoVersion={#AppVersion}.0
#if Trial == "1"
AppVerName=sshm {#AppVersion} (LOCAL TRIAL - UNSIGNED)
VersionInfoDescription=sshm local trial installer - not for distribution
#else
AppVerName=sshm {#AppVersion}
SignTool=sshm
SignedUninstaller=yes
#endif

[Files]
Source: "{#PayloadDir}\sshm.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\sshm.exe"; DestName: "sshm-candidate.exe"; Flags: dontcopy

[UninstallDelete]
Type: files; Name: "{app}\sshm.previous.exe"
Type: files; Name: "{app}\sshm-install.ini"

[Messages]
FinishedLabel=sshm has been installed. Open a NEW PowerShell or CMD window and run sshm.%nConnections, Google credentials and SSH keys are preserved.

[Code]
const
  Owner = 'sshm-inno-v1';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\sshm.qmtdlt.windows.x64_is1';
var
  Changed, Completed, HadOld, PathAddedNow: Boolean;
  OldVersion, OldMarker: String;

function CreateFileW(Name: String; Access, Share: Cardinal; Security: Integer;
  Creation, Flags: Cardinal; Template: Integer): THandle;
  external 'CreateFileW@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';
function GetFileAttributesW(Name: String): Cardinal;
  external 'GetFileAttributesW@kernel32.dll stdcall';
function ExpandEnvironmentStringsW(Source, Destination: String; Size: Cardinal): Cardinal;
  external 'ExpandEnvironmentStringsW@kernel32.dll stdcall';

function Marker: String;
begin
  Result := ExpandConstant('{app}\sshm-install.ini');
end;

function Normalize(Value: String): String;
var Buffer: String; Count: Cardinal;
begin
  Value := Trim(Value);
  if (Length(Value) >= 2) and (Value[1] = '"') and (Value[Length(Value)] = '"') then
    Value := Copy(Value, 2, Length(Value) - 2);
  SetLength(Buffer, 32768);
  Count := ExpandEnvironmentStringsW(Value, Buffer, 32768);
  if (Count > 0) and (Count <= 32768) then begin
    SetLength(Buffer, Count - 1); Value := Buffer;
  end;
  StringChangeEx(Value, '/', '\', True);
  Result := Lowercase(RemoveBackslashUnlessRoot(Value));
end;

function HasPath(Value, Target: String): Boolean;
var Parts: TArrayOfString; I: Integer;
begin
  Parts := StringSplit(Value, [';'], stAll);
  Result := False;
  for I := 0 to GetArrayLength(Parts) - 1 do
    if Normalize(Parts[I]) = Normalize(Target) then Result := True;
end;

procedure RemoveOwnedPath;
var Value, NewValue: String; Parts: TArrayOfString; I: Integer; First: Boolean;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Value) then Exit;
  Parts := StringSplit(Value, [';'], stAll);
  NewValue := ''; First := True;
  for I := 0 to GetArrayLength(Parts) - 1 do
    if Normalize(Parts[I]) <> Normalize(ExpandConstant('{app}')) then begin
      if not First then NewValue := NewValue + ';';
      NewValue := NewValue + Parts[I]; First := False;
    end;
  if Value <> NewValue then
    if not RegWriteExpandStringValue(HKCU, 'Environment', 'Path', NewValue) then
      RaiseException('Could not remove sshm from the user PATH.');
end;

procedure AddPath;
var Value: String;
begin
  RegQueryStringValue(HKCU, 'Environment', 'Path', Value);
  if not HasPath(Value, ExpandConstant('{app}')) then begin
    if Value <> '' then Value := Value + ';';
    if not RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Value + ExpandConstant('{app}')) then
      RaiseException('Could not update the user PATH.');
    PathAddedNow := True;
    if not SetIniString('Install', 'PathOwned', '1', Marker) then
      RaiseException('Could not record PATH ownership.');
  end;
end;

function ReadVersion(FileName: String): String;
var Code: Integer; Output: TExecOutput;
begin
  Result := '';
  if ExecAndCaptureOutput(FileName, '--version', '', SW_HIDE, ewWaitUntilTerminated, Code, Output) then
    if (Code = 0) and not Output.Error and (GetArrayLength(Output.StdOut) = 1) then
      if Pos('sshm ', Output.StdOut[0]) = 1 then Result := Trim(Copy(Output.StdOut[0], 6, MaxInt));
end;

function CompareVersions(A, B: String): Integer;
var Left, Right: TArrayOfString; I, L, R: Integer;
begin
  Left := StringSplit(A, ['.'], stAll); Right := StringSplit(B, ['.'], stAll);
  if (GetArrayLength(Left) <> 3) or (GetArrayLength(Right) <> 3) then
    RaiseException('Invalid installed version.');
  Result := 0;
  for I := 0 to 2 do begin
    L := StrToInt(Left[I]); R := StrToInt(Right[I]);
    if L > R then begin Result := 1; Exit; end;
    if L < R then begin Result := -1; Exit; end;
  end;
end;

function WritableExe(FileName: String): Boolean;
var Handle: THandle;
begin
  if not FileExists(FileName) then begin Result := True; Exit; end;
  Handle := CreateFileW(FileName, $C0000000, 0, 0, 3, 0, 0);
  Result := Handle <> THandle(-1);
  if Result then CloseHandle(Handle);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var Installed, Path: String; Attributes: Cardinal;
begin
  Result := '';
  if Pos(';', ExpandConstant('{app}')) > 0 then begin
    Result := 'The install directory cannot contain a semicolon (PATH separator).'; Exit;
  end;
  Path := ExpandConstant('{app}');
  while Length(Path) > 3 do begin
    Attributes := GetFileAttributesW(Path);
    if (Attributes <> $FFFFFFFF) and ((Attributes and $400) <> 0) then begin
      Result := 'Install directories must not be junctions or symbolic links.'; Exit;
    end;
    Path := ExtractFileDir(Path);
  end;
  if RegQueryStringValue(HKCU, UninstallKey, 'InstallLocation', Installed) then
    if Normalize(Installed) <> Normalize(ExpandConstant('{app}')) then begin
      Result := 'sshm is already installed elsewhere. Upgrade in that directory first.'; Exit;
    end;
  HadOld := FileExists(ExpandConstant('{app}\sshm.exe'));
  if not HadOld then
    if FileExists(Marker) or FileExists(ExpandConstant('{app}\sshm.previous.exe'))
      or FileExists(ExpandConstant('{app}\unins000.exe')) then begin
      Result := 'The destination contains conflicting installation files. Choose a dedicated empty directory.'; Exit;
    end;
  if HadOld then begin
    if (GetFileAttributesW(ExpandConstant('{app}\sshm.exe')) and $400) <> 0 then begin
      Result := 'The installed executable must not be a symbolic link.'; Exit;
    end;
    if GetIniString('Install', 'Owner', '', Marker) <> Owner then begin
      Result := 'Existing sshm is not managed by this installer. Use its original installation method.'; Exit;
    end;
    if not WritableExe(ExpandConstant('{app}\sshm.exe')) then begin
      Result := 'sshm is still running or locked. Exit all sshm sessions and retry. No process will be killed.'; Exit;
    end;
    OldVersion := ReadVersion(ExpandConstant('{app}\sshm.exe'));
    if (OldVersion = '') then begin Result := 'Existing sshm cannot start. Restore it before upgrading.'; Exit; end;
    if CompareVersions(OldVersion, '{#AppVersion}') > 0 then begin
      Result := 'Downgrade refused. Use the saved previous executable for recovery.'; Exit;
    end;
  end;
  ExtractTemporaryFile('sshm-candidate.exe');
  if ReadVersion(ExpandConstant('{tmp}\sshm-candidate.exe')) <> '{#AppVersion}' then begin
    Result := 'New executable failed version verification. Existing installation is unchanged.'; Exit;
  end;
  if HadOld then begin
    OldMarker := ExpandConstant('{tmp}\sshm-old.ini');
    if not FileCopy(ExpandConstant('{app}\sshm.exe'), ExpandConstant('{tmp}\sshm-old.exe'), False)
      or not FileCopy(Marker, OldMarker, False) then begin
      Result := 'Could not back up existing installation.'; Exit;
    end;
    // Keep the last different version across repeated installs. Backup failures
    // belong to PrepareToInstall so Inno cannot continue after a script exception.
    if OldVersion <> '{#AppVersion}' then
      if not FileCopy(ExpandConstant('{app}\sshm.exe'), ExpandConstant('{app}\sshm.previous.exe'), False) then begin
        Result := 'Could not preserve previous executable.'; Exit;
      end;
  end;
end;

procedure RestoreOld;
begin
  if not Changed or Completed then Exit;
  if PathAddedNow then RemoveOwnedPath;
  if HadOld then begin
    if not FileCopy(ExpandConstant('{tmp}\sshm-old.exe'), ExpandConstant('{app}\sshm.exe'), False) then
      Log('RECOVERY REQUIRED: copy sshm.previous.exe to sshm.exe after closing all sessions.');
    if not FileCopy(OldMarker, Marker, False) then Log('Could not restore installation marker.');
  end else begin
    DeleteFile(ExpandConstant('{app}\sshm.exe'));
    DeleteFile(Marker);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then begin
    Changed := True;
  end;
  if CurStep = ssPostInstall then begin
    if ReadVersion(ExpandConstant('{app}\sshm.exe')) <> '{#AppVersion}' then begin
      RestoreOld;
      RaiseException('Installed version verification failed. Previous executable restored; see setup log.');
    end;
    if not SetIniString('Install', 'Owner', Owner, Marker)
      or not SetIniString('Install', 'Version', '{#AppVersion}', Marker) then
      RaiseException('Could not write installation marker.');
    AddPath;
    Completed := True;
  end;
end;

function GetCustomSetupExitCode: Integer;
begin
  // Inno logs exceptions in ssPostInstall but may otherwise return success.
  // Ensure both the CLI supervisor and unattended callers see a real failure.
  if Changed and not Completed then Result := 10 else Result := 0;
end;

procedure DeinitializeSetup;
begin
  RestoreOld;
end;

function InitializeUninstall: Boolean;
begin
  Result := WritableExe(ExpandConstant('{app}\sshm.exe'));
  if not Result then SuppressibleMsgBox('Exit all sshm sessions before uninstalling. User data will be preserved.', mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    if GetIniString('Install', 'PathOwned', '0', Marker) = '1' then RemoveOwnedPath;
end;
