//! Identité d'un élément suggéré (site ou appli) et filtres des données brutes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ItemKey {
    /// Hôte sans `www.`.
    Site(String),
    /// Clé d'appli (`apps::app_key`).
    App(String),
}

impl ItemKey {
    /// Forme texte stable (`site:<hôte>`, `app:<clé>`), pour la base et l'interface.
    pub fn as_str(&self) -> String {
        match self {
            ItemKey::Site(h) => format!("site:{h}"),
            ItemKey::App(a) => format!("app:{a}"),
        }
    }

    pub fn parse(s: &str) -> Option<ItemKey> {
        if let Some(h) = s.strip_prefix("site:") {
            Some(ItemKey::Site(h.to_owned()))
        } else {
            s.strip_prefix("app:").map(|a| ItemKey::App(a.to_owned()))
        }
    }
}

/// Hôte d'une adresse web, en minuscules et sans `www.`. `None` pour ce qui n'est pas un site
/// public : pages internes du navigateur, fichiers, `localhost`, adresses IP, `.local`.
pub fn site_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    if host_port.starts_with('[') {
        return None; // IPv6 littérale
    }
    let host = host_port.split(':').next()?.trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    let is_ip = host.split('.').count() == 4 && host.split('.').all(|p| p.parse::<u8>().is_ok());
    if host.is_empty() || host == "localhost" || host.ends_with(".local") || is_ip || !host.contains('.') {
        return None;
    }
    Some(host)
}

/// Vraie navigation (lien, saisie, favori, suggestion de la barre d'adresse) : ni sous-cadre,
/// ni rechargement, ni étape intermédiaire d'une redirection.
pub(crate) fn is_real_navigation(transition: i64) -> bool {
    const CHAIN_END: i64 = 0x2000_0000;
    const REDIRECTS: i64 = 0x4000_0000 | 0x8000_0000;
    let core = transition & 0xFF;
    if !matches!(core, 0 | 1 | 2 | 5) {
        return false;
    }
    transition & REDIRECTS == 0 || transition & CHAIN_END != 0
}

/// Horodatage Chromium (microsecondes depuis 1601, UTC) → secondes Unix.
pub(crate) fn chrome_time_to_unix(us_since_1601: i64) -> i64 {
    us_since_1601.div_euclid(1_000_000) - 11_644_473_600
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::app_key;

    #[test]
    fn site_host_strips_www_and_lowercases() {
        assert_eq!(site_host("https://WWW.YouTube.com/watch?v=1").as_deref(), Some("youtube.com"));
        assert_eq!(site_host("http://github.com:443/a").as_deref(), Some("github.com"));
    }

    #[test]
    fn site_host_keeps_subdomain() {
        assert_eq!(site_host("https://music.youtube.com/").as_deref(), Some("music.youtube.com"));
    }

    #[test]
    fn site_host_rejects_local() {
        for u in [
            "vivaldi://startpage",
            "chrome://settings",
            "http://localhost:5173/",
            "http://192.168.1.49:9090",
            "http://[::1]:80/",
            "file:///C:/a",
            "http://nas.local/",
            "https:///",
            "",
        ] {
            assert_eq!(site_host(u), None, "{u}");
        }
    }

    #[test]
    fn transitions_filter() {
        for t in [0, 1, 2, 5, 0x2000_0000 | 0x4000_0000 | 1] {
            assert!(is_real_navigation(t), "{t:#x}");
        }
        for t in [3, 4, 8, 0x4000_0000, 0x8000_0000_u32 as i64] {
            assert!(!is_real_navigation(t), "{t:#x}");
        }
    }

    #[test]
    fn chrome_time_epoch() {
        assert_eq!(chrome_time_to_unix(11_644_473_600_000_000), 0);
        assert_eq!(chrome_time_to_unix(11_644_473_601_500_000), 1);
    }

    #[test]
    fn app_key_ignores_squirrel_version_dir() {
        let a = app_key(r"C:\Users\u\AppData\Local\Discord\app-1.0.9\Discord.exe");
        let b = app_key(r"C:\Users\u\AppData\Local\Discord\app-1.0.10\Discord.exe");
        assert_eq!(a, r"discord\discord.exe");
        assert_eq!(a, b);
        assert_eq!(app_key(r"C:\Users\u\AppData\Local\Vivaldi\Application\vivaldi.exe"), r"vivaldi\vivaldi.exe");
        assert_eq!(
            app_key(r"C:\Program Files\Google\Chrome\Application\120.0.1\chrome.exe"),
            r"chrome\chrome.exe"
        );
        assert_eq!(app_key(r"C:\Tools\bin\x.exe"), r"tools\x.exe");
        assert_eq!(app_key("x.exe"), "x.exe");
    }

    #[test]
    fn app_key_keeps_aumid() {
        let id = "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App";
        assert_eq!(app_key(id), id);
    }

    #[test]
    fn item_key_roundtrip() {
        for k in [ItemKey::Site("youtube.com".into()), ItemKey::App(r"discord\discord.exe".into())] {
            assert_eq!(ItemKey::parse(&k.as_str()), Some(k.clone()));
        }
        assert_eq!(ItemKey::parse("autre:x"), None);
    }
}
