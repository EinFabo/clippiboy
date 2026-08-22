//! Clip index in SQLite. The video files themselves live in the clip folder;
//! the database only holds metadata so the gallery loads without scanning the
//! file system.

use rusqlite::{params, Connection};

use crate::config;
use crate::model::{Clip, ClipEdit, ClipOriginal};

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
        // Added later on: an existing database should keep its clips, hence
        // appended instead of a fresh table.
        self.add_column("title", "TEXT")?;
        self.add_column("description", "TEXT")?;
        // Trim and track mix as JSON — a schema of its own for that would only
        // have columns that change again with the editor.
        self.add_column("edit", "TEXT")?;
        // Where the delivered excerpt sits inside the original, as long as the
        // untouched recording is still in the originals store.
        self.add_column("original", "TEXT")?;
        // The heart in the gallery.
        self.add_column("favorite", "INTEGER NOT NULL DEFAULT 0")?;
        // A still instead of a recording. Everything already in the database is
        // a clip, so the default answers the question for them.
        self.add_column("screenshot", "INTEGER NOT NULL DEFAULT 0")?;
        Ok(())
    }

    /// Add a column if it is missing. `ALTER TABLE … ADD COLUMN` has no
    /// `IF NOT EXISTS`, hence the look into `pragma_table_info`.
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
              thumb_path, title, description, edit, original, favorite, screenshot)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                encode_original(clip.original.as_ref()),
                clip.favorite,
                clip.screenshot,
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> rusqlite::Result<Vec<Clip>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, created_at, duration_ms, game, width, height, size_bytes,
                    thumb_path, title, description, edit, original, favorite, screenshot
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
                original: decode_original(row.get::<_, Option<String>>(12)?),
                favorite: row.get(13)?,
                screenshot: row.get(14)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Clip>> {
        Ok(self.list()?.into_iter().find(|c| c.id == id))
    }

    /// Change name, description and game. Empty fields arrive as `None` and
    /// clear the entry again.
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

    /// Set or take away the heart.
    pub fn set_favorite(&self, id: &str, favorite: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET favorite = ?2 WHERE id = ?1",
            params![id, favorite],
        )?;
        Ok(())
    }

    /// Record the file path — after a move into a different folder.
    pub fn set_path(&self, id: &str, path: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET path = ?2 WHERE id = ?1",
            params![id, path],
        )?;
        Ok(())
    }

    /// Record the thumbnail. `None` means there is none any more — the gallery
    /// then shows its gradient instead of a dead path.
    pub fn set_thumb(&self, id: &str, thumb_path: Option<&str>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET thumb_path = ?2 WHERE id = ?1",
            params![id, thumb_path],
        )?;
        Ok(())
    }

    /// Store trim and track mix. `None` clears them again — the clip then
    /// counts as untouched.
    pub fn set_edit(&self, id: &str, edit: Option<&ClipEdit>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET edit = ?2 WHERE id = ?1",
            params![id, encode_edit(edit)],
        )?;
        Ok(())
    }

    /// Record length and size. Needed as soon as the file has been rewritten —
    /// otherwise the gallery would still show the previous values, and after a
    /// trim the displayed duration would simply be wrong.
    pub fn set_file_state(
        &self,
        id: &str,
        duration_ms: u64,
        size_bytes: u64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET duration_ms = ?2, size_bytes = ?3 WHERE id = ?1",
            params![id, duration_ms as i64, size_bytes as i64],
        )?;
        Ok(())
    }

    /// Record edges and size. The counterpart to `set_file_state` for a still:
    /// it has no length, but cropping changes exactly what a recording never
    /// does — how big the picture is.
    pub fn set_picture(
        &self,
        id: &str,
        width: u32,
        height: u32,
        size_bytes: u64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET width = ?2, height = ?3, size_bytes = ?4 WHERE id = ?1",
            params![id, width, height, size_bytes as i64],
        )?;
        Ok(())
    }

    /// Note the originals store. `None` means there is no original any more;
    /// the clip is (again, or still) the whole recording.
    pub fn set_original(&self, id: &str, original: Option<&ClipOriginal>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clips SET original = ?2 WHERE id = ?1",
            params![id, encode_original(original)],
        )?;
        Ok(())
    }

    /// Removes the record and the video file.
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

/// The editor state as JSON. If serializing fails, a missing trim is better
/// than a clip that can no longer be saved.
fn encode_edit(edit: Option<&ClipEdit>) -> Option<String> {
    edit.and_then(|value| serde_json::to_string(value).ok())
}

/// Unreadable JSON (from an older version, say) is discarded rather than
/// letting the whole gallery fail.
fn decode_edit(raw: Option<String>) -> Option<ClipEdit> {
    let raw = raw?;
    match serde_json::from_str(&raw) {
        Ok(edit) => Some(edit),
        Err(err) => {
            log::warn!("editor state unreadable ({err}) — discarding it");
            None
        }
    }
}

/// The originals store as JSON.
fn encode_original(original: Option<&ClipOriginal>) -> Option<String> {
    original.and_then(|value| serde_json::to_string(value).ok())
}

/// Unreadable JSON is discarded. The clip then counts as untrimmed — the file
/// alongside it is found again by [`crate::edit::repair`] on the next start.
fn decode_original(raw: Option<String>) -> Option<ClipOriginal> {
    let raw = raw?;
    match serde_json::from_str(&raw) {
        Ok(original) => Some(original),
        Err(err) => {
            log::warn!("originals store unreadable ({err}) — discarding it");
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
            original: None,
            favorite: false,
            screenshot: false,
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
        lib.update_meta("a", Some("Ace"), Some("4k with the Deagle"), Some("CS2"))
            .unwrap();

        let stored = lib.get("a").unwrap().unwrap();
        assert_eq!(stored.title.as_deref(), Some("Ace"));
        assert_eq!(stored.description.as_deref(), Some("4k with the Deagle"));

        // Get rid of an empty name again.
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

        // Resetting makes the clip untouched again.
        lib.set_edit("a", None).unwrap();
        assert!(lib.get("a").unwrap().unwrap().edit.is_none());
    }

    /// The record carries the offset of the individual tracks. If it were lost
    /// on restart, the preview of a trimmed clip would run out of sync and
    /// "Undo trim" would no longer be findable.
    #[test]
    fn the_original_survives_the_roundtrip() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip("a", 1)).unwrap();
        assert!(lib.get("a").unwrap().unwrap().original.is_none());

        let original = ClipOriginal {
            duration_ms: 60_000,
            start_ms: 5_000,
            end_ms: 20_000,
        };
        lib.set_original("a", Some(&original)).unwrap();
        assert_eq!(lib.get("a").unwrap().unwrap().original, Some(original));

        lib.set_original("a", None).unwrap();
        assert!(lib.get("a").unwrap().unwrap().original.is_none());
    }

    /// Otherwise the gallery still shows the old length after a trim.
    #[test]
    fn length_and_size_move_together() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip("a", 1)).unwrap();
        lib.set_file_state("a", 12_000, 4_711).unwrap();

        let stored = lib.get("a").unwrap().unwrap();
        assert_eq!(stored.duration_ms, 12_000);
        assert_eq!(stored.size_bytes, 4_711);
    }

    /// Unreadable JSON must not take the whole gallery down with it.
    #[test]
    fn a_broken_entry_is_dropped_not_fatal() {
        assert!(decode_original(Some("{kaputt".into())).is_none());
        assert!(decode_edit(Some("{kaputt".into())).is_none());
    }
}
