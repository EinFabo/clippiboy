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
pub fn expired(clip: &Clip, days: u32, now: i64) -> bool {
    if days == 0 || clip.favorite {
        return false;
    }
    now.saturating_sub(clip.created_at) >= i64::from(days) * DAY_MS
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
    let clips = crate::clips::with_library(&state.library, |lib| lib.list())
        .and_then(Result::ok)
        .unwrap_or_default();
    let mut moved = 0u32;
    let mut stuck = 0u32;
    for clip in clips.iter().filter(|clip| expired(clip, days, now)) {
        if crate::clips::passes_stopped() {
            break;
        }
        // Asked again on the row as it stands under the clip's lock: a heart
        // set in the meantime keeps it.
        match crate::commands::trash_clip(&state, app, &clip.id, Asked::Automatically, &|fresh| {
            expired(fresh, days, now)
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
        assert!(!expired(&clip(0), 0, 1000 * DAY_MS));
    }

    #[test]
    fn a_clip_goes_once_it_is_as_old_as_the_setting() {
        let day = DAY_MS;
        let now = 100 * day;
        assert!(!expired(&clip(now - 30 * day + 1), 30, now), "a moment short of 30 days");
        assert!(expired(&clip(now - 30 * day), 30, now));
        assert!(expired(&clip(now - 31 * day), 30, now));
        assert!(!expired(&clip(now - 31 * day), 60, now));
        assert!(expired(&clip(now - 90 * day), 90, now));
    }

    #[test]
    fn favorites_stay() {
        let mut old = clip(0);
        old.favorite = true;
        assert!(!expired(&old, 14, 1000 * DAY_MS));
    }

    #[test]
    fn screenshots_and_recordings_go_alike() {
        let now = 100 * DAY_MS;
        let mut shot = clip(0);
        shot.screenshot = true;
        let mut recording = clip(0);
        recording.recording = true;
        assert!(expired(&shot, 14, now));
        assert!(expired(&recording, 14, now));
        assert!(!expired(&clip(now - DAY_MS), 14, now), "a fresh one of any kind stays");
    }

    #[test]
    fn a_clock_set_back_deletes_nothing() {
        assert!(!expired(&clip(50 * DAY_MS), 14, 0));
    }
}
