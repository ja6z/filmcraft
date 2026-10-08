//! Headless UI test of the Home page: the header house icon opens it, recent projects are cards
//! (newest opened first), a card opens its project and goes to editing, and unsaved changes are
//! asked about first.
//!
//! Set `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` to also render the window offscreen with wgpu and write
//! `home-*.png` there; without it no GPU is needed.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let snapshots = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }

    /// Registered elements under `prefix` (once the timeline has settled after an open).
    fn elements(&mut self, prefix: &str) -> Value {
        for _ in 0..200 {
            let v = self.call("ui.elements", json!({"prefix": prefix}));
            if v["ok"] == json!(true) {
                return v["result"].clone();
            }
            self.frames(2);
        }
        panic!("ui.elements {prefix} never settled");
    }

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.elements(prefix);
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    fn mode(&mut self) -> String {
        self.ok("ui.inspect", json!({}))["ui"]["mode"].as_str().unwrap_or_default().to_string()
    }

    fn path(&mut self) -> Option<String> {
        self.ok("engine.execute", json!({"command": "project.inspect"}))["path"].as_str().map(str::to_string)
    }

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(2);
        match self.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                img.save(dir.join(format!("home-{name}.png"))).unwrap();
            }
            Err(e) => eprintln!("snapshot {name} skipped: {e}"),
        }
    }
}

#[test]
fn home_lists_recent_projects_and_opens_them() {
    let dir = std::env::temp_dir().join(format!("filmcraft-home-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::default();
    session.prefs_path = Some(dir.join("preferences.json"));
    session.execute("file.openDemoProject", json!({})).unwrap();
    let a = dir.join("Alpha.fcproj").to_string_lossy().into_owned();
    let b = dir.join("Bravo.fcproj").to_string_lossy().into_owned();
    session.execute("file.saveAs", json!({"path": a})).unwrap();
    session.execute("file.saveAs", json!({"path": b})).unwrap();
    session.prefs.general.recent_opened.insert(a.clone(), 1_000);
    session.prefs.general.recent_opened.insert(b.clone(), 2_000);
    let mut d = Driver::new(session);

    // the house icon opens Home: actions, then the recent projects (Bravo opened last)
    d.ok("ui.click", json!({"id": "header.home"}));
    d.frames(3);
    assert_eq!(d.mode(), "Home");
    let ids = d.ids("home.");
    for id in ["home.new", "home.open", "home.demo", "home.recent.Alpha", "home.recent.Bravo"] {
        assert!(ids.iter().any(|i| i == id), "{id} in {ids:?}");
    }
    let rects = d.elements("home.recent.");
    let x_of = |name: &str| {
        rects.as_array().unwrap().iter().find(|e| e["id"] == json!(format!("home.recent.{name}"))).map(|e| e["rect"][0].as_f64().unwrap()).unwrap()
    };
    assert!(x_of("Bravo") < x_of("Alpha"), "newest opened first");
    d.snapshot("recent");

    // a card opens its project and goes to editing
    d.ok("ui.click", json!({"id": "home.recent.Alpha"}));
    d.frames(3);
    assert_eq!(d.path().as_deref(), Some(a.as_str()));
    assert_eq!(d.mode(), "Edit");

    // with unsaved changes, Home asks first; Don't Save goes ahead
    d.ok("engine.execute", json!({"command": "file.newSequence", "params": {"name": "Extra"}}));
    assert_eq!(d.ok("engine.execute", json!({"command": "project.inspect"}))["dirty"], json!(true));
    d.ok("ui.set", json!({"mode": "home"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "home.recent.Bravo"}));
    d.frames(3);
    assert_eq!(d.path().as_deref(), Some(a.as_str()), "nothing opened yet");
    assert!(!d.ids("home.unsaved.").is_empty(), "the save prompt is up");
    d.snapshot("unsaved");
    d.ok("ui.click", json!({"id": "home.unsaved.discard"}));
    d.frames(3);
    assert_eq!(d.path().as_deref(), Some(b.as_str()));
    assert_eq!(d.mode(), "Edit");
    let _ = std::fs::remove_dir_all(&dir);
}
