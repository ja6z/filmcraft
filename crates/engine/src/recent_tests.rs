//! Recent projects for the Home page: open times, newest-opened-first order, missing files,
//! forgetting, and the thumbnails written on save.

use serde_json::json;

use crate::Session;
use crate::media_test_util::tmp_dir;

#[test]
fn recent_projects_list_newest_opened_first_with_thumbnails() {
    let dir = tmp_dir("recent");
    let mut s = Session { prefs_path: Some(dir.join("preferences.json")), ..Default::default() };
    s.execute("file.openDemoProject", json!({})).unwrap();
    let a = dir.join("a.fcproj").to_string_lossy().into_owned();
    let b = dir.join("b.fcproj").to_string_lossy().into_owned();
    s.execute("file.saveAs", json!({"path": a})).unwrap();
    let thumb_a = crate::recent::thumbnail_for(&s, &a).unwrap();
    let png = std::fs::read(&thumb_a).expect("saving writes the Home thumbnail");
    assert_eq!(&png[1..4], b"PNG");
    s.execute("file.saveAs", json!({"path": b})).unwrap();
    assert!(s.prefs.general.recent_opened.contains_key(&b), "a first save stamps the project");

    // ordered by when each was opened, not by the list order
    s.prefs.general.recent_opened.insert(a.clone(), 2_000);
    s.prefs.general.recent_opened.insert(b.clone(), 1_000);
    let l = s.execute("file.recentProjects", json!({})).unwrap();
    assert_eq!((l[0]["path"].as_str(), l[1]["path"].as_str()), (Some(a.as_str()), Some(b.as_str())), "{l}");
    assert_eq!(l[1]["open"], json!(true), "b is the open project");
    assert_eq!(l[0]["thumbnail"], json!(thumb_a.to_string_lossy()));

    // opening stamps the time again
    s.execute("file.open", json!({"path": b})).unwrap();
    let l = s.execute("file.recentProjects", json!({})).unwrap();
    assert_eq!(l[0]["path"], json!(b));
    assert!(l[0]["openedAt"].as_i64().unwrap() > 2_000);
    assert_eq!(l[0]["name"], json!("b"));

    // a missing file stays listed, marked missing; forgetting drops it and its thumbnail
    std::fs::remove_file(&a).unwrap();
    let l = s.execute("file.recentProjects", json!({})).unwrap();
    assert_eq!(l[1]["exists"], json!(false));
    let l = s.execute("file.forgetRecent", json!({"path": a})).unwrap();
    assert_eq!(l.as_array().unwrap().len(), 1);
    assert!(!thumb_a.exists());
    assert!(!s.prefs.general.recent_opened.contains_key(&a));

    // all of it is saved with the preferences
    let p = crate::autosave::Preferences::load(&dir.join("preferences.json"));
    assert_eq!(p.general.recent_projects, vec![b.clone()]);
    assert!(p.general.recent_opened.contains_key(&b));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_recent_list_is_capped_and_forgets_stale_times() {
    let mut p = crate::autosave::Preferences::default();
    for i in 0..40 {
        p.note_recent(&format!("/p{i}.fcproj"), Some(i));
    }
    assert_eq!(p.general.recent_projects.len(), crate::settings::RECENT_PROJECTS);
    assert_eq!(p.general.recent_projects[0], "/p39.fcproj");
    assert_eq!(p.general.recent_opened.len(), crate::settings::RECENT_PROJECTS, "times of dropped projects go too");
    assert!(!p.general.recent_opened.contains_key("/p0.fcproj"));
}
