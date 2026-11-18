//! Profils : dispositions complètes des barres, choisies à la main ou activées par l'appli
//! au premier plan.

use super::{BarLayout, Layout, new_id};
use serde::{Deserialize, Serialize};

/// Identifiant du profil créé pour une disposition d'avant les profils.
pub(crate) const DEFAULT_PROFILE: &str = "normal";

/// Profil : une disposition complète des barres, qu'on choisit dans le menu de la barre ou
/// qui s'applique seule tant qu'une de ses applis est au premier plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Clés d'applis (`apps::app_key`) qui activent ce profil.
    #[serde(default)]
    pub apps: Vec<String>,
    /// Barres du profil ; vide pour le profil actif (ses barres sont `Layout::bars`).
    #[serde(default)]
    pub bars: Vec<BarLayout>,
}

impl Profile {
    pub(super) fn default_profile() -> Profile {
        Profile { id: DEFAULT_PROFILE.into(), name: "Normal".into(), apps: vec![], bars: vec![] }
    }
}

impl Layout {
    /// Garantit au moins un profil, un profil actif qui existe, et un retour valide.
    pub(crate) fn ensure_profiles(&mut self) {
        if self.profiles.is_empty() {
            self.profiles.push(Profile::default_profile());
        }
        if !self.profiles.iter().any(|p| p.id == self.active_profile) {
            self.active_profile = self.profiles[0].id.clone();
            self.auto_from = None;
        }
        if self.auto_from.as_ref().is_some_and(|f| !self.profiles.iter().any(|p| &p.id == f)) {
            self.auto_from = None;
        }
    }

    fn profile_index(&self, id: &str) -> anyhow::Result<usize> {
        self.profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| anyhow::anyhow!("profil inconnu : {id}"))
    }

    /// Active un profil ; les barres actuelles retournent dans le profil actif. `auto` = activé
    /// par une appli au premier plan (le profil choisi par l'utilisateur est retenu pour le retour).
    pub fn switch_profile(&mut self, id: &str, auto: bool) -> anyhow::Result<()> {
        let to = self.profile_index(id)?;
        let manual = self.auto_from.clone().unwrap_or_else(|| self.active_profile.clone());
        if id != self.active_profile {
            let from = self.profile_index(&self.active_profile)?;
            let target = std::mem::take(&mut self.profiles[to].bars);
            let current = std::mem::replace(&mut self.bars, target);
            if self.bars.is_empty() {
                // Profil sans barres (fichier modifié à la main) : copie de l'actuel.
                self.bars = current.clone();
            }
            self.profiles[from].bars = current;
            self.active_profile = id.to_owned();
        }
        self.auto_from = (auto && id != manual).then_some(manual);
        Ok(())
    }

    /// Nouveau profil, copie de la disposition actuelle, activé si `activate`. Renvoie son id.
    pub fn add_profile(&mut self, name: &str, activate: bool) -> anyhow::Result<String> {
        let id = new_id();
        self.profiles.push(Profile { id: id.clone(), name: name.to_owned(), apps: vec![], bars: self.bars.clone() });
        if activate {
            self.switch_profile(&id, false)?;
        }
        Ok(id)
    }

    /// Nom libre pour un nouveau profil (« Profil 2 », « Profil 3 »…).
    pub fn next_profile_name(&self) -> String {
        (2..)
            .map(|n| format!("Profil {n}"))
            .find(|n| !self.profiles.iter().any(|p| &p.name == n))
            .expect("nom libre")
    }

    pub fn rename_profile(&mut self, id: &str, name: &str) -> anyhow::Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "nom vide");
        let i = self.profile_index(id)?;
        self.profiles[i].name = name.to_owned();
        Ok(())
    }

    /// Applis d'un profil ; une appli n'active qu'un profil (elle est retirée des autres).
    pub fn set_profile_apps(&mut self, id: &str, mut apps: Vec<String>) -> anyhow::Result<()> {
        let i = self.profile_index(id)?;
        apps.sort();
        apps.dedup();
        for (j, p) in self.profiles.iter_mut().enumerate() {
            if j != i {
                p.apps.retain(|a| !apps.contains(a));
            }
        }
        self.profiles[i].apps = apps;
        Ok(())
    }

    /// Supprime un profil (jamais le dernier) ; s'il est actif, le premier autre est activé.
    pub fn delete_profile(&mut self, id: &str) -> anyhow::Result<()> {
        self.profile_index(id)?;
        anyhow::ensure!(self.profiles.len() > 1, "le dernier profil ne peut pas être supprimé");
        if self.active_profile == id {
            let other = self.profiles.iter().find(|p| p.id != id).expect("autre profil").id.clone();
            self.switch_profile(&other, false)?;
        }
        if self.auto_from.as_deref() == Some(id) {
            self.auto_from = None;
        }
        let i = self.profile_index(id)?;
        self.profiles.remove(i);
        Ok(())
    }

    /// Profil voulu pour l'appli au premier plan (`None` = rien à changer) : celui qui la
    /// liste, sinon le retour au profil choisi par l'utilisateur.
    pub fn profile_for_app(&self, app_key: Option<&str>) -> Option<String> {
        let ruled = app_key.and_then(|k| self.profiles.iter().find(|p| p.apps.iter().any(|a| a == k)));
        let wanted = match (ruled, &self.auto_from) {
            (Some(p), _) => p.id.clone(),
            (None, Some(manual)) => manual.clone(),
            (None, None) => return None,
        };
        (wanted != self.active_profile).then_some(wanted)
    }
}

/// Profil automatique : suit l'appli au premier plan et dit quand activer le profil qui la
/// liste, ou revenir au profil choisi par l'utilisateur. Le retour attend que l'appli soit
/// quittée depuis `delay`, pour qu'un Alt-Tab rapide ne fasse pas tout redessiner.
#[derive(Debug)]
pub struct ProfileFollower {
    delay: std::time::Duration,
    /// Clé de l'appli au premier plan (la dernière vraie appli : bureau et menu Démarrer ne
    /// comptent pas).
    app: Option<String>,
    /// Retour au profil de l'utilisateur prévu à cet instant.
    pending: Option<std::time::Instant>,
}

impl ProfileFollower {
    pub fn new(delay: std::time::Duration) -> ProfileFollower {
        ProfileFollower { delay, app: None, pending: None }
    }

    /// Nouvelle appli au premier plan (`apps::app_key`).
    pub fn foreground(&mut self, app_key: String) {
        self.app = Some(app_key);
    }

    /// Instant où `next` aura peut-être un retour à faire.
    pub fn deadline(&self) -> Option<std::time::Instant> {
        self.pending
    }

    /// Profil à activer maintenant d'après la disposition, s'il y en a un.
    pub fn next(&mut self, l: &Layout, now: std::time::Instant) -> Option<String> {
        let Some(wanted) = l.profile_for_app(self.app.as_deref()) else {
            self.pending = None;
            return None;
        };
        let ruled = self.app.as_deref().is_some_and(|k| l.profiles.iter().any(|p| p.apps.iter().any(|a| a == k)));
        if !ruled {
            // Retour au profil choisi : seulement après le délai.
            match self.pending {
                None => {
                    self.pending = Some(now + self.delay);
                    return None;
                }
                Some(p) if now < p => return None,
                Some(_) => {}
            }
        }
        self.pending = None;
        Some(wanted)
    }
}

