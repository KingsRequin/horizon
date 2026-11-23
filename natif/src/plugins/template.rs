//! « Nouveau plugin » (page Plugins des réglages) : un dossier modèle (`natif/plugin-template/`,
//! avec une copie de `docs/PLUGINS.md`) créé dans le dossier des plugins, posé sur la barre
//! principale et ouvert dans l'explorateur : il ne reste qu'à le modifier (ou à le donner à une
//! IA).

use crate::host::{AppHandle, Manager};
use horizon_core::layout::{BarRole, WidgetKind, Zone};
use horizon_core::plugin::kind_of;
use std::path::{Path, PathBuf};

/// Fichiers du modèle (`{{id}}` et `{{name}}` remplacés).
const FILES: [(&str, &str); 5] = [
    ("plugin.json", include_str!("../../plugin-template/plugin.json")),
    ("widget.slint", include_str!("../../plugin-template/widget.slint")),
    ("panel.slint", include_str!("../../plugin-template/panel.slint")),
    ("main.js", include_str!("../../plugin-template/main.js")),
    ("PLUGINS.md", include_str!("../../../docs/PLUGINS.md")),
];

/// Crée un plugin modèle dans `root` : `mon-plugin`, sinon `mon-plugin-2`… Renvoie son id et
/// son dossier.
pub fn create(root: &Path) -> anyhow::Result<(String, PathBuf)> {
    let (id, n) = (1..100)
        .map(|n| (if n == 1 { "mon-plugin".to_owned() } else { format!("mon-plugin-{n}") }, n))
        .find(|(id, _)| !root.join(id).exists())
        .ok_or_else(|| anyhow::anyhow!("trop de plugins « mon-plugin »"))?;
    let name = if n == 1 { "Mon plugin".to_owned() } else { format!("Mon plugin {n}") };
    let dir = root.join(&id);
    // Écrit dans un dossier à part puis renommé : Horizon ne voit jamais un plugin à moitié écrit.
    let tmp = root.join(format!(".{id}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    for (file, text) in FILES {
        std::fs::write(tmp.join(file), text.replace("{{id}}", &id).replace("{{name}}", &name))?;
    }
    std::fs::rename(&tmp, &dir)?;
    Ok((id, dir))
}

/// Action « Nouveau plugin » : créé, ajouté à droite de la barre principale, dossier ouvert.
pub fn new_plugin(app: &AppHandle) -> Result<(), String> {
    let (id, dir) = create(&super::plugins_dir()).map_err(|e| format!("{e:#}"))?;
    let state = app.state::<crate::state::AppState>();
    let kind = WidgetKind::new(&kind_of(&id));
    state
        .mutate(|l| {
            let monitor = l
                .bars
                .iter()
                .find(|b| b.role == BarRole::Main)
                .or_else(|| l.bars.first())
                .map(|b| b.monitor_id.clone())
                .ok_or_else(|| anyhow::anyhow!("aucune barre"))?;
            l.add_widget(&monitor, Zone::Right, kind).map(|_| ())
        })
        .map_err(|e| format!("{e:#}"))?;
    crate::edit::broadcast_layout(app);
    log::info!("plugin {id} créé dans {}", dir.display());
    horizon_sys::launch::launch(&dir.display().to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_a_valid_plugin() {
        let root = std::env::temp_dir().join(format!("horizon-modele-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (a, dir) = create(&root).unwrap();
        let (b, _) = create(&root).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("mon-plugin", "mon-plugin-2"));
        assert_eq!(super::super::check::check(&dir), Vec::<String>::new());
        assert!(dir.join("PLUGINS.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
