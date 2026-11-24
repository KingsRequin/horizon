//! Pare-feu de Windows : lecture de ses règles (sans élévation) et pose de la règle d'Horizon
//! (élévation ponctuelle, demandée par l'utilisateur). La décision et la commande sont dans
//! `horizon_core::phone::firewall`.

use crate::com::ComGuard;
use anyhow::{Context, bail};
use horizon_core::phone::firewall::{FirewallState, PORT, RuleView, decide, netsh_script};
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, S_OK, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, INetFwRule3, NET_FW_ACTION_ALLOW, NET_FW_IP_PROTOCOL_ANY, NET_FW_IP_PROTOCOL_TCP, NET_FW_PROFILE2_PRIVATE,
    NET_FW_RULE_DIR_IN, NetFwPolicy2,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDispatch};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VariantClear};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, Interface, PCWSTR, w};

/// Temps laissé aux deux commandes `netsh` une fois l'élévation accordée.
const NETSH_TIMEOUT: Duration = Duration::from_secs(60);

/// État du pare-feu pour `exe` écoutant sur `port` ; `Unknown` si ses règles sont illisibles ou
/// si le pare-feu de Windows est coupé en réseau privé (un autre pare-feu décide alors).
pub fn state(exe: &Path, port: u16) -> FirewallState {
    match rules() {
        Ok(Some(rules)) => decide(&rules, &exe.to_string_lossy(), port),
        Ok(None) => FirewallState::Unknown,
        Err(e) => {
            log::warn!("pare-feu : règles illisibles : {e}");
            FirewallState::Unknown
        }
    }
}

/// Programme et arguments qui lancent `script` : le `cmd.exe` du système par son chemin complet
/// (pas celui que trouverait la recherche), sans la commande `AutoRun` du registre de
/// l'utilisateur (`/d`), qui tournerait sinon en administrateur, et sans développement de
/// « ! » (`/v:off`).
fn elevated_command(script: &str) -> (PathBuf, String) {
    let mut buffer = [0u16; 260];
    // SAFETY : `buffer` est un tampon valide, dont la taille est celle que l'appel reçoit.
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    let system = if len > 0 && len < buffer.len() { PathBuf::from(String::from_utf16_lossy(&buffer[..len])) } else { PathBuf::from(r"C:\Windows\System32") };
    (system.join("cmd.exe"), format!("/d /v:off /s /c \"{script}\""))
}

