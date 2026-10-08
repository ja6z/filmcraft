//! Home (header house icon, and the start page with Settings ▸ General ▸ At Startup ▸ Show
//! Home): start a project, open one, or pick up a recent one. Recent projects are cards with the
//! project's thumbnail, newest opened first (`file.recentProjects`).

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::state::Mode;
use crate::theme::Tokens;

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Open(String),
    New,
    Browse,
}

const CARD_W: f32 = 228.0;
const THUMB_H: f32 = 128.0;
const CARD_H: f32 = THUMB_H + 52.0;
const GAP: f32 = 18.0;

fn pending_id() -> egui::Id {
    egui::Id::new("home-pending")
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(rect, t.radius, t.panel_bg);
    let left = rect.min.x + 32.0;
    ui.painter().text(pos2(left, rect.min.y + 34.0), Align2::LEFT_CENTER, "Home", Tokens::semibold(22.0), t.text);
    ui.painter().text(pos2(left, rect.min.y + 62.0), Align2::LEFT_CENTER, "Start a new project or pick up where you left off.", Tokens::ui(13.0), t.text_dim);

    // ---- actions
    let mut action: Option<Pending> = None;
    let mut demo = false;
    let actions = [
        ("New Project", "Start from an empty project", crate::icons::Icon::Plus, "home.new"),
        ("Open Project…", "Choose a .fcproj file", crate::icons::Icon::Folder, "home.open"),
        ("Demo Project", "A cut sequence to explore", crate::icons::Icon::Sequence, "home.demo"),
    ];
    for (i, (title, sub, icon, id)) in actions.iter().enumerate() {
        let r = Rect::from_min_size(pos2(left + i as f32 * (CARD_W + GAP), rect.min.y + 90.0), vec2(CARD_W, 76.0));
        let resp = ui.interact(r, egui::Id::new(*id), Sense::click());
        app.auto.add(id, r, title);
        let primary = i == 0;
        let bg = match (primary, resp.hovered()) {
            (true, true) => t.accent_hover,
            (true, false) => t.accent,
            (false, true) => t.hover,
            (false, false) => t.tl_header_bg,
        };
        let fg = if primary { Color32::WHITE } else { t.text };
        ui.painter().rect_filled(r, 10.0, bg);
        crate::icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 34.0, r.center().y), vec2(26.0, 26.0)), *icon, fg);
        ui.painter().text(pos2(r.min.x + 62.0, r.center().y - 9.0), Align2::LEFT_CENTER, *title, Tokens::semibold(14.0), fg);
        let sub_col = if primary { Color32::from_white_alpha(200) } else { t.text_dim };
        ui.painter().text(pos2(r.min.x + 62.0, r.center().y + 11.0), Align2::LEFT_CENTER, *sub, Tokens::ui(11.5), sub_col);
        if resp.clicked() {
            match i {
                0 => action = Some(Pending::New),
                1 => action = Some(Pending::Browse),
                _ => demo = true,
            }
        }
    }

    // ---- recent projects
    let recents = filmcraft_engine::recent::list(&app.session);
    let rows: &[Value] = recents.as_array().map_or(&[], Vec::as_slice);
    let top = rect.min.y + 196.0;
    ui.painter().text(pos2(left, top), Align2::LEFT_CENTER, "Recent Projects", Tokens::semibold(15.0), t.text);
    if rows.is_empty() {
        ui.painter().text(pos2(left, top + 30.0), Align2::LEFT_CENTER, "Projects you open or save show up here.", Tokens::ui(12.5), t.text_dim);
    }
    let now = filmcraft_engine::autosave::unix_now();
    let offset = app.session.persistence.as_ref().map(|p| p.local_offset);
    let grid = Rect::from_min_max(pos2(left, top + 20.0), pos2(rect.max.x - 24.0, rect.max.y - 12.0));
    let mut forget: Option<String> = None;
    let mut reveal: Option<String> = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(grid).id_salt("home-recent"));
    egui::ScrollArea::vertical().id_salt("home-recent-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        let cols = (((ui.available_width() + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
        for chunk in rows.chunks(cols) {
            ui.add_space(8.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_H), Sense::hover());
            for (c, p) in chunk.iter().enumerate() {
                let card = Rect::from_min_size(pos2(row.min.x + c as f32 * (CARD_W + GAP), row.min.y), vec2(CARD_W, CARD_H));
                if let Some(a) = recent_card(app, ui, card, p, now, offset, &mut forget, &mut reveal) {
                    action = Some(a);
                }
            }
            ui.add_space(GAP - 8.0);
        }
    });
    if let Some(p) = forget
        && let Err(e) = app.session.execute("file.forgetRecent", json!({"path": p}))
    {
        app.ui.status = e.to_string();
    }
    if let Some(p) = reveal
        && let Some(open) = app.hooks.open_path.as_mut()
        && let Err(e) = open(&p, true)
    {
        app.ui.status = e;
    }
    if demo {
        action = None;
        if app.session.is_dirty() {
            app.ui.status = "Save or close the current project first".into();
        } else if let Err(e) = app.session.execute("file.openDemoProject", json!({})) {
            app.ui.status = e.to_string();
        } else {
            app.ui.mode = Mode::Edit;
        }
    }
    if let Some(a) = action {
        if app.session.is_dirty() {
            ctx.data_mut(|d| d.insert_temp(pending_id(), Some(a)));
        } else {
            run(app, &ctx, a);
        }
    }
    unsaved_prompt(app, &ctx);
}

