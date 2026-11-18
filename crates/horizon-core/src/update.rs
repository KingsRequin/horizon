//! Mise à jour automatique : logique pure. Lecture de la dernière Release GitHub, choix des
//! fichiers à télécharger, contrôle des empreintes SHA-256, et échange des exécutables dans le
//! dossier d'installation (l'exécutable qui tourne peut être renommé sous Windows, pas
//! remplacé : il devient `<nom>.old`, effacé au démarrage suivant).
//!
//! Une Release publiée par `scripts/package.ps1` joint `horizon.exe`, `horizon-hook.exe` et
//! `SHA256SUMS` (« <empreinte>  <nom> » par ligne). Les fichiers téléchargés attendent dans
//! `<dossier d'installation>/update/`, avec `version.txt` écrit en dernier : sans lui, rien n'est
//! prêt (téléchargement interrompu).

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Exécutables remplacés par une mise à jour (tous dans le dossier d'installation).
pub const FILES: [&str; 2] = ["horizon.exe", "horizon-hook.exe"];
/// Empreintes des fichiers de la Release.
pub const SUMS: &str = "SHA256SUMS";
/// Sous-dossier du dossier d'installation où attend une mise à jour téléchargée.
pub const STAGE_DIR: &str = "update";
/// Marque d'une mise à jour complète et vérifiée (sa version), écrite en dernier.
pub const READY: &str = "version.txt";
/// Suffixe des exécutables remplacés pendant qu'ils tournaient.
pub const OLD: &str = ".old";

/// Version « majeur.mineur.correctif » (un `v` devant est accepté, un suffixe `-beta…` refusé :
/// les préversions ne sont pas proposées).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let mut parts = s.split('.');
        let mut next = || parts.next()?.parse::<u32>().ok();
        let v = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(v)
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.0, self.1, self.2).cmp(&(o.0, o.1, o.2))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Adresse de la dernière Release publiée (ni brouillon, ni préversion) d'un dépôt.
pub fn latest_url(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases/latest")
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Mise à jour proposée : version, page de la Release (notes) et adresses des fichiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: Version,
    pub notes_url: String,
    /// Adresse de `SHA256SUMS`.
    pub sums_url: String,
    /// (nom, adresse) de chaque fichier de `FILES`, dans cet ordre.
    pub files: Vec<(String, String)>,
}

/// Lecture de la réponse de l'API.
#[derive(Debug, PartialEq, Eq)]
pub enum Latest {
    /// Une version plus récente, avec tous ses fichiers.
    Newer(Offer),
    /// Rien de plus récent (la version publiée, si elle se lit).
    UpToDate(Option<Version>),
    /// Release plus récente mais incomplète, ou réponse illisible.
    Invalid(String),
}

/// Compare la réponse de `latest_url(repo)` à la version qui tourne. Seuls les fichiers des
/// Releases de `repo` sont acceptés (`https://github.com/<repo>/releases/download/…`).
pub fn read_latest(json: &str, repo: &str, current: Version) -> Latest {
    let r: ApiRelease = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(e) => return Latest::Invalid(format!("réponse illisible : {e}")),
    };
    let Some(version) = Version::parse(&r.tag_name) else {
        return Latest::Invalid(format!("étiquette « {} » sans version", r.tag_name));
    };
    if r.draft || r.prerelease || version <= current {
        return Latest::UpToDate(Some(version));
    }
    let prefix = format!("https://github.com/{repo}/releases/download/");
    let url = |name: &str| {
        r.assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| a.browser_download_url.clone())
            .filter(|u| u.starts_with(&prefix) && !u.contains(".."))
    };
    let Some(sums_url) = url(SUMS) else { return Latest::Invalid(format!("{version} : {SUMS} absent de la Release")) };
    let mut files = Vec::new();
    for name in FILES {
        match url(name) {
            Some(u) => files.push((name.to_owned(), u)),
            None => return Latest::Invalid(format!("{version} : {name} absent de la Release")),
        }
    }
    Latest::Newer(Offer { version, notes_url: r.html_url, sums_url, files })
}

