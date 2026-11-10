use incremental_export::timeline::*;
use serde_json::json;
fn base() -> Snapshot {
    serde_json::from_str(include_str!("../examples/snapshot.json")).unwrap()
}
fn check(edit: impl FnOnce(&mut Snapshot), expected: Decision) {
    let old = base();
    let mut new = old.clone();
    edit(&mut new);
    assert_eq!(diff(&old, &new), expected);
}
fn local(ranges: Vec<[usize; 2]>) -> Decision {
    Decision::Patch { ranges }
}
#[test]
fn caption() {
    check(
        |s| {
            s.items[1].content = Content::Caption {
                text: "수정".into(),
                style_revision: "new".into(),
            }
        },
        local(vec![[264, 384]]),
    );
}
#[test]
fn moved_clip() {
    check(
        |s| {
            s.items[0].start = 60;
            s.items[0].end = 690;
        },
        local(vec![[0, 720]]),
    );
}
#[test]
fn added() {
    check(
        |s| {
            let mut i = s.items[0].clone();
            i.id = "insert".into();
            i.layer = 2;
            i.start = 120;
            i.end = 150;
            s.items.push(i);
        },
        local(vec![[120, 150]]),
    );
}
#[test]
fn removed() {
    check(
        |s| {
            s.items.remove(0);
        },
        local(vec![[0, 720]]),
    );
}
#[test]
fn disjoint() {
    check(
        |s| {
            s.items[1].start = 500;
            s.items[1].end = 620;
        },
        local(vec![[264, 384], [500, 620]]),
    );
}
#[test]
fn unchanged() {
    check(|s| s.items.reverse(), Decision::Unchanged);
}
#[test]
fn global() {
    let mut new = base();
    new.global_effects.push(json!("adjustment"));
    assert!(matches!(
        diff(&base(), &new),
        Decision::FullExportRequired { .. }
    ));
}
#[test]
fn settings() {
    let mut new = base();
    new.settings.width *= 2;
    assert!(matches!(
        diff(&base(), &new),
        Decision::FullExportRequired { .. }
    ));
}
#[test]
fn unsafe_dependencies() {
    for i in 0..4 {
        let mut new = base();
        match i {
            0 => new.dependencies = Dependencies::Unknown,
            1 => new.duration_frames += 1,
            2 => new.audio_revision = "edited".into(),
            _ => new.items[1].layer = 0,
        }
        assert!(matches!(
            diff(&base(), &new),
            Decision::FullExportRequired { .. }
        ));
    }
}
