//! Headless UI test of the Text panel ▸ Captions style strip: font family and style, and the
//! word-by-word highlight (mode and colour).

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

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(2);
        match self.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                img.save(dir.join(format!("captions-{name}.png"))).unwrap();
            }
            Err(e) => eprintln!("snapshot {name} skipped: {e}"),
        }
    }
}

#[test]
fn caption_style_strip_sets_font_and_highlight() {
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    session.execute("captions.add", json!({"text": "Ella me traicionó", "seconds": 1.0})).unwrap();
    let mut d = Driver::new(session);
    d.ok("ui.panel.show", json!({"panel": "Text"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "text.tab.Captions"}));
    d.frames(3);
    let ids = d.ids("text.captions.style.");
    for id in [
        "text.captions.style.font",
        "text.captions.style.fontStyle",
        "text.captions.style.highlight.none",
        "text.captions.style.highlight.color",
        "text.captions.style.highlight.box",
    ] {
        assert!(ids.iter().any(|i| i == id), "{id} in {ids:?}");
    }
    assert!(!ids.iter().any(|i| i == "text.captions.style.highlightColor"), "no colour while the highlight is off");
    d.ok("ui.click", json!({"id": "text.captions.style.highlight.box"}));
    d.frames(3);
    let style = |d: &mut Driver| d.ok("engine.execute", json!({"command": "captions.list"}))["tracks"][0]["style"].clone();
    assert_eq!(style(&mut d)["highlight"], json!("box"));
    assert!(d.ids("text.captions.style.highlightColor").len() == 1, "the colour button shows up");
    d.snapshot("caption-style");
    // the font menu lists the bundled faces; picking one sets the track font
    d.ok("ui.click", json!({"id": "text.captions.style.font"}));
    d.frames(3);
    d.ok("engine.execute", json!({"command": "captions.setStyle", "params": {"font": "Noto Serif", "fontStyle": "Bold"}}));
    d.frames(2);
    let st = style(&mut d);
    assert_eq!((st["font"].as_str(), st["fontStyle"].as_str()), (Some("Noto Serif"), Some("Bold")));
}
