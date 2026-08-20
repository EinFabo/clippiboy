//! Clip-Index in SQLite. Die Videodateien selbst liegen im Clip-Ordner; die
//! Datenbank hält nur Metadaten, damit die Galerie ohne Dateisystem-Scan lädt.

use rusqlite::{params, Connection};

use crate::config;
use crate::model::{Clip, ClipEdit};

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
        )?;
        // Nachträglich dazugekommen: eine bestehende Datenbank soll ihre Clips
        // behalten, deshalb angehängt statt Tabelle neu.
        self.add_column("title", "TEXT")?;
        self.add_column("description", "TEXT")?;
        // Zuschnitt und Spurenmischung als JSON — ein eigenes Tabellenschema
        // dafür hätte nur Spalten, die sich mit dem Editor wieder ändern.
        self.add_column("edit", "TEXT")?;
        Ok(())
    }

    /// Spalte anlegen, falls sie fehlt. `ALTER TABLE … ADD COLUMN` kennt kein
    /// `IF NOT EXISTS`, deshalb der Blick in `pragma_table_info`.
    fn add_column(&self, name: &str, decl: &str) -> rusqlite::Result<()> {
        let exists: bool = self
            .conn
            .prepare("SELECT 1 FROM pragma_table_info('clips') WHERE name = ?1")?
            .exists(params![name])?;
        if !exists {
            self.conn
                .execute_batch(&format!("ALTER TABLE clips ADD COLUMN {name} {decl}"))?;
        }
        Ok(())
    }

    pub fn insert(&self, clip: &Clip) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO clips
             (id, path, created_at, duration_ms, game, width, height, size_bytes,
              thumb_path, title, description, edit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                clip.title,
                clip.description,
                encode_edit(clip.edit.as_ref()),
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> rusqlite::Result<Vec<Clip>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, created_at, duration_ms, game, width, height, size_bytes,
                    thumb_path, title, description, edit
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
                title: row.get(9)?,
                description: row.get(10)?,
                edit: decode_edit(row.get::<_, Option<String>>(11)?),
            })
        })?;
        rows.collect()
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Clip>> {
        Ok(self.list()?.into_iter().find(|c| c.id == id))
    }

    /// Name, Beschreibung und Spiel ändern. Leere Felder kommen als `None` an
    /// und löschen den Eintrag wieder.
    pub fn update_meta(
        &self,
        id: &str,
        title: Option<&str>,
        description: Option<&str>,
        game: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET title = ?2, description = ?3, game = ?4 WHERE id = ?1",
            params![id, title, description, game],
        )?;
        Ok(())
    }

    /// Zuschnitt und Spurenmischung ablegen. `None` löscht sie wieder — der
    /// Clip gilt dann als unangetastet.
    pub fn set_edit(&self, id: &str, edit: Option<&ClipEdit>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET edit = ?2 WHERE id = ?1",
            params![id, encode_edit(edit)],
        )?;
        Ok(())
    }

    /// Die Dateigröße nachtragen. Nötig, sobald die Datei neu geschrieben
    /// wurde — sonst stünde in der Galerie noch die Größe von vorher.
    pub fn set_size(&self, id: &str, size_bytes: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET size_bytes = ?2 WHERE id = ?1",
            params![id, size_bytes],
        )?;
        Ok(())
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

/// Der Editor-Stand als JSON. Scheitert das Serialisieren, ist ein fehlender
/// Zuschnitt besser als ein Clip, der sich nicht mehr speichern lässt.
fn encode_edit(edit: Option<&ClipEdit>) -> Option<String> {
    edit.and_then(|value| serde_json::to_string(value).ok())
}

/// Unlesbares JSON (etwa aus einer älteren Version) wird verworfen statt die
/// ganze Galerie scheitern zu lassen.
fn decode_edit(raw: Option<String>) -> Option<ClipEdit> {
    let raw = raw?;
    match serde_json::from_str(&raw) {
        Ok(edit) => Some(edit),
        Err(err) => {
            log::warn!("Editor-Stand unlesbar ({err}) — wird verworfen");
            None
        }
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
            title: None,
            description: None,
            edit: None,
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

    #[test]
    fn metadata_survives_the_roundtrip() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip("a", 1)).unwrap();
        lib.update_meta("a", Some("Ace"), Some("4k mit Deagle"), Some("CS2"))
            .unwrap();

        let stored = lib.get("a").unwrap().unwrap();
        assert_eq!(stored.title.as_deref(), Some("Ace"));
        assert_eq!(stored.description.as_deref(), Some("4k mit Deagle"));

        // Leeren Namen wieder loswerden.
        lib.update_meta("a", None, None, None).unwrap();
        assert!(lib.get("a").unwrap().unwrap().title.is_none());
    }

    #[test]
    fn edit_survives_the_roundtrip() {
        use crate::model::TrackMix;

        let lib = Library::in_memory().unwrap();
        lib.insert(&clip("a", 1)).unwrap();
        assert!(lib.get("a").unwrap().unwrap().edit.is_none());

        let edit = ClipEdit {
            start_ms: 2_500,
            end_ms: 9_000,
            tracks: vec![TrackMix {
                index: 1,
                gain_db: -6.0,
                muted: true,
            }],
        };
        lib.set_edit("a", Some(&edit)).unwrap();
        assert_eq!(lib.get("a").unwrap().unwrap().edit, Some(edit));

        // Zurücksetzen macht den Clip wieder unangetastet.
        lib.set_edit("a", None).unwrap();
        assert!(lib.get("a").unwrap().unwrap().edit.is_none());
    }
}
