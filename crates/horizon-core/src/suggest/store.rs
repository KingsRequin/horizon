//! Base locale `usage.db` : intervalles de premier plan, visites importées de Vivaldi,
//! exclusions et caches (noms, cibles de lancement). Ne contient jamais d'URL complète.

use crate::apps::app_key;
use super::score::Usage;
use super::ItemKey;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::path::Path;

/// Clé d'appli de Vivaldi : ses intervalles donnent l'écran des visites.
pub const VIVALDI_APP: &str = r"vivaldi\vivaldi.exe";
/// Intervalle de premier plan compté à partir de cette durée.
pub(crate) const MIN_INTERVAL_SECS: i64 = 10;

pub struct Store {
    db: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS intervals (
    app TEXT NOT NULL, app_key TEXT NOT NULL, monitor TEXT NOT NULL,
    start_utc INTEGER NOT NULL, end_utc INTEGER NOT NULL, start_local INTEGER NOT NULL,
    vivaldi INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS intervals_time ON intervals(start_utc);
CREATE TABLE IF NOT EXISTS visits (
    profile TEXT NOT NULL, visit_id INTEGER NOT NULL, host TEXT NOT NULL,
    utc INTEGER NOT NULL, local INTEGER NOT NULL, PRIMARY KEY (profile, visit_id));
CREATE INDEX IF NOT EXISTS visits_time ON visits(utc);
CREATE TABLE IF NOT EXISTS cursors (profile TEXT PRIMARY KEY, last_id INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS site_profile (host TEXT PRIMARY KEY, profile TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS blocked (item TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS labels (item TEXT PRIMARY KEY, label TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS launch (app_key TEXT PRIMARY KEY, target TEXT NOT NULL);
";

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.execute_batch(SCHEMA)?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            // Version 1 : les visites synchronisées depuis le téléphone ne sont plus importées.
            // Celles importées avant sont retirées en repartant de zéro (l'import suivant relit
            // tout l'historique avec le filtre).
            db.execute_batch("DELETE FROM visits; DELETE FROM cursors; DELETE FROM site_profile;")?;
            db.pragma_update(None, "user_version", 1)?;
        }
        Ok(Store { db })
    }

    /// Intervalle de premier plan ; ignoré s'il dure moins de 10 s.
    pub fn add_interval(
        &self,
        app: &str,
        monitor: &str,
        start_utc: i64,
        end_utc: i64,
        start_local: i64,
    ) -> anyhow::Result<()> {
        if end_utc - start_utc < MIN_INTERVAL_SECS {
            return Ok(());
        }
        let key = app_key(app);
        self.db.execute(
            "INSERT INTO intervals VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![app, key, monitor, start_utc, end_utc, start_local, key == VIVALDI_APP],
        )?;
        Ok(())
    }

    /// Visites `(id Vivaldi, hôte, utc, heure locale)` d'un profil ; avance son curseur.
    pub fn add_visits(&mut self, profile: &str, visits: &[(i64, String, i64, i64)]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        let mut last = None;
        for (id, host, utc, local) in visits {
            tx.execute(
                "INSERT OR IGNORE INTO visits VALUES (?1, ?2, ?3, ?4, ?5)",
                params![profile, id, host, utc, local],
            )?;
            tx.execute(
                "INSERT INTO site_profile VALUES (?1, ?2) ON CONFLICT(host) DO UPDATE SET profile = ?2",
                params![host, profile],
            )?;
            last = last.max(Some(*id));
        }
        if let Some(id) = last {
            tx.execute(
                "INSERT INTO cursors VALUES (?1, ?2)
                 ON CONFLICT(profile) DO UPDATE SET last_id = max(last_id, ?2)",
                params![profile, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Dernier identifiant de visite importé pour ce profil (0 sinon).
    pub fn cursor(&self, profile: &str) -> anyhow::Result<i64> {
        let v = self
            .db
            .query_row("SELECT last_id FROM cursors WHERE profile = ?1", [profile], |r| r.get(0))
            .optional()?;
        Ok(v.unwrap_or(0))
    }

    /// Avance le curseur sans visite (lot entièrement filtré).
    pub fn set_cursor(&self, profile: &str, last_id: i64) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO cursors VALUES (?1, ?2) ON CONFLICT(profile) DO UPDATE SET last_id = max(last_id, ?2)",
            params![profile, last_id],
        )?;
        Ok(())
    }

    /// Usages depuis `since_utc`. Une visite prend l'écran de l'intervalle Vivaldi qui la
    /// contient, sinon elle est « sans écran ».
    pub fn usages(&self, since_utc: i64) -> anyhow::Result<Vec<Usage>> {
        let mut out = Vec::new();
        let mut q = self
            .db
            .prepare("SELECT app_key, start_local, monitor FROM intervals WHERE start_utc >= ?1")?;
        let apps = q.query_map([since_utc], |r| {
            Ok(Usage { item: ItemKey::App(r.get(0)?), local_ts: r.get(1)?, monitor: Some(r.get(2)?) })
        })?;
        for u in apps {
            out.push(u?);
        }
        let mut q = self.db.prepare(
            "SELECT v.host, v.local,
                    (SELECT i.monitor FROM intervals i
                     WHERE i.vivaldi = 1 AND i.start_utc <= v.utc AND v.utc < i.end_utc
                     ORDER BY i.start_utc DESC LIMIT 1)
             FROM visits v WHERE v.utc >= ?1",
        )?;
        let sites = q.query_map([since_utc], |r| {
            Ok(Usage { item: ItemKey::Site(r.get(0)?), local_ts: r.get(1)?, monitor: r.get(2)? })
        })?;
        for u in sites {
            out.push(u?);
        }
        Ok(out)
    }

    /// Profil Vivaldi où l'hôte a été vu en dernier.
    pub fn profile_of(&self, host: &str) -> Option<String> {
        self.db
            .query_row("SELECT profile FROM site_profile WHERE host = ?1", [host], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    /// Chemin (ou AUMID) le plus récent vu pour cette clé d'appli.
    pub fn last_raw_app(&self, key: &str) -> Option<String> {
        self.db
            .query_row(
                "SELECT app FROM intervals WHERE app_key = ?1 ORDER BY start_utc DESC LIMIT 1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Clés des applis les plus utilisées au premier plan depuis `since_utc`.
    pub fn top_apps(&self, since_utc: i64, limit: usize) -> anyhow::Result<Vec<String>> {
        let mut q = self.db.prepare(
            "SELECT app_key FROM intervals WHERE start_utc >= ?1
             GROUP BY app_key ORDER BY SUM(end_utc - start_utc) DESC LIMIT ?2",
        )?;
        let rows = q.query_map(params![since_utc, limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn purge(&self, older_than_utc: i64) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM intervals WHERE end_utc < ?1", [older_than_utc])?;
        self.db.execute("DELETE FROM visits WHERE utc < ?1", [older_than_utc])?;
        Ok(())
    }

    /// Un écran a changé d'identifiant : son historique le suit.
    pub fn remap_monitor(&self, old: &str, new: &str) -> anyhow::Result<()> {
        self.db.execute("UPDATE intervals SET monitor = ?2 WHERE monitor = ?1", [old, new])?;
        Ok(())
    }

    pub fn block(&self, item: &ItemKey) -> anyhow::Result<()> {
        self.db.execute("INSERT OR IGNORE INTO blocked VALUES (?1)", [item.as_str()])?;
        Ok(())
    }

    pub fn unblock(&self, item: &ItemKey) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM blocked WHERE item = ?1", [item.as_str()])?;
        Ok(())
    }

    pub fn blocked(&self) -> anyhow::Result<HashSet<ItemKey>> {
        let mut q = self.db.prepare("SELECT item FROM blocked")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok().and_then(|s| ItemKey::parse(&s))).collect())
    }

    pub fn set_label(&self, item: &ItemKey, label: &str) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO labels VALUES (?1, ?2) ON CONFLICT(item) DO UPDATE SET label = ?2",
            params![item.as_str(), label],
        )?;
        Ok(())
    }

    pub fn label(&self, item: &ItemKey) -> Option<String> {
        self.db
            .query_row("SELECT label FROM labels WHERE item = ?1", [item.as_str()], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_launch(&self, app_key: &str, target: &str) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO launch VALUES (?1, ?2) ON CONFLICT(app_key) DO UPDATE SET target = ?2",
            params![app_key, target],
        )?;
        Ok(())
    }

    pub fn launch_target(&self, app_key: &str) -> Option<String> {
        self.db
            .query_row("SELECT target FROM launch WHERE app_key = ?1", [app_key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const VIVALDI: &str = r"C:\Users\u\AppData\Local\Vivaldi\Application\vivaldi.exe";

    /// Base neuve ; le dossier est déclaré en premier pour être supprimé après la base.
    fn store(name: &str) -> (TestDir, Store) {
        let d = TestDir::new(&format!("usage-{name}"));
        let s = Store::open(&d.join("usage.db")).unwrap();
        (d, s)
    }

    fn site(h: &str) -> ItemKey {
        ItemKey::Site(h.into())
    }

    #[test]
    fn short_intervals_are_dropped() {
        let (_d, s) = store("short");
        s.add_interval("a.exe", "M", 100, 109, 100).unwrap();
        s.add_interval("a.exe", "M", 200, 210, 200).unwrap();
        let u = s.usages(0).unwrap();
        assert_eq!(u.len(), 1);
        assert_eq!(u[0], Usage { item: ItemKey::App("a.exe".into()), local_ts: 200, monitor: Some("M".into()) });
    }

    #[test]
    fn visit_gets_screen_of_enclosing_vivaldi_interval() {
        let (_d, mut s) = store("attr");
        s.add_interval(VIVALDI, "M", 1000, 2000, 4600).unwrap();
        s.add_interval(r"C:\x\discord\Discord.exe", "R", 2000, 3000, 5600).unwrap();
        s.add_visits("Default", &[(1, "youtube.com".into(), 1500, 5100)]).unwrap();
        let u = s.usages(0).unwrap();
        let v = u.iter().find(|u| u.item == site("youtube.com")).unwrap();
        assert_eq!(v.monitor.as_deref(), Some("M"));
        assert_eq!(v.local_ts, 5100);
    }

    #[test]
    fn visit_outside_vivaldi_is_unattributed() {
        let (_d, mut s) = store("unattr");
        s.add_interval(r"C:\x\discord\Discord.exe", "R", 1000, 2000, 1000).unwrap();
        s.add_visits("Default", &[(1, "youtube.com".into(), 1500, 1500), (2, "twitch.tv".into(), 9000, 9000)])
            .unwrap();
        let u = s.usages(0).unwrap();
        assert!(u.iter().filter(|u| matches!(u.item, ItemKey::Site(_))).all(|u| u.monitor.is_none()));
    }

    #[test]
    fn cursor_advances_per_profile() {
        let (_d, mut s) = store("cursor");
        assert_eq!(s.cursor("Default").unwrap(), 0);
        s.add_visits("Default", &[(5, "a.com".into(), 1, 1), (9, "b.com".into(), 2, 2)]).unwrap();
        s.add_visits("Profile 1", &[(3, "a.com".into(), 3, 3)]).unwrap();
        assert_eq!(s.cursor("Default").unwrap(), 9);
        assert_eq!(s.cursor("Profile 1").unwrap(), 3);
        s.set_cursor("Default", 4).unwrap(); // ne recule jamais
        assert_eq!(s.cursor("Default").unwrap(), 9);
        assert_eq!(s.profile_of("a.com").as_deref(), Some("Profile 1"));
        // Réimport du même identifiant : pas de doublon.
        s.add_visits("Default", &[(9, "b.com".into(), 2, 2)]).unwrap();
        assert_eq!(s.usages(0).unwrap().len(), 3);
    }

    #[test]
    fn purge_removes_old_rows() {
        let (_d, mut s) = store("purge");
        s.add_interval("a.exe", "M", 0, 100, 0).unwrap();
        s.add_interval("a.exe", "M", 5000, 5100, 5000).unwrap();
        s.add_visits("Default", &[(1, "a.com".into(), 10, 10), (2, "a.com".into(), 6000, 6000)]).unwrap();
        s.purge(1000).unwrap();
        assert_eq!(s.usages(0).unwrap().len(), 2);
    }

    #[test]
    fn remap_monitor_moves_history() {
        let (_d, s) = store("remap");
        s.add_interval("a.exe", "OLD", 0, 100, 0).unwrap();
        s.remap_monitor("OLD", "NEW").unwrap();
        assert_eq!(s.usages(0).unwrap()[0].monitor.as_deref(), Some("NEW"));
    }

    #[test]
    fn block_unblock_roundtrip() {
        let (_d, s) = store("block");
        s.block(&site("youtube.com")).unwrap();
        s.block(&site("youtube.com")).unwrap();
        assert_eq!(s.blocked().unwrap(), [site("youtube.com")].into());
        s.unblock(&site("youtube.com")).unwrap();
        assert!(s.blocked().unwrap().is_empty());
    }

    #[test]
    fn version_0_base_drops_imported_visits_once() {
        let (d, mut s) = store("v0");
        let p = d.join("usage.db");
        s.add_visits("Default", &[(7, "a.com".into(), 1, 1)]).unwrap();
        s.add_interval("a.exe", "M", 0, 100, 0).unwrap();
        s.db.pragma_update(None, "user_version", 0).unwrap();
        drop(s);
        let mut s = Store::open(&p).unwrap();
        assert_eq!(s.cursor("Default").unwrap(), 0);
        assert_eq!(s.usages(0).unwrap().len(), 1, "le journal des applis est gardé");
        s.add_visits("Default", &[(7, "a.com".into(), 1, 1)]).unwrap();
        drop(s);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.usages(0).unwrap().len(), 2, "pas de nouvelle purge en version 1");
    }

    #[test]
    fn reopen_keeps_data() {
        let (d, s) = store("reopen");
        let p = d.join("usage.db");
        s.add_interval(VIVALDI, "M", 0, 100, 0).unwrap();
        s.set_label(&site("a.com"), "A").unwrap();
        s.set_launch("x\\x.exe", "C:\\x.lnk").unwrap();
        drop(s);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.usages(0).unwrap()[0].item, ItemKey::App(VIVALDI_APP.into()));
        assert_eq!(s.label(&site("a.com")).as_deref(), Some("A"));
        assert_eq!(s.launch_target("x\\x.exe").as_deref(), Some("C:\\x.lnk"));
        assert_eq!(s.last_raw_app(VIVALDI_APP).as_deref(), Some(VIVALDI));
    }
}
