; Installateur d'Horizon (Inno Setup 6) : Horizon-Setup-<version>.exe, sans droits
; administrateur. Compilé par scripts\package.ps1 :
;   ISCC /DVersion=<x.y.z> /DBin=<dossier des exécutables> /DStage=<scripts à embarquer> /O<sortie>
; Fait ce que fait scripts\install.ps1 pour une installation depuis les sources : exécutables
; dans %LOCALAPPDATA%\Programs\Horizon, identité de paquet (notifications), démarrage avec
; Windows, menu Démarrer, hooks Claude Code. S'y ajoute la désinstallation depuis les
; applications installées de Windows.
; Fichier en UTF-8 avec BOM (sans lui, le compilateur lit les accents en ANSI).

#ifndef Version
  #error Version manquante : compiler par scripts\package.ps1
#endif

#define Exe "horizon.exe"
#define Hook "horizon-hook.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"

[Setup]
AppId={{71B70E6C-14F8-4AC1-B059-24BF2643951A}
AppName=Horizon
AppVersion={#Version}
AppPublisher=KingsRequin
AppPublisherURL=https://github.com/KingsRequin/horizon
AppSupportURL=https://github.com/KingsRequin/horizon/issues
AppUpdatesURL=https://github.com/KingsRequin/horizon/releases
VersionInfoVersion={#Version}
; Dossier fixe : celui des scripts et de l'exclusion antivirus conseillée.
DefaultDirName={localappdata}\Programs\Horizon
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputBaseFilename=Horizon-Setup-{#Version}
Compression=lzma2/max
SolidCompression=yes
SetupIconFile=..\natif\icons\horizon.ico
UninstallDisplayIcon={app}\{#Exe}
UninstallDisplayName=Horizon
WizardStyle=modern dynamic
ShowLanguageDialog=no
; Horizon est fermé proprement par StopHorizon (place en haut des écrans libérée).
CloseApplications=no

[Languages]
; Langue de Windows ; à défaut la première (anglais).
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Messages]
; La page des tâches est la dernière avant la copie (bouton « Installer », CurPageChanged).
en.SelectTasksLabel2=Select the additional tasks you would like Setup to perform while installing [name], then click Install.
fr.SelectTasksLabel2=Sélectionnez les tâches supplémentaires que l'assistant d'installation doit effectuer pendant l'installation de [name], puis cliquez sur Installer.
; Le texte français d'origine annonce des icônes sur le Bureau : il n'y en a pas.
fr.FinishedLabel=L'assistant a terminé l'installation de [name] sur votre ordinateur. L'application se lance depuis le menu Démarrer.

[CustomMessages]
en.AutoStart=Start Horizon with Windows
fr.AutoStart=Démarrer Horizon avec Windows
en.ClaudeHooks=Add the Claude Code hooks (session tracking; a backup of settings.json is kept)
fr.ClaudeHooks=Ajouter les hooks Claude Code (suivi des sessions ; settings.json est sauvegardé)
en.ShortcutComment=Toolbar at the top of every screen
fr.ShortcutComment=Barre d'outils en haut des écrans
en.StatusPackage=Registering Horizon for notifications...
fr.StatusPackage=Enregistrement d'Horizon pour les notifications...
en.StatusHooks=Adding the Claude Code hooks...
fr.StatusHooks=Ajout des hooks Claude Code...
en.RemoveSettings=Also delete your Horizon settings (bars, profiles, plugins)?
fr.RemoveSettings=Supprimer aussi tes réglages d'Horizon (barres, profils, plugins) ?

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"
Name: "claudehooks"; Description: "{cm:ClaudeHooks}"; Check: HasClaudeCode

[Dirs]
; Dossier retiré à la désinstallation même s'il existait avant (installation par les scripts).
Name: "{app}"; Flags: uninsalwaysuninstall

[Files]
Source: "{#Bin}\{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bin}\{#Hook}"; DestDir: "{app}"; Flags: ignoreversion
; Scripts lancés après la copie et à la désinstallation (RunScript).
Source: "{#Stage}\*"; DestDir: "{app}\setup"; Flags: ignoreversion recursesubdirs

[InstallDelete]
; Restes d'une mise à jour automatique (exécutables remplacés pendant qu'ils tournaient).
Type: files; Name: "{app}\*.old"

[Icons]
Name: "{userprograms}\Horizon"; Filename: "{app}\{#Exe}"; WorkingDir: "{app}"; Comment: "{cm:ShortcutComment}"

[Registry]
; Même valeur que « Démarrer avec Windows » dans le menu d'Horizon (retirée à la
; désinstallation par CurUninstallStepChanged, cochée ou non ici).
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "Horizon"; ValueData: """{app}\{#Exe}"""; Tasks: autostart
Root: HKCU; Subkey: "{#RunKey}"; ValueType: none; ValueName: "Horizon"; Flags: deletevalue; Tasks: not autostart

[Run]
Filename: "{app}\{#Exe}"; Description: "{cm:LaunchProgram,Horizon}"; Flags: nowait postinstall

[UninstallDelete]
; Version téléchargée en attente et restes d'un échange (mise à jour automatique).
Type: filesandordirs; Name: "{app}\update"
Type: files; Name: "{app}\*.old"
; Copie du manifeste du paquet faite par register-dev.ps1.
Type: filesandordirs; Name: "{localappdata}\Horizon\package"
Type: dirifempty; Name: "{localappdata}\Horizon"

[Code]
// Claude Code est installé : la case des hooks n'est proposée que dans ce cas.
function HasClaudeCode: Boolean;
begin
  Result := DirExists(GetEnv('USERPROFILE') + '\.claude');
end;

// Fichier ouvert par un processus (exécutable qui tourne).
function Locked(const Path: String): Boolean;
var
  Stream: TFileStream;
begin
  Result := False;
  try
    Stream := TFileStream.Create(Path, fmOpenReadWrite or fmShareExclusive);
    Stream.Free;
  except
    Result := True;
  end;
end;

// Fermeture propre d'Horizon (place réservée libérée), arrêt forcé en dernier recours.
procedure StopHorizon;
var
  Exe: String;
  Code, Tries: Integer;
begin
  Exe := ExpandConstant('{app}\{#Exe}');
  if not FileExists(Exe) then Exit;
  Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  for Tries := 1 to 50 do begin
    if not Locked(Exe) then Exit;
    Sleep(100);
  end;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#Exe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
end;

// Lance un script du dossier setup et attend sa fin. Un échec n'arrête rien : sans le mode
// développeur de Windows, l'identité de paquet est refusée et Horizon marche quand même.
procedure RunScript(const Name, Args: String);
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -File "' + ExpandConstant('{app}\setup\') + Name + '" ' + Args,
    '', SW_HIDE, ewWaitUntilTerminated, Code);
  Log(Format('%s %s : code %d', [Name, Args, Code]));
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopHorizon;
  Result := '';
end;

// Sans page « Prêt à installer », la page des tâches lance la copie : son bouton le dit.
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpSelectTasks then WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep <> ssPostInstall then Exit;
  WizardForm.StatusLabel.Caption := CustomMessage('StatusPackage');
  RunScript('register-dev.ps1', '-ExeDir "' + ExpandConstant('{app}') + '"');
  if WizardIsTaskSelected('claudehooks') then begin
    WizardForm.StatusLabel.Caption := CustomMessage('StatusHooks');
    RunScript('install-claude-hooks.ps1', '-HookExe "' + ExpandConstant('{app}\{#Hook}') + '"');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  case CurUninstallStep of
    usUninstall: begin
      StopHorizon;
      RegDeleteValue(HKEY_CURRENT_USER, '{#RunKey}', 'Horizon');
      RunScript('register-dev.ps1', '-Unregister');
      RunScript('install-claude-hooks.ps1', '-Uninstall');
    end;
    usPostUninstall:
      if SuppressibleMsgBox(CustomMessage('RemoveSettings'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
        DelTree(ExpandConstant('{userappdata}\Horizon'), True, True, True);
  end;
end;