/// One recent-project card; returns an action when it was clicked.
#[allow(clippy::too_many_arguments)]
fn recent_card(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    card: Rect,
    p: &Value,
    now: i64,
    offset: Option<fn(i64) -> i32>,
    forget: &mut Option<String>,
    reveal: &mut Option<String>,
) -> Option<Pending> {
    let t = app.tokens;
    let path = p["path"].as_str().unwrap_or_default().to_string();
    let name = p["name"].as_str().unwrap_or_default();
    let exists = p["exists"].as_bool().unwrap_or(false);
    let is_open = p["open"].as_bool().unwrap_or(false);
    let resp = ui.interact(card, egui::Id::new(("home-card", &path)), Sense::click()).on_hover_text(&path);
    app.auto.add(&format!("home.recent.{name}"), card, name);
    let thumb = Rect::from_min_size(card.min, vec2(CARD_W, THUMB_H));
    let painter = ui.painter_at(card.expand(2.0));
    painter.rect_filled(thumb, 8.0, Color32::from_gray(18));
    match p["thumbnail"].as_str().and_then(|f| thumbnail_texture(ui.ctx(), f)) {
        Some(tex) => {
            let [w, h] = tex.size();
            let k = (thumb.width() / w.max(1) as f32).min(thumb.height() / h.max(1) as f32);
            let r = Rect::from_center_size(thumb.center(), vec2(w as f32 * k, h as f32 * k));
            let tint = if exists { Color32::WHITE } else { Color32::from_white_alpha(90) };
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
        None => {
            crate::icons::paint(&painter, Rect::from_center_size(thumb.center(), vec2(34.0, 34.0)), crate::icons::Icon::Film, t.text_dim);
        }
    }
    if resp.hovered() {
        painter.rect_stroke(thumb, 8.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
    }
    if is_open {
        let badge = Rect::from_min_size(thumb.min + vec2(8.0, 8.0), vec2(44.0, 18.0));
        painter.rect_filled(badge, 9.0, t.accent);
        painter.text(badge.center(), Align2::CENTER_CENTER, "Open", Tokens::semibold(10.5), Color32::WHITE);
    }
    let text_col = if exists { t.text } else { t.text_dim };
    let title = painter.layout(name.to_string(), Tokens::semibold(13.0), text_col, CARD_W);
    let title_row = title.rows.first().map_or(0.0, |r| r.rect().height());
    painter.with_clip_rect(Rect::from_min_size(pos2(card.min.x, thumb.max.y + 8.0), vec2(CARD_W, title_row + 1.0))).galley(
        pos2(card.min.x + 2.0, thumb.max.y + 8.0),
        title,
        text_col,
    );
    let when = if !exists {
        "File missing".to_string()
    } else {
        match p["openedAt"].as_i64() {
            Some(at) => format!("Opened {}", when_label(now, at, offset)),
            None => p["modifiedAt"].as_i64().map(|m| format!("Saved {}", when_label(now, m, offset))).unwrap_or_default(),
        }
    };
    painter.text(pos2(card.min.x + 2.0, thumb.max.y + 36.0), Align2::LEFT_CENTER, when, Tokens::ui(11.5), t.text_dim);
    // remove from the list (hover)
    let x = Rect::from_center_size(pos2(thumb.max.x - 16.0, thumb.min.y + 16.0), vec2(22.0, 22.0));
    let mut removed = false;
    if resp.hovered() || ui.rect_contains_pointer(x) {
        let xr = ui.interact(x, egui::Id::new(("home-forget", &path)), Sense::click()).on_hover_text("Remove from Recent Projects");
        app.auto.add(&format!("home.recent.{name}.remove"), x, "Remove from Recent Projects");
        painter.circle_filled(x.center(), 11.0, if xr.hovered() { Color32::from_black_alpha(230) } else { Color32::from_black_alpha(160) });
        crate::icons::paint(&painter, x.shrink(6.0), crate::icons::Icon::Close, Color32::WHITE);
        if xr.clicked() {
            *forget = Some(path.clone());
            removed = true;
        }
    }
    let mut open = false;
    resp.context_menu(|ui| {
        if ui.add_enabled(exists, egui::Button::new("Open")).clicked() {
            open = true;
            ui.close();
        }
        if ui.add_enabled(exists, egui::Button::new("Show in Finder")).clicked() {
            *reveal = Some(path.clone());
            ui.close();
        }
        if ui.button("Remove from Recent Projects").clicked() {
            *forget = Some(path.clone());
            ui.close();
        }
    });
    if removed || !(resp.clicked() || open) {
        return None;
    }
    if !exists {
        app.ui.status = format!("{path} is missing");
        return None;
    }
    if is_open {
        app.ui.mode = Mode::Edit;
        return None;
    }
    Some(Pending::Open(path))
}

/// "Just now", "12 min ago", "Today 14:05", "Yesterday 09:30", else the local date and time.
fn when_label(now: i64, at: i64, offset: Option<fn(i64) -> i32>) -> String {
    let off = |s: i64| offset.map_or(0, |f| f(s) as i64);
    let ago = now - at;
    if (0..60).contains(&ago) {
        return "just now".into();
    }
    if (60..3600).contains(&ago) {
        return format!("{} min ago", ago / 60);
    }
    let local = at + off(at);
    let day = |s: i64| s.div_euclid(86_400);
    let today = day(now + off(now));
    let hm = format!("{:02}:{:02}", local.rem_euclid(86_400) / 3600, local.rem_euclid(3600) / 60);
    match today - day(local) {
        0 => format!("today {hm}"),
        1 => format!("yesterday {hm}"),
        _ => filmcraft_engine::media_browser::format_date(local.max(0) as u64),
    }
}

/// The texture of a thumbnail file, reloaded when the file changes.
fn thumbnail_texture(ctx: &egui::Context, file: &str) -> Option<egui::TextureHandle> {
    let mtime = std::fs::metadata(file).ok()?.modified().ok()?;
    let id = egui::Id::new(("home-thumb", file));
    if let Some((m, tex)) = ctx.data(|d| d.get_temp::<(std::time::SystemTime, egui::TextureHandle)>(id))
        && m == mtime
    {
        return Some(tex);
    }
    let bytes = std::fs::read(file).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let tex = ctx.load_texture(format!("home-thumb-{file}"), color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, (mtime, tex.clone())));
    Some(tex)
}

fn run(app: &mut FilmcraftApp, ctx: &egui::Context, a: Pending) {
    let r = match a {
        Pending::Open(p) => app.session.execute("file.open", json!({"path": p})).map(|_| true).map_err(|e| e.to_string()),
        Pending::New => app.session.execute("file.newProject", json!({"name": "Untitled"})).map(|_| true).map_err(|e| e.to_string()),
        Pending::Browse => {
            let before = app.session.path.clone();
            crate::menus::invoke(app, ctx, "file.open", json!({})).map(|_| app.session.path != before).map_err(|e| e.to_string())
        }
    };
    match r {
        Ok(true) => app.ui.mode = Mode::Edit,
        Ok(false) => {}
        Err(e) => app.ui.status = e,
    }
}

/// "Save changes before …?" for an action picked while the project has unsaved changes.
fn unsaved_prompt(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(a) = ctx.data(|d| d.get_temp::<Option<Pending>>(pending_id())).flatten() else { return };
    let name = app
        .session
        .path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .map_or_else(|| app.session.project.name.clone(), |n| n.to_string_lossy().into_owned());
    let mut choice: Option<&str> = None;
    egui::Window::new("Unsaved Changes").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(format!("Save changes to “{name}” first?"));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let r = ui.button("Cancel");
            app.auto.add("home.unsaved.cancel", r.rect, "Cancel");
            if r.clicked() {
                choice = Some("cancel");
            }
            let r = ui.button("Don't Save");
            app.auto.add("home.unsaved.discard", r.rect, "Don't Save");
            if r.clicked() {
                choice = Some("discard");
            }
            let label = if app.session.path.is_some() { "Save" } else { "Save As…" };
            let r = ui.add(egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(app.tokens.accent));
            app.auto.add("home.unsaved.save", r.rect, label);
            if r.clicked() {
                choice = Some("save");
            }
        });
    });
    let Some(c) = choice.or_else(|| ctx.input(|i| i.key_pressed(egui::Key::Escape)).then_some("cancel")) else { return };
    ctx.data_mut(|d| d.remove::<Option<Pending>>(pending_id()));
    match c {
        "save" => {
            let cmd = if app.session.path.is_some() { "file.save" } else { "file.saveAs" };
            if let Err(e) = crate::menus::invoke(app, ctx, cmd, json!({})) {
                app.ui.status = e.to_string();
            } else if !app.session.is_dirty() {
                run(app, ctx, a);
            }
        }
        "discard" => run(app, ctx, a),
        _ => {}
    }
}
