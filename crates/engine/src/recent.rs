//! Recent projects for the Home page: the list, most recently opened first, with each project's
//! last opened time, whether its file still exists and a thumbnail of its program picture. The
//! thumbnails live in the data directory (`thumbnails/<hash>.png`) and are written when a project
//! is saved, or opened without one.

use std::path::{Path, PathBuf};

use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// Longest side of a thumbnail, in pixels.
const THUMB_PX: f32 = 360.0;

/// FNV-1a: a hash that stays the same across builds, for thumbnail file names.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Where the Home page thumbnail of the project at `path` lives (None without a data directory).
pub fn thumbnail_for(s: &Session, path: &str) -> Option<PathBuf> {
    let dir = s.prefs_path.as_ref()?.parent()?;
    Some(dir.join("thumbnails").join(format!("{:016x}.png", fnv1a(path))))
}

/// Render the open project's program picture (at the playhead, else a third of the way in) as its
/// Home page thumbnail. Best effort: a failure is logged, never raised.
pub fn write_thumbnail(s: &Session) {
    let Some(path) = s.path.clone() else { return };
    let Some(out) = thumbnail_for(s, &path) else { return };
    let Some(q) = s.active_sequence() else { return };
    let (w, h) = (q.settings.width.max(1) as f32, q.settings.height.max(1) as f32);
    let dur = q.duration();
    let ph = s.playhead();
    let t = if ph > Tick::ZERO && ph < dur { ph } else { Tick(dur.0 / 3) };
    let Some(img) = s.render_program_at((THUMB_PX / w.max(h)).min(1.0), t) else { return };
    let png = match filmcraft_export::encode_png(img.over_black_rgba8(), img.w as u32, img.h as u32) {
        Ok(b) => b,
        Err(e) => return log::warn!("thumbnail of {path}: {e}"),
    };
    if let Some(d) = out.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = s.services.write_file(&out.to_string_lossy(), &png) {
        log::warn!("thumbnail of {path}: {e}");
    }
}

fn unix_secs(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

/// `file.recentProjects`: the recent projects, most recently opened first (projects without an
/// open time sort by their file's modification time).
pub fn list(s: &Session) -> Value {
    let g = &s.prefs.general;
    let mut rows: Vec<(i64, usize, Value)> = g
        .recent_projects
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let meta = std::fs::metadata(p).ok();
            let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(unix_secs);
            let opened = g.recent_opened.get(p).copied();
            let name = Path::new(p).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
            let thumb = thumbnail_for(s, p).filter(|t| t.exists()).map(|t| t.to_string_lossy().into_owned());
            let row = json!({
                "path": p,
                "name": name,
                "exists": meta.is_some(),
                "openedAt": opened,
                "modifiedAt": modified,
                "thumbnail": thumb,
                "open": s.path.as_deref() == Some(p.as_str()),
            });
            (opened.or(modified).unwrap_or(0), i, row)
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    Value::Array(rows.into_iter().map(|r| r.2).collect())
}

/// `file.forgetRecent {path}`: drop a project from the list (its file stays where it is).
fn forget(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.forgetRecent", "need `path`"))?.to_string();
    let mut next = s.prefs.clone();
    next.forget_recent(&path);
    s.set_prefs(next).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
    if let Some(t) = thumbnail_for(s, &path) {
        let _ = std::fs::remove_file(t);
    }
    Ok(list(s))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "file.recentProjects",
            label: "Recent Projects",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: |s, _| Ok(list(s)),
            journal: false,
        },
        CommandSpec {
            id: "file.forgetRecent",
            label: "Remove from Recent Projects",
            menu: &[],
            shortcut: None,
            params: r#"{"path":str}"#,
            enabled: always,
            run: forget,
            journal: false,
        },
    ]
}
