use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use incremental_export::{
    cache::{self, Cache},
    core::{self, Edit},
    timeline::{self, Snapshot},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

#[derive(Parser)]
#[command(version, about = "Verified partial re-export of H.264 MP4 video")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Patch half-open, frame-aligned time ranges (seconds or fractions).
    Patch {
        #[arg(long)]
        original: PathBuf,
        #[arg(long)]
        replacement: PathBuf,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long,num_args=3,value_names=["START","END","MP4"],action=clap::ArgAction::Append)]
        edit: Vec<String>,
    },
    /// Associate a snapshot with an existing full export.
    Record {
        #[arg(long)]
        original: PathBuf,
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        cache: PathBuf,
    },
    /// Check the exported file fingerprint and compare snapshots.
    Diff {
        #[arg(long)]
        cache: PathBuf,
        #[arg(long)]
        current: PathBuf,
    },
    /// Process Premiere file-adapter requests in the user-selected exchange folder.
    Watch {
        #[arg(long)]
        folder: PathBuf,
        #[arg(long)]
        once: bool,
    },
}
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Record {
        project_id: String,
        sequence_id: String,
        preset: String,
    },
    Plan {
        baseline: String,
        project_id: String,
        sequence_id: String,
    },
}
fn job_name(name: &str) -> bool {
    name.starts_with("ix-")
        && name.len() < 100
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn handle(folder: &Path, job: &Path) -> Result<Value> {
    match cache::read(&job.join("request.json"))? {
        Request::Record {
            project_id,
            sequence_id,
            preset,
        } => {
            let path = job.join("export.mp4");
            ensure!(
                path.canonicalize()?.parent() == Some(job),
                "export outside job folder"
            );
            let cache_file = job.join("cache.json");
            let entry = if cache_file.try_exists()? {
                let saved: Cache = cache::read(&cache_file)?;
                ensure!(
                    saved.original == path.canonicalize()?
                        && saved.project_id == project_id
                        && saved.sequence_id == sequence_id
                        && saved.preset == preset
                        && saved.sha256 == cache::digest(&path)?,
                    "existing cache differs from request"
                );
                saved
            } else {
                let saved = Cache::record(&path, &project_id, &sequence_id, &preset, None)?;
                cache::save(&cache_file, &saved)?;
                saved
            };
            Ok(
                json!({"status":"recorded","frames":entry.media["frames"],"timeline_complete":false}),
            )
        }
        Request::Plan {
            baseline,
            project_id,
            sequence_id,
        } => {
            ensure!(job_name(&baseline), "invalid baseline ID");
            let root = folder.join(baseline).canonicalize()?;
            ensure!(
                root.parent() == Some(folder),
                "baseline outside exchange folder"
            );
            let entry: Cache = cache::read(&root.join("cache.json"))?;
            let reason = if entry.project_id != project_id || entry.sequence_id != sequence_id {
                "different project or sequence"
            } else {
                "Premiere caption, source and effect dependency coverage is unverified"
            };
            Ok(serde_json::to_value(timeline::full(reason))?)
        }
    }
}
fn watch(folder: PathBuf, once: bool) -> Result<()> {
    let root = folder.canonicalize()?;
    loop {
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() || !job_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let job = entry.path().canonicalize()?;
            let response = job.join("result.json");
            if job.parent() != Some(&root) || !job.join("ready").is_file() || response.exists() {
                continue;
            }
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(job.join("worker.lock"))?;
            match lock.try_lock() {
                Ok(()) => (),
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
            if response.try_exists()? {
                continue;
            }
            let result = handle(&root, &job)
                .unwrap_or_else(|e| json!({"status":"error","message":format!("{e:#}")}));
            cache::save(&response, &result)?;
        }
        if once {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(750));
    }
}
fn main() -> Result<()> {
    let value = match Cli::parse().command {
        Action::Patch {
            original,
            replacement,
            start,
            end,
            output,
            edit,
        } => {
            let mut edits = vec![Edit {
                start,
                end,
                replacement,
            }];
            for list in edit.as_chunks::<3>().0 {
                edits.push(Edit {
                    start: list[0].clone(),
                    end: list[1].clone(),
                    replacement: PathBuf::from(&list[2]),
                });
            }
            serde_json::to_value(core::patch(&original, &edits, &output)?)?
        }
        Action::Record {
            original,
            snapshot,
            cache: path,
        } => {
            let s: Snapshot = cache::read(&snapshot)?;
            let c = Cache::record(
                &original,
                &s.project_id,
                &s.sequence_id,
                &s.settings.preset_revision,
                Some(s.clone()),
            )?;
            cache::save(&path, &c)?;
            json!({"status":"recorded","cache":path})
        }
        Action::Diff {
            cache: path,
            current,
        } => {
            let entry: Cache = cache::read(&path)?;
            match cache::read(&current) {
                Ok(s) => serde_json::to_value(entry.decide(&s))?,
                Err(e) => serde_json::to_value(timeline::full(&format!(
                    "cannot establish timeline coverage: {e:#}"
                )))?,
            }
        }
        Action::Watch { folder, once } => return watch(folder, once),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
