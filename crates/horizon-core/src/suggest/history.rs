//! Lecture de l'historique et des icônes de Vivaldi (bases SQLite au schéma Chromium), toujours
//! sur une copie : la base de Vivaldi n'est jamais ouverte, donc jamais verrouillée ni modifiée.

use super::keys::{chrome_time_to_unix, is_real_navigation, site_host};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    pub id: i64,
    pub host: String,
    pub utc: i64,
}

/// Visites retenues et dernier identifiant parcouru (retenu ou non) : le curseur avance même
/// quand tout un lot est filtré.
#[derive(Debug, Default, PartialEq)]
pub struct Batch {
    pub visits: Vec<Visit>,
    pub last_id: Option<i64>,
}

/// Copie temporaire d'une base et de ses journaux, supprimée à la fin.
struct Snapshot {
    files: Vec<PathBuf>,
    db: PathBuf,
}

impl Snapshot {
    fn take(src: &Path, scratch: &Path) -> anyhow::Result<Snapshot> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(scratch)?;
        let name = format!("copy-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        let db = scratch.join(&name);
        let mut snap = Snapshot { files: vec![db.clone()], db };
        std::fs::copy(src, &snap.db)?;
        for suffix in ["-wal", "-journal"] {
            let j = PathBuf::from(format!("{}{suffix}", src.display()));
            if j.exists() {
                let dst = PathBuf::from(format!("{}{suffix}", snap.db.display()));
                snap.files.push(dst.clone());
                std::fs::copy(&j, &dst)?;
            }
        }
        snap.files.push(PathBuf::from(format!("{}-shm", snap.db.display())));
        Ok(snap)
    }

