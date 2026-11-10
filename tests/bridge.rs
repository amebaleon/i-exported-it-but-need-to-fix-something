mod common;
use anyhow::Result;
use incremental_export::cache;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
fn watch(root: &Path) -> Result<()> {
    let r = Command::new(env!("CARGO_BIN_EXE_incremental-export"))
        .args(["watch", "--once", "--folder"])
        .arg(root)
        .output()?;
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    Ok(())
}
#[test]
fn record_and_plan_mailbox() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let (orig, _) = common::fixture(root, "192x108")?;
    let record = root.join("ix-first");
    fs::create_dir(&record)?;
    fs::rename(orig, record.join("export.mp4"))?;
    cache::save(
        &record.join("request.json"),
        &json!({"command":"record","project_id":"p","sequence_id":"s","preset":"h264.epr"}),
    )?;
    watch(root)?;
    assert!(!record.join("result.json").exists());
    fs::write(record.join("ready"), [])?;
    watch(root)?;
    let result: Value = cache::read(&record.join("result.json"))?;
    assert_eq!(result["status"], "recorded");
    assert_eq!(result["frames"], 720);
    fs::remove_file(record.join("result.json"))?;
    watch(root)?;
    assert_eq!(cache::read::<Value>(&record.join("result.json"))?, result);
    let next = root.join("ix-next");
    fs::create_dir(&next)?;
    cache::save(
        &next.join("request.json"),
        &json!({"command":"plan","baseline":"ix-first","project_id":"p","sequence_id":"s"}),
    )?;
    fs::write(next.join("ready"), [])?;
    watch(root)?;
    let plan: Value = cache::read(&next.join("result.json"))?;
    assert_eq!(plan["status"], "full_export_required");
    Ok(())
}
