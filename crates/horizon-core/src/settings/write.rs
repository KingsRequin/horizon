//! Texte de `settings.toml` écrit par Horizon : chaque option avec un commentaire qui la
//! décrit, pour qui ouvre le fichier à la main.

use super::Settings;
use std::fmt::Write;

/// Chaîne TOML entre guillemets (caractères spéciaux échappés).
fn quoted(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

/// Ligne `clé = valeur`, ou mise en commentaire si la valeur n'est pas réglée.
fn line<T: std::fmt::Display>(out: &mut String, key: &str, v: Option<T>, unset: &str) {
    let _ = match v {
        Some(v) => writeln!(out, "{key} = {v}"),
        None => writeln!(out, "# {key} = {unset}"),
    };
}

impl Settings {
    /// Fichier complet, commenté.
    pub fn to_toml(&self) -> String {
        let mut o = String::new();
        let n = &self.night;
        let a = &self.autohide;
        let al = &self.alerts;
        let t = &self.timer;
        let _ = write!(
            o,
            "# Réglages d'Horizon. Modifiables ici ou dans « Réglages d'Horizon » (menu de la barre,\n\
             # icône de notification, palette) ; un fichier illisible n'est jamais écrasé.\n\
             \n\
             [night]\n\
             # Plage de nuit (HH:MM, peut passer minuit) : les barres non cliquables s'assombrissent.\n\
             start = {}\n\
             end = {}\n\
             # Luminosité de ces barres la nuit (0 à 1 ; 1 = normale).\n\
             opacity = {}\n\
             # « Ne pas déranger » s'active de lui-même pendant la nuit.\n\
             dnd = {}\n\
             \n\
             [autohide]\n\
             # Réapparition d'une barre cachée par un plein écran : hauteur de la bande du haut de\n\
             # l'écran (pixels) et temps d'arrêt de la souris dedans (millisecondes).\n\
             reveal_px = {}\n\
             reveal_ms = {}\n\
             \n\
             [palette]\n\
             # Raccourci global de la palette de commandes (« Ctrl+Alt+P », « Win+Shift+Space » ;\n\
             # vide = aucun).\n\
             hotkey = {}\n\
             \n\
             [timer]\n\
             # Durées proposées par le minuteur (minutes, 6 au plus).\n\
             presets = [{}]\n\
             # Pomodoro : minutes de travail et de pause.\n\
             focus = {}\n\
             rest = {}\n\
             \n\
             [alerts]\n\
             # « Claude attend » : dans l'îlot de l'écran principal, et signal lumineux par Prisme.\n",
            quoted(&n.start),
            quoted(&n.end),
            (n.opacity * 100.0).round() / 100.0,
            n.dnd,
            a.reveal_px,
            a.reveal_ms,
            quoted(&self.palette.hotkey),
            t.presets.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
            t.focus,
            t.rest,
        );
        line(&mut o, "claude_island", al.claude_island, "true");
        line(&mut o, "claude_light", al.claude_light, "true");
        o.push_str("# Pendant « Ne pas déranger », l'îlot montre quand même « Claude attend » et les alertes\n# d'état (batterie faible, serveur hors ligne).\n");
        line(&mut o, "dnd_claude", al.dnd_claude, "true");
        line(&mut o, "dnd_battery", al.dnd_battery, "true");
        o.push_str("# « Ne pas déranger » passe Discord en « Ne pas déranger » et tient la scène « focus » de Prisme.\n");
        line(&mut o, "dnd_discord", al.dnd_discord, "true");
        line(&mut o, "dnd_light", al.dnd_light, "false");
        o.push_str("# Batterie faible du casque : seuil en % (0 = pas d'alerte).\n");
        line(&mut o, "headset_low", al.headset_low, "15");
        let f = &self.features;
        let _ = write!(
            o,
            "
             [features]
             # Nouvelle capture d'écran : l'îlot propose de coller son chemin dans la session Claude qui
             # attend (jamais Entrée).
             capture_to_claude = {}
             # Casque éteint : la sortie audio passe sur fallback_output (identifiant de la sortie, choisi
             # dans « Réglages d'Horizon › Fonctions »), puis revient quand il se rallume.
             headset_switch = {}
             fallback_output = {}
             # Appli plein écran sur l'écran principal : spectre coupé, sondages espacés.
             game_mode = {}
             # Couleur d'accent tirée du fond d'écran (barres, ambiance de Prisme), et aussi celle
             # de Windows (windows_accent ; celle d'origine revient quand c'est coupé).
             wallpaper_accent = {}
             windows_accent = {}
             # Volume et média affichés dans la barre principale à la place de l'affichage de
             # Windows ; osd_track : titre de chaque nouveau morceau.
             osd = {}
             osd_track = {}
             # Pendant la lecture : îlots éclairés par la basse, flash à chaque grosse caisse.
             bar_pulse = {}
             # Versions publiées : nouvelle version cherchée sur GitHub, installée au démarrage suivant.
             auto_update = {}

             [discord]
             # Widget Discord vocal : ta propre application Discord (portail des développeurs, avec
             # http://localhost dans « RPC Origins »). Vide : StreamKit, l'application de Discord pour
             # les overlays, sans rien à régler (certaines commandes peuvent alors être refusées).
             client_id = {}
             client_secret = {}
",
            f.capture_to_claude,
            f.headset_switch,
            quoted(&f.fallback_output),
            f.game_mode,
            f.wallpaper_accent,
            f.windows_accent,
            f.osd,
            f.osd_track,
            f.bar_pulse,
            f.auto_update,
            quoted(&self.discord.client_id),
            quoted(&self.discord.client_secret),
        );
        o
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AlertSettings, Settings, SettingsFile};
    use crate::testdir::TestDir;

    #[test]
    fn written_text_reads_back_the_same() {
        let mut s = Settings::default();
        s.night.start = "23:30".into();
        s.night.opacity = 0.4;
        s.night.dnd = true;
        s.palette.hotkey = "Win+Shift+\"P\"".into();
        s.timer.presets = vec![10, 20];
        s.alerts = AlertSettings { dnd_discord: Some(false), headset_low: Some(25), ..AlertSettings::default() };
        s.features.headset_switch = true;
        s.features.fallback_output = r#"{0.0.0.00000000}.{a\"b}"#.into();
        s.features.game_mode = false;
        let text = s.to_toml();
        assert!(text.contains("# claude_island = true"), "valeur non réglée en commentaire");
        assert_eq!(Settings::parse(&text), Some(s));
        assert_eq!(Settings::parse(&Settings::default().to_toml()), Some(Settings::default()));
    }

    #[test]
    fn save_never_overwrites_an_invalid_file() {
        let d = TestDir::new("settings-save");
        let p = d.join("settings.toml");
        let mine = "[night]\nstart = = \"23:00\"\n";
        std::fs::write(&p, mine).unwrap();
        assert!(matches!(Settings::inspect(&p).unwrap(), SettingsFile::Invalid(_)));
        assert!(Settings::default().save(&p).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), mine);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(Settings::inspect(&p).unwrap(), SettingsFile::Missing);
        let mut s = Settings::default();
        s.autohide.reveal_px = 30;
        s.save(&p).unwrap();
        assert_eq!(Settings::inspect(&p).unwrap(), SettingsFile::Valid(s));
    }

    #[test]
    fn timer_values_are_bounded() {
        let s = Settings::parse("[timer]\npresets = [0, 5, 5, 9000, 1, 2, 3, 4, 6]\nfocus = 0\n").unwrap();
        assert_eq!(s.timer.presets, vec![1, 5, 600, 1, 2, 3]);
        assert_eq!(s.timer.focus, 1);
        assert_eq!(Settings::parse("[timer]\npresets = []\n").unwrap().timer.presets, vec![5, 15, 25, 50]);
    }
}
