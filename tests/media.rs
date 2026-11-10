mod common;
use anyhow::Result;
use common::{Pictures, fixture, pcm, pictures};
use incremental_export::{
    adapter::{self, MockAdapter, Outcome},
    cache::{self, Cache},
    core::{self, Edit},
    media::{command, inspect, run},
    timeline::{Content, Decision, Snapshot},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    _dir: tempfile::TempDir,
    orig: PathBuf,
    rep: PathBuf,
    a: Pictures,
    b: Pictures,
}
fn files() -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("한글 원본 ")
        .tempdir()
        .unwrap();
    let (orig, rep) = fixture(dir.path(), "192x108").unwrap();
    Fixture {
        a: pictures(&orig).unwrap(),
        b: pictures(&rep).unwrap(),
        orig,
        rep,
        _dir: dir,
    }
}
fn edit(path: &Path, a: &str, b: &str) -> Edit {
    Edit {
        start: a.into(),
        end: b.into(),
        replacement: path.into(),
    }
}
fn exercise(edits: &[(usize, usize)], expected: &[[usize; 2]]) -> Result<()> {
    let f = files();
    let dir = tempfile::Builder::new().prefix("공백 media ").tempdir()?;
    let out = dir.path().join("수정 결과.mp4");
    let args: Vec<_> = edits
        .iter()
        .map(|(a, b)| edit(&f.rep, &format!("{a}/24"), &format!("{b}/24")))
        .collect();
    let orig_digest = cache::digest(&f.orig)?;
    let report = core::patch(&f.orig, &args, &out)?;
    assert_eq!(report.copied_replacement_packets, edits.len() * 120);
    assert_eq!(
        report.reencoded_frame_count
            + report.verified_reused_packets
            + report.copied_replacement_packets,
        720
    );
    let actual = pictures(&out)?;
    let orig = inspect(&f.orig)?;
    let result = inspect(&out)?;
    let replacement = inspect(&f.rep)?;
    for &(a, b) in edits {
        for i in a..b {
            assert_eq!(
                result.samples[i].data_hash,
                replacement.samples[i - a].data_hash
            );
            assert_eq!(actual.hashes[i], f.b.hashes[i - a]);
        }
    }
    assert_eq!(actual.hashes.len(), 720);
    assert_eq!(actual.gray.len(), 720 * 576);
    let mut reused = 0;
    for i in 0..720 {
        if expected.iter().any(|[a, b]| (*a..*b).contains(&i)) {
            let (pic, j) = edits
                .iter()
                .find(|(a, b)| (*a..*b).contains(&i))
                .map(|(a, _)| (&f.b, i - a))
                .unwrap_or((&f.a, i));
            let mse = actual.gray[i * 576..(i + 1) * 576]
                .iter()
                .zip(&pic.gray[j * 576..(j + 1) * 576])
                .map(|(x, y)| (i32::from(*x) - i32::from(*y)).pow(2))
                .sum::<i32>() as f64
                / 576.;
            assert!(mse < 12., "wrong decoded picture at {i}, MSE={mse}");
        } else {
            reused += 1;
            assert_eq!(actual.hashes[i], f.a.hashes[i]);
            assert_eq!(orig.samples[i].data_hash, result.samples[i].data_hash);
        }
    }
    assert_eq!(reused, report.verified_reused_packets);
    assert_eq!(pcm(&f.orig)?, pcm(&out)?);
    assert_eq!(cache::digest(&f.orig)?, orig_digest);
    Ok(())
}
#[test]
fn middle_non_idr() -> Result<()> {
    exercise(&[(264, 384)], &[[240, 384]])
}
#[test]
fn start() -> Result<()> {
    exercise(&[(0, 120)], &[[0, 144]])
}
#[test]
fn end() -> Result<()> {
    exercise(&[(600, 720)], &[[576, 720]])
}
#[test]
fn multiple() -> Result<()> {
    exercise(&[(72, 192), (444, 564)], &[[48, 192], [432, 576]])
}
#[test]
fn invalid() -> Result<()> {
    let f = files();
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("new.mp4");
    for (a, b) in [
        ("-1", "4"),
        ("0", "4"),
        ("0.001", "5.001"),
        ("28", "33"),
        ("5", "4"),
    ] {
        assert!(core::patch(&f.orig, &[edit(&f.rep, a, b)], &out).is_err());
        assert!(!out.exists());
    }
    assert!(core::patch(&f.orig, &[edit(&f.rep, "0", "5")], &f.orig).is_err());
    let wrong = dir.path().join("mpeg4.mp4");
    run(command("ffmpeg")
        .arg("-i")
        .arg(&f.rep)
        .args(["-c:v", "mpeg4"])
        .arg(&wrong))?;
    let proc = Command::new(env!("CARGO_BIN_EXE_incremental-export"))
        .args(["patch", "--original"])
        .arg(&f.orig)
        .arg("--replacement")
        .arg(wrong)
        .args(["--start", "2", "--end", "7", "--output"])
        .arg(&out)
        .output()?;
    assert!(!proc.status.success());
    assert!(String::from_utf8_lossy(&proc.stderr).contains("H.264"));
    assert!(!out.exists());
    Ok(())
}
#[test]
fn cache_and_mock() -> Result<()> {
    let f = files();
    let dir = tempfile::tempdir()?;
    let saved = dir.path().join("baseline.mp4");
    fs::copy(&f.orig, &saved)?;
    let s: Snapshot = serde_json::from_str(include_str!("../examples/snapshot.json"))?;
    let record = Cache::record(
        &saved,
        &s.project_id,
        &s.sequence_id,
        &s.settings.preset_revision,
        Some(s.clone()),
    )?;
    let path = dir.path().join("cache.json");
    cache::save(&path, &record)?;
    let cache: Cache = cache::read(&path)?;
    assert_eq!(cache.decide(&s), Decision::Unchanged);
    let mut current = s.clone();
    current.items[1].content = Content::Caption {
        text: "new".into(),
        style_revision: "v1".into(),
    };
    let mut mock = MockAdapter {
        current,
        renders: vec![([264, 384], f.rep.clone())],
    };
    let outcome = adapter::reexport(&mut mock, &cache, &dir.path().join("new.mp4"))?;
    assert!(matches!(outcome,Outcome::Patched(r) if r.verified_reused_packets==576));
    let mut bytes = fs::read(&saved)?;
    bytes[0] ^= 1;
    fs::write(&saved, bytes)?;
    assert!(matches!(
        cache.decide(&s),
        Decision::FullExportRequired { .. }
    ));
    Ok(())
}
