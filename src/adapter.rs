//! Contract for editors that can prove complete frame-local coverage.
use crate::{
    cache::Cache,
    core::{self, Edit, Report},
    media::{Rational, rational},
    timeline::{Decision, Snapshot},
};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub trait EditorAdapter {
    fn snapshot(&self) -> Result<Snapshot>;
    fn render_range(&mut self, range: [usize; 2]) -> Result<PathBuf>;
}
pub enum Outcome {
    NoChanges,
    Patched(Box<Report>),
    FullExportRequired(String),
}
pub fn reexport(adapter: &mut impl EditorAdapter, cache: &Cache, output: &Path) -> Result<Outcome> {
    let current = adapter.snapshot()?;
    let ranges = match cache.decide(&current) {
        Decision::Unchanged => return Ok(Outcome::NoChanges),
        Decision::FullExportRequired { reason } => return Ok(Outcome::FullExportRequired(reason)),
        Decision::Patch { ranges } => ranges,
    };
    let fps = rational(&current.settings.fps)?;
    let seconds = |f| (Rational::from_integer(f as i128) / fps).to_string();
    let mut edits = Vec::new();
    for [a, b] in ranges {
        edits.push(Edit {
            start: seconds(a),
            end: seconds(b),
            replacement: adapter.render_range([a, b])?,
        });
    }
    Ok(Outcome::Patched(Box::new(core::patch(
        &cache.original,
        &edits,
        output,
    )?)))
}
/// Offline editor simulator. Supplied media are complete renders of the requested ranges.
pub struct MockAdapter {
    pub current: Snapshot,
    pub renders: Vec<([usize; 2], PathBuf)>,
}
impl EditorAdapter for MockAdapter {
    fn snapshot(&self) -> Result<Snapshot> {
        Ok(self.current.clone())
    }
    fn render_range(&mut self, range: [usize; 2]) -> Result<PathBuf> {
        if let Some(i) = self.renders.iter().position(|(r, _)| *r == range) {
            return Ok(self.renders.remove(i).1);
        }
        bail!("mock render unavailable for {range:?}")
    }
}
