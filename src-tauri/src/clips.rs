//! Clip-Index in SQLite. Die Videodateien selbst liegen im Clip-Ordner; die
//! Datenbank hält nur Metadaten, damit die Galerie ohne Dateisystem-Scan lädt.

use rusqlite::{params, Connection};

use crate::config;
use crate::model::Clip;

pub struct Library {
    conn: Connection,
}

impl Library {
    pub fn open() -> rusqlite::Result<Self> {
        let dir = config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let conn = Connection::open(dir.join("clips.db"))?;
        let library = Self { conn };
        library.migrate()?;
        Ok(library)
    }

    #[cfg(test)]
    pub fn in_memory() -> rusqlite::Result<Self> {
        let library = Self {
            conn: Connection::open_in_memory()?,
        };
        library.migrate()?;
        Ok(library)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS clips (
                 id          TEXT PRIMARY KEY,
                 path        TEXT NOT NULL UNIQUE,
                 created_at  INTEGER NOT NULL,
                 duration_ms INTEGER NOT NULL,
                 game        TEXT,
                 width       INTEGER NOT NULL,
                 height      INTEGER NOT NULL,
                 size_bytes  INTEGER NOT NULL,
                 thumb_path  TEXT
             );
             CREATE INDEX IF NOT EXISTS clips_created_at ON clips (created_at DESC);
             CREATE TABLE IF NOT EXISTS tags (
                 clip_id TEXT NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
                 tag     TEXT NOT NULL,
                 PRIMARY KEY (clip_id, tag)
             );",
        )
    }

    pub fn insert(&self, clip: &Clip) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO clips
             (id, path, created_at, duration_ms, game, width, height, size_bytes, thumb_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                clip.id,
                clip.path,
                clip.created_at,
                clip.duration_ms as i64,
                clip.game,
                clip.width,
                clip.height,
                clip.size_bytes as i64,
                clip.thumb_path,
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> rusqlite::Result<Vec<Clip>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, created_at, duration_ms, game, width, height, size_bytes, thumb_path
             FROM clips ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Clip {
                id: row.get(0)?,
                path: row.get(1)?,
                created_at: row.get(2)?,
                duration_ms: row.get::<_, i64>(3)? as u64,
                game: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                size_bytes: row.get::<_, i64>(7)? as u64,
                thumb_path: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Clip>> {
        Ok(self.list()?.into_iter().find(|c| c.id == id))
    }

    /// Entfernt den Eintrag und die Videodatei.
    pub fn delete(&self, id: &str) -> rusqlite::Result<()> {
        if let Some(clip) = self.get(id)? {
            let _ = std::fs::remove_file(&clip.path);
            if let Some(thumb) = clip.thumb_path {
                let _ = std::fs::remove_file(thumb);
            }
        }
        self.conn
            .execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: &str, created_at: i64) -> Clip {
        Clip {
            id: id.into(),
            path: format!("C:/clips/{id}.mp4"),
            created_at,
            duration_ms: 30_000,
            game: Some("CS2".into()),
            width: 1920,
            height: 1080,
            size_bytes: 1234,
            thumb_path: None,
        }
    }

    #[test]
    fn newest_clip_comes_first() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip("alt", 1000)).unwrap();
        lib.insert(&clip("neu", 2000)).unwrap();

        let clips = lib.list().unwrap();
        assert_eq!(clips[0].id, "neu");
        assert_eq!(clips.len(), 2);
    }

    #[test]
    fn delete_removes_the_entry() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip("a", 1)).unwrap();
        lib.delete("a").unwrap();
        assert!(lib.list().unwrap().is_empty());
    }
}
