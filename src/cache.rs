use crate::{
    media::{self, Video},
    timeline::{self, Decision, Snapshot},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(File::open(path)?)
        .with_context(|| format!("invalid JSON at {}", path.display()))
}
pub fn save(path: &Path, data: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut f = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut f, data)?;
    f.flush()?;
    f.as_file().sync_all()?;
    f.persist_noclobber(path)?;
    Ok(())
}
pub fn digest(path: &Path) -> Result<String> {
    let mut h = Sha256::new();
    io::copy(&mut File::open(path)?, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}
#[derive(Serialize, Deserialize)]
pub struct Cache {
    pub version: u32,
    pub original: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub project_id: String,
    pub sequence_id: String,
    pub preset: String,
    pub media: Value,
    pub snapshot: Option<Snapshot>,
}
fn matches(s: &Snapshot, v: &Video) -> Result<()> {
    s.validate()?;
    ensure!(
        s.duration_frames == v.samples.len()
            && media::rational(&s.settings.fps)? == v.fps
            && v.stream["width"] == s.settings.width
            && v.stream["height"] == s.settings.height
            && v.stream["codec_name"] == s.settings.codec,
        "snapshot and exported video format differ"
    );
    Ok(())
}
impl Cache {
    pub fn record(
        original: &Path,
        project_id: &str,
        sequence_id: &str,
        preset: &str,
        snapshot: Option<Snapshot>,
    ) -> Result<Self> {
        ensure!(
            !project_id.is_empty() && !sequence_id.is_empty() && !preset.is_empty(),
            "missing export identity"
        );
        let v = media::inspect(original)?;
        if let Some(s) = &snapshot {
            matches(s, &v)?;
            ensure!(
                s.project_id == project_id
                    && s.sequence_id == sequence_id
                    && s.settings.preset_revision == preset,
                "snapshot identity mismatch"
            );
        }
        Ok(Self {
            version: 1,
            size: v.path.metadata()?.len(),
            sha256: digest(&v.path)?,
            original: v.path,
            project_id: project_id.into(),
            sequence_id: sequence_id.into(),
            preset: preset.into(),
            media: json!({"video":v.stream,"audio":v.audio,"frames":v.samples.len()}),
            snapshot,
        })
    }
    pub fn decide(&self, current: &Snapshot) -> Decision {
        if self.version != 1 {
            return timeline::full("unsupported cache version");
        }
        let Some(old) = &self.snapshot else {
            return timeline::full("incomplete Premiere timeline snapshot");
        };
        if self.original.metadata().map(|m| m.len()).ok() != Some(self.size)
            || digest(&self.original).ok().as_ref() != Some(&self.sha256)
        {
            return timeline::full("cached export missing or changed");
        }
        timeline::diff(old, current)
    }
}
