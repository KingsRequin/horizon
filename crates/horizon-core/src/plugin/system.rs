//! Actions sur le système qu'un plugin peut demander (`permissions.system`) : sonde d'une
//! machine, terminal SSH, lien quelconque dans le navigateur. Ici, ce qui se vérifie sans
//! Windows : la forme d'une cible.

use url::Url;

/// Noms acceptés dans `permissions.system`.
pub const SYSTEM: [&str; 3] = ["probe", "ssh", "open"];

/// Cible de `horizon.net.probe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeTarget {
    /// Port TCP ouvert.
    Tcp { host: String, port: u16 },
    /// Réponse HTTP quelconque à cette adresse.
    Http(String),
}

/// Cible lue : `hôte:port`, `[ipv6]:port`, ou une adresse `http://` / `https://` (sans nom
/// d'utilisateur ni mot de passe). `None` : forme refusée.
pub fn probe_target(s: &str) -> Option<ProbeTarget> {
    let s = s.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        let u = Url::parse(s).ok()?;
        return (u.host_str().is_some() && u.username().is_empty() && u.password().is_none()).then(|| ProbeTarget::Http(u.to_string()));
    }
    let (host, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let ok = (1..=253).contains(&host.len()) && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    ok.then(|| ProbeTarget::Tcp { host: host.to_owned(), port })
}

/// Nom transmissible tel quel à `ssh` : pas d'option déguisée (`-oProxyCommand=…`), pas
/// d'espace ni de caractère de contrôle ou de guillemet.
pub fn safe_ssh_target(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '[' | ']'))
}

/// Nom en clair d'une action (fenêtre d'accord).
pub(super) fn label(name: &str) -> &str {
    match name {
        "probe" => "Tester si des machines du réseau répondent",
        "ssh" => "Ouvrir un terminal SSH",
        "open" => "Ouvrir des liens dans le navigateur",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_targets() {
        let tcp = |h: &str, p| Some(ProbeTarget::Tcp { host: h.into(), port: p });
        assert_eq!(probe_target("nas.local:22"), tcp("nas.local", 22));
        assert_eq!(probe_target(" 192.168.1.20:5000 "), tcp("192.168.1.20", 5000));
        assert_eq!(probe_target("[fe80::1]:22"), tcp("fe80::1", 22));
        assert_eq!(probe_target("https://nas.local:5001/x"), Some(ProbeTarget::Http("https://nas.local:5001/x".into())));
        assert_eq!(probe_target("http://192.168.1.1"), Some(ProbeTarget::Http("http://192.168.1.1/".into())));
        for bad in ["nas.local", "nas.local:0", "nas.local:70000", ":22", "a b:22", "ftp://nas.local", "https://u:p@nas.local", "nas;x:22"] {
            assert_eq!(probe_target(bad), None, "{bad}");
        }
    }

    #[test]
    fn ssh_targets_cannot_carry_an_option() {
        for ok in ["nas", "moi@192.168.1.20", "[fe80::1]", "srv-1.local"] {
            assert!(safe_ssh_target(ok), "{ok}");
        }
        for bad in ["", "-oProxyCommand=calc", "nas; calc", "nas calc", "nas\"", "a`b", "a$b"] {
            assert!(!safe_ssh_target(bad), "{bad}");
        }
    }
}
