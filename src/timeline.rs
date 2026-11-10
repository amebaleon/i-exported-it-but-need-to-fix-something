//! Snapshot v1 uses absolute presentation-frame positions and explicit layer order.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub project_id: String,
    pub sequence_id: String,
    pub duration_frames: usize,
    pub settings: Settings,
    pub audio_revision: String,
    pub global_effects: Vec<Value>,
    pub dependencies: Dependencies,
    pub items: Vec<Item>,
}
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: String,
    pub preset_revision: String,
}
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Dependencies {
    FrameLocal,
    Unknown,
}
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub layer: u32,
    pub start: usize,
    pub end: usize,
    pub content: Content,
    pub effects: Vec<Effect>,
}
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Content {
    Clip {
        source_id: String,
        media_revision: String,
        source_in: usize,
    },
    Caption {
        text: String,
        style_revision: String,
    },
}
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", content = "parameters", rename_all = "snake_case")]
pub enum Effect {
    Opacity(Value),
    Transform(Value),
    Color(Value),
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Decision {
    Unchanged,
    Patch { ranges: Vec<[usize; 2]> },
    FullExportRequired { reason: String },
}
pub fn full(reason: &str) -> Decision {
    Decision::FullExportRequired {
        reason: reason.into(),
    }
}

impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && !self.project_id.is_empty()
                && !self.sequence_id.is_empty()
                && self.duration_frames > 0
                && !self.audio_revision.is_empty()
                && !self.settings.preset_revision.is_empty(),
            "incomplete or unknown snapshot"
        );
        ensure!(
            self.settings.codec == "h264"
                && self.settings.width > 0
                && self.settings.height > 0
                && crate::media::rational(&self.settings.fps)?
                    > crate::media::Rational::from_integer(0),
            "invalid export settings"
        );
        let mut ids = std::collections::HashSet::new();
        let mut layers: BTreeMap<u32, Vec<&Item>> = BTreeMap::new();
        for i in &self.items {
            ensure!(
                !i.id.is_empty()
                    && ids.insert(&i.id)
                    && i.start < i.end
                    && i.end <= self.duration_frames,
                "ambiguous item ID or bounds"
            );
            match &i.content {
                Content::Clip {
                    source_id,
                    media_revision,
                    ..
                } => ensure!(
                    !source_id.is_empty() && !media_revision.is_empty(),
                    "missing source identity"
                ),
                Content::Caption { style_revision, .. } => {
                    ensure!(!style_revision.is_empty(), "missing caption style revision")
                }
            }
            layers.entry(i.layer).or_default().push(i);
        }
        for layer in layers.values_mut() {
            layer.sort_by_key(|i| i.start);
            ensure!(
                layer.windows(2).all(|w| w[0].end <= w[1].start),
                "overlap on one layer has unknown ordering"
            );
        }
        Ok(())
    }
}
pub fn diff(old: &Snapshot, new: &Snapshot) -> Decision {
    if let Err(e) = old.validate().and_then(|_| new.validate()) {
        return full(&e.to_string());
    }
    if old.dependencies != Dependencies::FrameLocal || new.dependencies != Dependencies::FrameLocal
    {
        return full("transition, nested sequence, temporal effect or incomplete adapter coverage");
    }
    if old.project_id != new.project_id || old.sequence_id != new.sequence_id {
        return full("different project or sequence");
    }
    if old.settings != new.settings || old.duration_frames != new.duration_frames {
        return full("export settings or sequence duration changed");
    }
    if old.global_effects != new.global_effects
        || !old.global_effects.is_empty()
        || old.audio_revision != new.audio_revision
    {
        return full("global effects or audio dependency");
    }
    let index = |s: &Snapshot| {
        s.items
            .iter()
            .map(|i| (i.id.clone(), i.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let a = index(old);
    let b = index(new);
    let mut spans = Vec::new();
    for (before, after) in [(&a, &b), (&b, &a)] {
        for (id, item) in before {
            if after.get(id) != Some(item) {
                spans.push([item.start, item.end]);
            }
        }
    }
    spans.sort_unstable();
    let mut ranges: Vec<[usize; 2]> = Vec::new();
    for [a, b] in spans {
        if let Some(last) = ranges.last_mut().filter(|r| a <= r[1]) {
            last[1] = last[1].max(b);
        } else {
            ranges.push([a, b]);
        }
    }
    if ranges.is_empty() {
        Decision::Unchanged
    } else {
        Decision::Patch { ranges }
    }
}
