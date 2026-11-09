//! Clean-up by age: "Delete clips older than" 14, 30, 60 or 90 days.
//!
//! Clips, screenshots and recordings alike, favorites never. Nothing is
//! deleted for good here — an expired clip goes to Recently deleted
//! (`trash.rs`) the way a delete by hand does, and waits there another
//! [`crate::trash::KEEP_DAYS`]. Runs beside `commands::purge_expired`: a
//! minute after the start, then every hour.

use tauri::{AppHandle, Manager};

use crate::commands::Asked;
use crate::model::Clip;
use crate::state::AppState;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Is `clip` past `days` at `now` (Unix ms)? `0` days is off. A favorite
/// stays, whatever its age; a clip from the future (a clock set back) too.
/// A clip restored from Recently deleted counts from its restore
/// (`kept_since`), or it would go straight back.
pub fn expired(clip: &Clip, kept_since: Option<i64>, days: u32, now: i64) -> bool {
    if days == 0 || clip.favorite {
        return false;
    }
    let since = kept_since.map_or(clip.created_at, |kept| kept.max(clip.created_at));
    now.saturating_sub(since) >= i64::from(days) * DAY_MS
}

/// The last word on a clip, on its row as it stands under the clip's lock:
/// still expired, and its file there. A file not found — a drive unplugged, a
/// network share away — must not make the clip vanish: the delete by hand
/// erases a clip without a file for good, the clean-up leaves it alone.
pub fn take(fresh: &Clip, kept_since: Option<i64>, days: u32, now: i64) -> bool {
    expired(fresh, kept_since, days, now) && std::path::Path::new(&fresh.path).is_file()
}

/// Moves what is past the age to Recently deleted. A clip that is busy (being
/// saved, exported, edited) or whose file is held open stays until the next
/// round, without a word to the user.
pub fn run(app: &AppHandle) {
    let state = app.state::<AppState>();
    let days = state.config.lock().delete_older_than_days;
    if days == 0 {
        return;
    }
    let now = crate::trash::now_ms();
    let Some(Ok((clips, kept))) =
        crate::clips::with_library(&state.library, |lib| Ok::<_, rusqlite::Error>((lib.list()?, lib.kept_since()?)))
    else {
        return;
    };
    let mut moved = 0u32;
    let mut stuck = 0u32;
    for clip in clips.iter().filter(|clip| expired(clip, kept.get(&clip.id).copied(), days, now)) {
        if crate::clips::passes_stopped() {
            break;
        }
        // Going up as a link, out to a friend or to a channel: next round.
        if crate::links::uploading(&clip.id) || crate::share::sending(app, &clip.id) || crate::webhook::posting(&clip.id) {
            continue;
        }
        let kept_since = kept.get(&clip.id).copied();
        // Asked again on the row as it stands under the clip's lock: a heart
        // set in the meantime keeps it, a file gone missing too.
        match crate::commands::trash_clip(&state, app, &clip.id, Asked::Automatically, &|fresh| {
            take(fresh, kept_since, days, now)
        }) {
            Ok(true) => moved += 1,
            Ok(false) => {}
            Err(err) => {
                stuck += 1;
                log::debug!("clean-up: clip '{}' stays for now: {err}", clip.id);
            }
        }
    }
    if moved > 0 {
        log::info!("clean-up: {moved} clip(s) older than {days} days moved to Recently deleted");
        crate::commands::announce_change(app);
    }
    if stuck > 0 {
        log::info!("clean-up: {stuck} clip(s) older than {days} days could not be moved, next try in an hour");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(created_at: i64) -> Clip {
        serde_json::from_value(serde_json::json!({
            "id": "a",
            "path": "C:/clips/a.mp4",
            "createdAt": created_at,
            "durationMs": 1000,
            "game": null,
            "width": 1920,
            "height": 1080,
            "sizeBytes": 1,
            "thumbPath": null,
        }))
        .expect("a clip")
    }

    #[test]
    fn off_deletes_nothing() {
        assert!(!expired(&clip(0), None, 0, 1000 * DAY_MS));
    }

    #[test]
    fn a_clip_goes_once_it_is_as_old_as_the_setting() {
        let day = DAY_MS;
        let now = 100 * day;
        assert!(!expired(&clip(now - 30 * day + 1), None, 30, now), "a moment short of 30 days");
        assert!(expired(&clip(now - 30 * day), None, 30, now));
        assert!(expired(&clip(now - 31 * day), None, 30, now));
        assert!(!expired(&clip(now - 31 * day), None, 60, now));
        assert!(expired(&clip(now - 90 * day), None, 90, now));
    }

    #[test]
    fn favorites_stay() {
        let mut old = clip(0);
        old.favorite = true;
        assert!(!expired(&old, None, 14, 1000 * DAY_MS));
    }

    #[test]
    fn screenshots_and_recordings_go_alike() {
        let now = 100 * DAY_MS;
        let mut shot = clip(0);
        shot.screenshot = true;
        let mut recording = clip(0);
        recording.recording = true;
        assert!(expired(&shot, None, 14, now));
        assert!(expired(&recording, None, 14, now));
        assert!(!expired(&clip(now - DAY_MS), None, 14, now), "a fresh one of any kind stays");
    }

    #[test]
    fn a_restored_clip_counts_from_its_restore() {
        let now = 100 * DAY_MS;
        let old = clip(now - 45 * DAY_MS);
        assert!(expired(&old, None, 30, now));
        assert!(!expired(&old, Some(now - DAY_MS), 30, now), "restored yesterday");
        assert!(expired(&old, Some(now - 30 * DAY_MS), 30, now), "restored a month ago");
        // A restore moment before the creation (a clock set back) does not
        // make a clip older than it is.
        assert!(!expired(&clip(now - DAY_MS), Some(0), 30, now));
    }

    #[test]
    fn a_clip_without_its_file_is_left_alone() {
        let now = 100 * DAY_MS;
        let dir = std::env::temp_dir().join(format!("clippiboy-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("old.mp4");
        std::fs::write(&file, b"x").unwrap();
        let mut present = clip(0);
        present.path = file.to_string_lossy().to_string();
        assert!(take(&present, None, 14, now));
        let mut missing = clip(0);
        missing.path = dir.join("gone.mp4").to_string_lossy().to_string();
        assert!(!take(&missing, None, 14, now), "file not found");
        missing.path = r"Z:\offline\drive\old.mp4".into();
        assert!(!take(&missing, None, 14, now), "drive not there");
        present.favorite = true;
        assert!(!take(&present, None, 14, now), "still only what is expired");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clock_set_back_deletes_nothing() {
        assert!(!expired(&clip(50 * DAY_MS), None, 14, 0));
    }
}