    fn open(&self) -> anyhow::Result<Connection> {
        // Lecture-écriture sur la copie : SQLite doit pouvoir y rejouer un journal en attente.
        Ok(Connection::open_with_flags(
            &self.db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for f in &self.files {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// Visites d'identifiant supérieur à `after_id` : vraies navigations vers des sites publics,
/// faites sur ce PC. Les visites synchronisées depuis un autre appareil (téléphone) portent
/// l'identifiant de cet appareil dans `originator_cache_guid` et sont ignorées.
pub fn read_visits(history: &Path, after_id: i64, scratch: &Path) -> anyhow::Result<Batch> {
    let snap = Snapshot::take(history, scratch)?;
    let db = snap.open()?;
    let has_origin = db
        .prepare("SELECT 1 FROM pragma_table_info('visits') WHERE name = 'originator_cache_guid'")?
        .exists([])?;
    let origin = if has_origin { "coalesce(v.originator_cache_guid, '')" } else { "''" };
    let mut q = db.prepare(&format!(
        "SELECT v.id, u.url, v.visit_time, v.transition, {origin} FROM visits v
         JOIN urls u ON u.id = v.url WHERE v.id > ?1 ORDER BY v.id"
    ))?;
    let rows = q.query_map([after_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut batch = Batch::default();
    for row in rows {
        let (id, url, time, transition, origin) = row?;
        batch.last_id = batch.last_id.max(Some(id));
        if !origin.is_empty() || !is_real_navigation(transition) {
            continue;
        }
        if let Some(host) = site_host(&url) {
            batch.visits.push(Visit { id, host, utc: chrome_time_to_unix(time) });
        }
    }
    Ok(batch)
}

/// Icône (PNG) la plus grande, jusqu'à 64 px, d'une page de cet hôte.
pub fn read_favicon(favicons: &Path, host: &str, scratch: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let snap = Snapshot::take(favicons, scratch)?;
    let db = snap.open()?;
    let patterns = [format!("%://{host}/%"), format!("%://www.{host}/%")];
    let mut q = db.prepare(
        "SELECT b.image_data, b.width FROM icon_mapping m
         JOIN favicon_bitmaps b ON b.icon_id = m.icon_id
         WHERE m.page_url LIKE ?1 OR m.page_url LIKE ?2
         ORDER BY (b.width <= 64) DESC, CASE WHEN b.width <= 64 THEN -b.width ELSE b.width END
         LIMIT 1",
    )?;
    let mut rows = q.query(rusqlite::params![patterns[0], patterns[1]])?;
    Ok(match rows.next()? {
        Some(r) => Some(r.get::<_, Vec<u8>>(0)?),
        None => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> crate::testdir::TestDir {
        crate::testdir::TestDir::new(&format!("hist-{name}"))
    }

    /// Microsecondes Chromium d'un instant Unix.
    fn chrome(unix: i64) -> i64 {
        (unix + 11_644_473_600) * 1_000_000
    }

    fn history(path: &Path, wal: bool) -> Connection {
        let db = Connection::open(path).unwrap();
        if wal {
            db.pragma_update(None, "journal_mode", "WAL").unwrap();
        }
        db.execute_batch(
            "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT);
             CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER, transition INTEGER);
             INSERT INTO urls VALUES (1, 'https://www.youtube.com/watch?v=1'), (2, 'vivaldi://startpage'),
                                     (3, 'https://ads.example.com/frame');",
        )
        .unwrap();
        let rows = [(1, 1, 100, 1), (2, 2, 200, 1), (3, 3, 300, 3), (4, 1, 400, 0x2000_0000 | 0x4000_0000), (5, 1, 500, 8)];
        for (id, url, t, tr) in rows {
            db.execute("INSERT INTO visits VALUES (?1, ?2, ?3, ?4)", [id, url, chrome(t), tr]).unwrap();
        }
        db
    }

    #[test]
    fn reads_only_new_real_visits() {
        let d = dir("new");
        let p = d.join("History");
        drop(history(&p, false));
        let b = read_visits(&p, 0, &d.join("scratch")).unwrap();
        assert_eq!(b.visits, vec![
            Visit { id: 1, host: "youtube.com".into(), utc: 100 },
            Visit { id: 4, host: "youtube.com".into(), utc: 400 },
        ]);
        assert_eq!(b.last_id, Some(5));
        let b = read_visits(&p, 4, &d.join("scratch")).unwrap();
        assert!(b.visits.is_empty());
        assert_eq!(b.last_id, Some(5));
    }

    #[test]
    fn visits_synced_from_other_devices_are_ignored() {
        let d = dir("sync");
        let p = d.join("History");
        let db = history(&p, false);
        db.execute_batch(
            "ALTER TABLE visits ADD COLUMN originator_cache_guid TEXT NOT NULL DEFAULT '';
             UPDATE visits SET originator_cache_guid = 'telephone' WHERE id = 4;",
        )
        .unwrap();
        drop(db);
        let b = read_visits(&p, 0, &d.join("scratch")).unwrap();
        assert_eq!(b.visits.iter().map(|v| v.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(b.last_id, Some(5));
    }

    #[test]
    fn reads_db_in_wal_mode_while_open() {
        let d = dir("wal");
        let p = d.join("History");
        let live = history(&p, true); // connexion tenue ouverte, comme Vivaldi
        live.execute("INSERT INTO visits VALUES (6, 1, ?1, 1)", [chrome(600)]).unwrap();
        let b = read_visits(&p, 5, &d.join("scratch")).unwrap();
        assert_eq!(b.visits.len(), 1, "la visite encore dans le journal WAL est lue");
        drop(live);
    }

    #[test]
    fn scratch_copy_is_removed() {
        let d = dir("scratch");
        let p = d.join("History");
        let _live = history(&p, true);
        read_visits(&p, 0, &d.join("scratch")).unwrap();
        assert_eq!(std::fs::read_dir(d.join("scratch")).unwrap().count(), 0);
    }

    #[test]
    fn favicon_picks_largest_up_to_64() {
        let d = dir("fav");
        let p = d.join("Favicons");
        let db = Connection::open(&p).unwrap();
        db.execute_batch(
            "CREATE TABLE icon_mapping (page_url TEXT, icon_id INTEGER);
             CREATE TABLE favicon_bitmaps (icon_id INTEGER, image_data BLOB, width INTEGER);
             INSERT INTO icon_mapping VALUES ('https://www.youtube.com/', 1), ('https://other.com/', 2);
             INSERT INTO favicon_bitmaps VALUES (1, x'10', 16), (1, x'20', 32), (1, x'30', 144), (2, x'40', 32);",
        )
        .unwrap();
        drop(db);
        let s = d.join("scratch");
        assert_eq!(read_favicon(&p, "youtube.com", &s).unwrap(), Some(vec![0x20]));
        assert_eq!(read_favicon(&p, "missing.com", &s).unwrap(), None);
    }
}
