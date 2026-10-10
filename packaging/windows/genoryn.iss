; GENORYN Windows installer (Inno Setup 6). Build with scripts/release/build_installer.py,
; which passes the verified package directory produced by scripts/release/package.py.
; Installs per user by default (no administrator rights), upgrades in place and never
; touches wallets or chain data in %USERPROFILE%\.genoryn.

#ifndef AppVersion
  #error AppVersion must be defined
#endif
#ifndef SourceDir
  #error SourceDir must point at the verified package directory
#endif
#ifndef RepoDir
  #error RepoDir must point at the repository root
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
; /DLite=1 builds GENORYN Lite: the wallet and full node from a lite package, without the
; miner. It has its own AppId, folder and shortcuts, so both editions can be installed.
#ifdef Lite
  #define AppName "GENORYN Lite"
  #define AppGuid "{{9C3F2D71-5B8E-4E0A-A6D4-1F7B2C93E845}"
  #define SetupName "GENORYN-Lite-Setup-"
  #define Edition "Lite edition: wallet and full node, without mining."
#else
  #define AppName "GENORYN"
  #define AppGuid "{{4E1B8C2A-7D3F-4A6E-9B21-5C0D8E7F6A13}"
  #define SetupName "GENORYN-Setup-"
  #define Edition "Mainnet wallet, full node and CPU miner."
#endif

[Setup]
AppId={#AppGuid}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=GENORYN
AppPublisherURL=https://github.com/genoryn/genoryn
AppSupportURL=https://github.com/genoryn/genoryn/issues
AppUpdatesURL=https://github.com/genoryn/genoryn/releases/latest
AppComments=Private by design. Finite by protocol. {#Edition}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=no
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DisableWelcomePage=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename={#SetupName}{#AppVersion}
SetupIconFile={#RepoDir}\apps\genoryn-wallet\assets\genoryn.ico
UninstallDisplayIcon={app}\genoryn-wallet.exe
UninstallDisplayName={#AppName} {#AppVersion}
WizardStyle=modern dark windows11 hidebevels
WizardImageFile={#RepoDir}\branding\installer\wizard-large.png
WizardSmallImageFile={#RepoDir}\branding\installer\wizard-small.png
Compression=lzma2/ultra64
SolidCompression=yes
CloseApplications=force
RestartApplications=no
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Messages]
WelcomeLabel2=This will install [name/ver]: the desktop wallet with its own full node for the GENORYN mainnet. {#Edition}%n%nYour wallets and chain data stay in your user folder and are kept on update or uninstall.%n%nNew, independently unaudited software. Keep your recovery words safe and only mine or hold what you can afford to lose.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\genoryn-wallet.exe"; WorkingDir: "{app}"; Comment: "{#AppName} wallet and node"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\genoryn-wallet.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\genoryn-wallet.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Stop only the GENORYN programs running from this installation's folder.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -NonInteractive -Command ""Get-Process genoryn-miner,genoryn-wallet,genorynd -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -like '{app}\*' } | Stop-Process -Force"""; Flags: runhidden; RunOnceId: "StopGenoryn"

[Code]
const
  RuntimeUrl = 'https://aka.ms/vs/17/release/vc_redist.x64.exe';

var
  DownloadPage: TDownloadWizardPage;

function RuntimePresent: Boolean;
begin
  Result := FileExists(ExpandConstant('{sys}\vcruntime140.dll')) and
    FileExists(ExpandConstant('{sys}\vcruntime140_1.dll')) and
    FileExists(ExpandConstant('{sys}\msvcp140.dll'));
end;

#ifndef Signed
{ Smart App Control blocks unsigned programs without a per-app exception, so an
  unsigned build would install and then fail to start. 1 = enforcing. }
function SmartAppControlEnforced: Boolean;
var
  State: Cardinal;
begin
  Result := RegQueryDWordValue(HKLM64, 'SYSTEM\CurrentControlSet\Control\CI\Policy',
    'VerifiedAndReputablePolicyState', State) and (State = 1);
end;

function InitializeSetup: Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  if not SmartAppControlEnforced then
    exit;
  Result := False;
  if SuppressibleMsgBox('Smart App Control is turned on in Windows. It blocks every program that is not signed ' +
      'with a commercial code-signing certificate, and this GENORYN build is not signed yet, so Windows ' +
      'would refuse to start it after installation. Nothing has been installed.' + #13#10#13#10 +
      'To install GENORYN now, turn Smart App Control off: Windows Security > App & browser control > ' +
      'Smart App Control settings > Off. Then run this installer again.' + #13#10#13#10 +
      'Open the Smart App Control settings now?',
      mbError, MB_YESNO, IDNO) = IDYES then
    if not ShellExec('open', 'windowsdefender://smartapp', '', '', SW_SHOW, ewNoWait, ResultCode) then
      ShellExec('open', 'ms-settings:windowsdefender', '', '', SW_SHOW, ewNoWait, ResultCode);
end;
#endif

procedure InitializeWizard;
begin
  DownloadPage := CreateDownloadPage(SetupMessage(msgWizardPreparing),
    'Downloading the Microsoft Visual C++ runtime that GENORYN needs...', nil);
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  if (CurPageID <> wpReady) or RuntimePresent then
    exit;
  if MsgBox('GENORYN needs the Microsoft Visual C++ x64 runtime, which is not installed.' + #13#10 +
      'Download and install it from Microsoft now? Windows will ask for permission.',
      mbConfirmation, MB_YESNO) <> IDYES then
    exit;
  DownloadPage.Clear;
  DownloadPage.Add(RuntimeUrl, 'vc_redist.x64.exe', '');
  DownloadPage.Show;
  try
    try
      DownloadPage.Download;
      if not ShellExec('runas', ExpandConstant('{tmp}\vc_redist.x64.exe'),
          '/install /passive /norestart', '', SW_SHOW, ewWaitUntilTerminated, ResultCode) then
        MsgBox('The runtime installer could not start. Install it later from ' + RuntimeUrl,
          mbInformation, MB_OK);
    except
      MsgBox('The runtime could not be downloaded. Install it later from ' + RuntimeUrl,
        mbInformation, MB_OK);
    end;
  finally
    DownloadPage.Hide;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    MsgBox('{#AppName} was removed. Your wallets and chain data were kept in ' +
      ExpandConstant('{%USERPROFILE}') + '\.genoryn.', mbInformation, MB_OK);
end;
