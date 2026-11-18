//! Écrans : une barre par écran branché, retrouvée par identifiant ou par position.

use super::{BarLayout, BarRole, Layout, role_of};
use crate::geometry::MonitorInfo;

impl Layout {
    /// Barre d'un écran : par identifiant, sinon par position (l'identifiant est alors mis à
    /// jour), sinon une nouvelle barre par défaut. Renvoie aussi l'ancien identifiant quand la
    /// barre a été retrouvée par sa position sous un autre identifiant (écran rebranché ailleurs).
    pub(crate) fn bar_for_tracked(&mut self, m: &MonitorInfo, all: &[MonitorInfo]) -> (&mut BarLayout, Option<String>) {
        let pos = (m.rect.x, m.rect.y);
        let by_id = self.bars.iter().position(|b| b.monitor_id == m.id);
        // Par position, seulement une barre dont l'écran n'est plus branché.
        let by_pos = || {
            self.bars
                .iter()
                .position(|b| b.monitor_pos == pos && !all.iter().any(|o| o.id == b.monitor_id))
        };
        let index = match by_id.or_else(by_pos) {
            Some(i) => i,
            None => {
                self.bars.push(BarLayout::default_for(m, role_of(m, all)));
                self.bars.len() - 1
            }
        };
        let bar = &mut self.bars[index];
        let old = (bar.monitor_id != m.id).then(|| std::mem::replace(&mut bar.monitor_id, m.id.clone()));
        bar.monitor_pos = pos;
        (bar, old)
    }

    /// Garantit une barre à chaque écran branché (identifiants et positions mis à jour).
    /// Renvoie les écrans retrouvés sous un autre identifiant (ancien, nouveau).
    pub fn ensure_monitors(&mut self, monitors: &[MonitorInfo]) -> Vec<(String, String)> {
        monitors
            .iter()
            .filter_map(|m| self.bar_for_tracked(m, monitors).1.map(|old| (old, m.id.clone())))
            .collect()
    }

    /// Écrans où envoyer un widget (« Déplacer vers l'écran ») : ceux qui sont branchés,
    /// sauf le sien. Les barres des écrans débranchés restent dans la disposition, sans
    /// fenêtre pour les afficher.
    pub fn move_targets(&self, widget_id: &str, connected: &[&str]) -> Vec<(String, BarRole)> {
        let owner = self.find(widget_id).map(|(b, _, _)| self.bars[b].monitor_id.as_str());
        self.bars
            .iter()
            .filter(|b| Some(b.monitor_id.as_str()) != owner && connected.contains(&b.monitor_id.as_str()))
            .map(|b| (b.monitor_id.clone(), b.role))
            .collect()
    }
}