/// Remet les règles de `exe` en ordre (`netsh_script`), en administrateur : Windows demande
/// l'accord de l'utilisateur. Bloque jusqu'à sa réponse et la fin des commandes, donc jamais
/// sur le fil de l'interface. Rend l'état relu ; `Err` si l'élévation est refusée.
pub fn allow_elevated(exe: &Path) -> anyhow::Result<FirewallState> {
    let path = exe.to_string_lossy();
    // `%` serait développé par cmd, `"` fermerait l'argument : aucun chemin d'installation
    // d'Horizon n'en contient.
    if path.contains(['%', '"']) {
        bail!("chemin d'Horizon inattendu : {path}");
    }
    let (cmd, params) = elevated_command(&netsh_script(&path));
    let (cmd, params) = (HSTRING::from(cmd.as_os_str()), HSTRING::from(params));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(cmd.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY : `info` et les chaînes qu'elle désigne vivent jusqu'au retour de l'appel.
    unsafe { ShellExecuteExW(&mut info) }.context("autorisation d'administrateur refusée")?;
    if !info.hProcess.is_invalid() {
        // SAFETY : poignée de processus rendue par ShellExecuteExW (SEE_MASK_NOCLOSEPROCESS),
        // fermée ici une seule fois.
        unsafe {
            WaitForSingleObject(info.hProcess, NETSH_TIMEOUT.as_millis() as u32);
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(state(exe, PORT))
}

/// Règles du pare-feu ; `None` s'il est coupé pour le profil privé.
fn rules() -> windows::core::Result<Option<Vec<RuleView>>> {
    let _com = ComGuard::sta()?;
    // SAFETY : COM est initialisé sur ce fil (garde ci-dessus) ; les interfaces sont
    // libérées avant elle, à la sortie de la fonction.
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
        if policy.get_FirewallEnabled(NET_FW_PROFILE2_PRIVATE)? != VARIANT_TRUE {
            return Ok(None);
        }
        let enumerator: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
        let mut out = Vec::new();
        loop {
            let mut item = [VARIANT::default()];
            let mut fetched = 0u32;
            if enumerator.Next(&mut item, &mut fetched) != S_OK || fetched == 0 {
                break;
            }
            let rule = dispatch(&item[0]).and_then(|d| d.cast::<INetFwRule>().ok());
            let _ = VariantClear(&mut item[0]);
            if let Some(view) = rule.and_then(|r| view(&r)) {
                out.push(view);
            }
        }
        Ok(Some(out))
    }
}

/// L'objet porté par une variante de l'énumération.
///
/// # Safety
/// `item` vient de `IEnumVARIANT::Next` et n'a pas encore été vidée.
unsafe fn dispatch(item: &VARIANT) -> Option<IDispatch> {
    // SAFETY : le champ lu est celui que `vt` désigne.
    unsafe {
        let inner = &item.Anonymous.Anonymous;
        if inner.vt != VT_DISPATCH {
            return None;
        }
        (*inner.Anonymous.pdispVal).clone()
    }
}

/// La règle réduite à ce qui nous concerne ; `None` pour une règle propre à un service ou à
/// une appli empaquetée (elle ne peut ni nous autoriser ni nous bloquer), ou illisible.
///
/// # Safety
/// COM est initialisé sur ce fil.
unsafe fn view(rule: &INetFwRule) -> Option<RuleView> {
    // SAFETY : appels de lecture sur une interface vivante.
    unsafe {
        let text = |b: windows::core::Result<windows::core::BSTR>| b.map(|s| s.to_string()).unwrap_or_default();
        if !text(rule.ServiceName()).is_empty() {
            return None;
        }
        if rule.cast::<INetFwRule3>().is_ok_and(|r3| !text(r3.LocalAppPackageId()).is_empty()) {
            return None;
        }
        let protocol = rule.Protocol().ok()?;
        Some(RuleView {
            program: text(rule.ApplicationName()),
            ports: text(rule.LocalPorts()),
            allow: rule.Action().ok()? == NET_FW_ACTION_ALLOW,
            enabled: rule.Enabled().ok()? == VARIANT_TRUE,
            inbound: rule.Direction().ok()? == NET_FW_RULE_DIR_IN,
            tcp: protocol == NET_FW_IP_PROTOCOL_TCP.0 || protocol == NET_FW_IP_PROTOCOL_ANY.0,
            private: rule.Profiles().ok()? & NET_FW_PROFILE2_PRIVATE.0 != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_elevated_command_is_the_system_s_cmd_without_the_user_s_autorun() {
        let (file, params) = elevated_command("netsh x");
        let lower = file.to_string_lossy().to_lowercase();
        assert!(file.is_absolute() && lower.ends_with(r"\system32\cmd.exe"), "{lower}");
        // `/d` : pas de commande AutoRun du registre de l'utilisateur, qui tournerait élevée ;
        // `/v:off` : un « ! » du chemin reste un « ! ».
        assert_eq!(params, "/d /v:off /s /c \"netsh x\"");
    }

    /// Contrôle à la main : `cargo test -p horizon-sys firewall -- --ignored --nocapture`.
    #[test]
    #[ignore = "lit le pare-feu de ce PC"]
    fn show_the_firewall_of_this_pc() {
        let rules = rules().expect("lecture des règles");
        println!("règles lues : {:?}", rules.as_ref().map(Vec::len));
        let exe = std::env::current_exe().unwrap();
        println!("cet exécutable de test, port {PORT} : {:?}", state(&exe, PORT));
        if let Some(blocked) = rules.iter().flatten().find(|r| !r.allow && r.enabled && !r.program.is_empty()) {
            println!("un blocage existant : {} → {:?}", blocked.program, state(Path::new(&blocked.program), PORT));
        }
    }
}