/// Empreintes de `SHA256SUMS` : nom → empreinte en minuscules (lignes « <hex>  <nom> » ou
/// « <hex> *<nom> »).
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*').trim();
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_ascii_lowercase(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// Empreinte SHA-256 en hexadécimal minuscule.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `bytes` correspond-il à l'empreinte de `name` dans `sums` ?
pub fn verify(sums: &HashMap<String, String>, name: &str, bytes: &[u8]) -> Result<(), String> {
    let expected = sums.get(&name.to_ascii_lowercase()).ok_or_else(|| format!("{name} absent de {SUMS}"))?;
    if *expected == sha256_hex(bytes) { Ok(()) } else { Err(format!("{name} : empreinte différente, fichier refusé")) }
}

fn stage_dir(install: &Path) -> PathBuf {
    install.join(STAGE_DIR)
}

/// Repart d'un dossier d'attente vide (téléchargement précédent abandonné).
pub fn stage_reset(install: &Path) -> std::io::Result<PathBuf> {
    let d = stage_dir(install);
    if d.exists() {
        std::fs::remove_dir_all(&d)?;
    }
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Pose un fichier vérifié dans le dossier d'attente (écrit à part, puis renommé).
pub fn stage_file(install: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let d = stage_dir(install);
    let part = d.join(format!("{name}.part"));
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, d.join(name))
}

/// Marque la mise à jour comme complète (dernier geste du téléchargement).
pub fn stage_ready(install: &Path, version: Version) -> std::io::Result<()> {
    std::fs::write(stage_dir(install).join(READY), version.to_string())
}

/// Version qui attend, complète (marque et tous les fichiers présents).
pub fn staged(install: &Path) -> Option<Version> {
    let d = stage_dir(install);
    let v = Version::parse(&std::fs::read_to_string(d.join(READY)).ok()?)?;
    FILES.iter().all(|f| d.join(f).is_file()).then_some(v)
}

/// Retire le dossier d'attente (installée, ou plus ancienne que la version qui tourne).
pub fn stage_clear(install: &Path) {
    let _ = std::fs::remove_dir_all(stage_dir(install));
}

/// Échange les exécutables : chacun devient `<nom>.old`, le nouveau prend sa place. Au premier
/// échec, ce qui a déjà été échangé est remis comme avant. Le dossier d'attente est retiré
/// après un échange complet.
pub fn swap(install: &Path) -> Result<(), String> {
    let d = stage_dir(install);
    let mut done: Vec<&str> = Vec::new();
    let mut result = Ok(());
    for name in FILES {
        let cur = install.join(name);
        let old = install.join(format!("{name}{OLD}"));
        let _ = std::fs::remove_file(&old);
        let step = (|| {
            if cur.exists() {
                std::fs::rename(&cur, &old).map_err(|e| format!("{name} : renommage impossible ({e})"))?;
            }
            std::fs::rename(d.join(name), &cur).map_err(|e| {
                // Remis en place tout de suite : l'appli doit rester lançable.
                let _ = std::fs::rename(&old, &cur);
                format!("{name} : mise en place impossible ({e})")
            })
        })();
        if let Err(e) = step {
            result = Err(e);
            break;
        }
        done.push(name);
    }
    if result.is_err() {
        rollback(install, &done);
        return result;
    }
    stage_clear(install);
    Ok(())
}

/// Annule un échange : les `.old` reprennent leur place (nouvel exe qui ne démarre pas).
pub fn rollback(install: &Path, names: &[&str]) {
    for name in names {
        let old = install.join(format!("{name}{OLD}"));
        if old.exists() {
            let cur = install.join(name);
            let _ = std::fs::remove_file(&cur);
            let _ = std::fs::rename(&old, &cur);
        }
    }
}

/// Efface les `.old` d'un échange précédent (ils ne tournent plus).
pub fn remove_old(install: &Path) {
    for name in FILES {
        let _ = std::fs::remove_file(install.join(format!("{name}{OLD}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const V010: Version = Version(0, 1, 0);

    #[test]
    fn versions() {
        assert_eq!(Version::parse("v0.2.0"), Some(Version(0, 2, 0)));
        assert_eq!(Version::parse(" 1.10.3\n"), Some(Version(1, 10, 3)));
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1.2.3-beta"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(0, 2, 1).to_string(), "0.2.1");
    }

    fn read_latest_t(json: &str, current: Version) -> Latest {
        read_latest(json, "o/r", current)
    }

    fn release(tag: &str, assets: &[&str]) -> String {
        let assets: Vec<_> = assets
            .iter()
            .map(|n| serde_json::json!({ "name": n, "browser_download_url": format!("https://github.com/o/r/releases/download/{tag}/{n}") }))
            .collect();
        serde_json::json!({ "tag_name": tag, "html_url": "https://github.com/o/r/releases/tag/x", "assets": assets }).to_string()
    }

    #[test]
    fn newer_release_with_all_files() {
        let json = release("v0.2.0", &["Horizon-0.2.0.zip", "horizon.exe", "horizon-hook.exe", "SHA256SUMS"]);
        let Latest::Newer(o) = read_latest(&json, "o/r", V010) else { panic!("attendu : plus récente") };
        assert_eq!(o.version, Version(0, 2, 0));
        assert!(o.sums_url.ends_with("/SHA256SUMS"));
        assert_eq!(o.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), FILES);
    }

    #[test]
    fn same_older_draft_or_prerelease_is_up_to_date() {
        let all = ["horizon.exe", "horizon-hook.exe", "SHA256SUMS"];
        assert_eq!(read_latest_t(&release("v0.1.0", &all), V010), Latest::UpToDate(Some(V010)));
        assert_eq!(read_latest_t(&release("0.0.9", &all), V010), Latest::UpToDate(Some(Version(0, 0, 9))));
        let mut pre: serde_json::Value = serde_json::from_str(&release("v0.3.0", &all)).unwrap();
        pre["prerelease"] = true.into();
        assert!(matches!(read_latest(&pre.to_string(), "o/r", V010), Latest::UpToDate(_)));
    }

    #[test]
    fn incomplete_or_foreign_release_is_refused() {
        assert!(matches!(read_latest_t(&release("v0.2.0", &["horizon.exe", "SHA256SUMS"]), V010), Latest::Invalid(_)));
        assert!(matches!(read_latest_t(&release("v0.2.0", &["horizon.exe", "horizon-hook.exe"]), V010), Latest::Invalid(_)));
        assert!(matches!(read_latest_t(&release("nightly", &[]), V010), Latest::Invalid(_)));
        assert!(matches!(read_latest("pas du json", "o/r", V010), Latest::Invalid(_)));
        let mut j: serde_json::Value = serde_json::from_str(&release("v0.2.0", &["horizon.exe", "horizon-hook.exe", "SHA256SUMS"])).unwrap();
        j["assets"][0]["browser_download_url"] = "https://exemple.org/horizon.exe".into();
        assert!(matches!(read_latest(&j.to_string(), "o/r", V010), Latest::Invalid(_)), "fichier hors de github.com");
        j["assets"][0]["browser_download_url"] = "https://github.com/autre/r/releases/download/v0.2.0/horizon.exe".into();
        assert!(matches!(read_latest(&j.to_string(), "o/r", V010), Latest::Invalid(_)), "fichier d'un autre dépôt");
    }

    #[test]
    fn sums_and_verification() {
        let exe = b"nouvel exe";
        let text = format!("{}  horizon.exe\n{} *Horizon-Hook.exe\nligne fausse\n", sha256_hex(exe), "a".repeat(64));
        let sums = parse_sums(&text);
        assert_eq!(sums.len(), 2);
        assert!(verify(&sums, "horizon.exe", exe).is_ok());
        assert!(verify(&sums, "horizon.exe", b"autre chose").is_err());
        assert!(verify(&sums, "horizon-hook.exe", exe).is_err());
        assert!(verify(&sums, "absent.exe", exe).is_err());
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    fn installed(d: &TestDir) -> PathBuf {
        let install = d.join("Horizon");
        std::fs::create_dir_all(&install).unwrap();
        for f in FILES {
            std::fs::write(install.join(f), "ancien").unwrap();
        }
        install
    }

    #[test]
    fn stage_then_swap() {
        let d = TestDir::new("update-swap");
        let install = installed(&d);
        stage_reset(&install).unwrap();
        for f in FILES {
            stage_file(&install, f, b"neuf").unwrap();
        }
        assert_eq!(staged(&install), None, "pas prête sans la marque");
        stage_ready(&install, Version(0, 2, 0)).unwrap();
        assert_eq!(staged(&install), Some(Version(0, 2, 0)));
        swap(&install).unwrap();
        for f in FILES {
            assert_eq!(std::fs::read_to_string(install.join(f)).unwrap(), "neuf");
            assert_eq!(std::fs::read_to_string(install.join(format!("{f}{OLD}"))).unwrap(), "ancien");
        }
        assert!(!install.join(STAGE_DIR).exists(), "attente retirée");
        remove_old(&install);
        assert!(FILES.iter().all(|f| !install.join(format!("{f}{OLD}")).exists()));
    }

    #[test]
    fn failed_swap_puts_everything_back() {
        let d = TestDir::new("update-rollback");
        let install = installed(&d);
        stage_reset(&install).unwrap();
        // Seul le premier fichier attend : le second échoue, le premier est remis.
        stage_file(&install, FILES[0], b"neuf").unwrap();
        assert!(swap(&install).is_err());
        for f in FILES {
            assert_eq!(std::fs::read_to_string(install.join(f)).unwrap(), "ancien", "{f}");
        }
    }

    #[test]
    fn interrupted_download_is_not_ready() {
        let d = TestDir::new("update-part");
        let install = installed(&d);
        stage_reset(&install).unwrap();
        stage_file(&install, FILES[0], b"neuf").unwrap();
        std::fs::write(install.join(STAGE_DIR).join(READY), "0.2.0").unwrap();
        assert_eq!(staged(&install), None, "un fichier manque");
        stage_clear(&install);
        assert_eq!(staged(&install), None);
    }
}
