//! Recherche dans la fenêtre « Réglages d'Horizon » : chaque réglage y a une entrée (section,
//! groupe, libellé, mots en plus). La recherche ignore la casse et les accents, et demande que
//! chaque mot tapé se retrouve quelque part dans l'entrée.
//!
//! Les libellés reprennent ceux de `natif/ui/options/*.slint` : un réglage ajouté là s'ajoute
//! aussi ici.

/// Un réglage qu'on peut chercher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchEntry {
    /// Section de la navigation (`barres`, `apparence`…).
    pub section: &'static str,
    /// Titre du groupe dans la section.
    pub group: &'static str,
    pub label: &'static str,
    /// Mots qui ne sont pas dans le libellé mais qu'on pourrait taper.
    pub keywords: &'static str,
}

const fn e(section: &'static str, group: &'static str, label: &'static str, keywords: &'static str) -> SearchEntry {
    SearchEntry { section, group, label, keywords }
}

/// Libellé de chaque section, dans l'ordre de la navigation.
pub const SECTIONS: [(&str, &str); 9] = [
    ("barres", "Barres"),
    ("apparence", "Apparence"),
    ("son", "Son et média"),
    ("alertes", "Alertes"),
    ("minuteur", "Minuteur"),
    ("palette", "Palette"),
    ("plugins", "Plugins"),
    ("general", "Général"),
    ("etat", "État"),
];

/// Libellé d'une section (vide si elle est inconnue).
pub fn section_label(id: &str) -> &'static str {
    SECTIONS.iter().find(|(s, _)| *s == id).map_or("", |(_, l)| l)
}

/// Tous les réglages.
pub const ENTRIES: &[SearchEntry] = &[
    e("barres", "Écrans", "Style", "îlots graphique apparence barre"),
    e("barres", "Écrans", "Souris", "cliquable survol information clic interaction"),
    e("barres", "Écrans", "Hauteur", "taille tuiles px"),
    e("barres", "Écrans", "Masquer au-dessus d’une appli en plein écran", "cacher jeu vidéo autohide"),
    e("barres", "Réapparition au-dessus d’un plein écran", "Faire revenir la barre avec la souris", "réapparition jeu menu désactiver"),
    e("barres", "Réapparition au-dessus d’un plein écran", "Hauteur de la bande du haut", "réapparition souris px"),
    e("barres", "Réapparition au-dessus d’un plein écran", "Temps d’arrêt de la souris", "délai ms réapparition"),
    e("barres", "Écrans", "Musique : animation", "pulsation bords respiration onde ligne horizon lumière tourne comète kick basse aucune désactiver"),
    e("barres", "Écrans", "Musique : fond de la lecture en cours", "halo aurore spectre ruban points capsules visualiseur son aucun"),
    e("barres", "Écrans", "Musique : barre de progression", "lecture en cours ondule vivante simple"),
    e("apparence", "Musique", "Animations de la musique", "pulsation spectre visualiseur barres écran"),
    e("apparence", "Couleur du fond d’écran", "Prendre la couleur du fond d’écran quand rien ne joue", "wallpaper engine accent teinte"),
    e("apparence", "Couleur du fond d’écran", "L’appliquer aussi à la couleur d’accent de Windows", "accent menu démarrer"),
    e("apparence", "Nuit", "Début de la nuit", "heure soir"),
    e("apparence", "Nuit", "Fin de la nuit", "heure matin"),
    e("apparence", "Nuit", "Luminosité la nuit", "assombrir sombre opacité"),
    e("apparence", "Nuit", "« Ne pas déranger » pendant la nuit", "dnd silence"),
    e("son", "Volume et média", "Afficher le volume et la lecture dans la barre, à la place de Windows", "osd touches média son"),
    e("son", "Volume et média", "Annoncer chaque nouveau morceau", "titre artiste pochette musique"),
    e("son", "Casque éteint", "Basculer le son quand le casque s’éteint", "casque sortie audio enceintes"),
    e("son", "Casque éteint", "Sortie de repli", "enceintes écran audio"),
    e("alertes", "Quand Claude attend ou a terminé", "Montrer le projet et le message dans l’îlot", "claude session terminal"),
    e("alertes", "Quand Claude attend ou a terminé", "Signal lumineux (plugin Prisme)", "lumière rgb éclairage"),
    e("alertes", "Captures d’écran vers Claude", "Proposer chaque nouvelle capture à la session Claude qui attend", "capture image screenshot"),
    e("alertes", "Pendant « Ne pas déranger »", "« Claude attend »", "dnd silence"),
    e("alertes", "Pendant « Ne pas déranger »", "Les alertes d’état", "dnd batterie"),
    e("alertes", "« Ne pas déranger » change aussi", "Discord en « Ne pas déranger »", "statut"),
    e("alertes", "« Ne pas déranger » change aussi", "Plugin Prisme : scène « focus »", "lumière éclairage"),
    e("alertes", "Batterie faible", "Casque", "seuil pourcentage"),
    e("minuteur", "Durées proposées", "Durées proposées", "minutes chronomètre"),
    e("minuteur", "Pomodoro", "Travail", "pomodoro minutes"),
    e("minuteur", "Pomodoro", "Pause", "pomodoro minutes"),
    e("palette", "Palette de commandes", "Raccourci", "clavier touche ctrl alt"),
    e("plugins", "Installés", "Plugins installés", "widget extension autoriser permissions"),
    e("plugins", "Installés", "Nouveau plugin", "créer modèle développer"),
    e("plugins", "Bibliothèque", "Bibliothèque de plugins", "github télécharger installer mettre à jour retirer désinstaller"),
    e("general", "Démarrage", "Démarrer avec Windows", "lancement session autostart"),
    e("general", "Démarrage", "Alléger Horizon pendant un jeu en plein écran", "mode jeu performance"),
    e("general", "Mises à jour", "Installer automatiquement les nouvelles versions", "github version"),
    e("general", "Profils", "Gérer les profils", "disposition"),
    e("general", "Fichier", "settings.toml", "fichier réglages chemin"),
    e("etat", "État", "État des intégrations", "diagnostic santé erreur"),
    e("etat", "Journal", "Ouvrir le journal", "log horizon.log avertissements"),
];

/// Minuscules sans accents (le texte des réglages est en français).
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            '’' => '\'',
            c => c,
        })
        .collect()
}

/// Réglages où se retrouve chaque mot de `query` (vide : aucun), dans l'ordre de la fenêtre.
pub fn search(query: &str) -> Vec<&'static SearchEntry> {
    let words: Vec<String> = fold(query).split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return Vec::new();
    }
    ENTRIES
        .iter()
        .filter(|e| {
            let hay = fold(&format!("{} {} {} {}", e.label, e.group, section_label(e.section), e.keywords));
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_in_a_known_section() {
        for e in ENTRIES {
            assert!(!section_label(e.section).is_empty(), "{}", e.label);
        }
    }

    #[test]
    fn accents_and_case_do_not_matter() {
        let hits = search("LUMINOSITE");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].section, "apparence");
        assert_eq!(search("éCRAN plein").first().map(|e| e.label), Some("Masquer au-dessus d’une appli en plein écran"));
    }

    #[test]
    fn every_word_must_match() {
        assert!(search("casque").len() >= 2);
        assert_eq!(search("casque repli").len(), 1);
        assert!(search("casque introuvable").is_empty());
        assert!(search("   ").is_empty());
    }

    #[test]
    fn section_name_and_keywords_are_searched() {
        assert!(search("son et media").iter().all(|e| e.section == "son"));
        assert_eq!(search("osd").len(), 1);
    }
}
