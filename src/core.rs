use crate::{
    media::{self, DISPLAY, Packet, Video, command, run},
    mp4::{self, Rendered},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone)]
pub struct Edit {
    pub start: String,
    pub end: String,
    pub replacement: PathBuf,
}
struct Change {
    start: usize,
    end: usize,
    video: Video,
}
struct Group {
    start: usize,
    end: usize,
    edits: Vec<Change>,
}

enum Work {
    Encode(Group),
    Copy(Box<Change>),
}

// A replacement starts with its own IDR. When its sample entry matches, the
// two original GOP fringes can be encoded independently around that bitstream.
fn split_group(source: &Video, group: Group) -> Vec<Work> {
    let mut work = Vec::new();
    let mut start = group.start;
    let mut pending = Vec::new();
    for edit in group.edits {
        let config = &edit.video.stream["extradata_hash"];
        if config.is_string() && *config == source.stream["extradata_hash"] {
            if start < edit.start {
                work.push(Work::Encode(Group {
                    start,
                    end: edit.start,
                    edits: std::mem::take(&mut pending),
                }));
            }
            start = edit.end;
            work.push(Work::Copy(Box::new(edit)));
        } else {
            pending.push(edit);
        }
    }
    if start < group.end {
        work.push(Work::Encode(Group {
            start,
            end: group.end,
            edits: pending,
        }));
    }
    work
}

fn plan(source: &Video, edits: &[Edit]) -> Result<Vec<Group>> {
    let mut changes = Vec::new();
    for e in edits {
        let start = source.frame(&e.start)?;
        let end = source.frame(&e.end)?;
        ensure!(
            start < end && end <= source.samples.len(),
            "edit outside timeline or empty"
        );
        let video = media::inspect(&e.replacement)?;
        ensure!(
            video.samples.len() == end - start && video.fps == source.fps,
            "replacement length or FPS mismatch"
        );
        ensure!(
            DISPLAY.iter().all(|&k| video.stream[k] == source.stream[k]),
            "replacement display format mismatch"
        );
        changes.push(Change { start, end, video });
    }
    changes.sort_by_key(|e| e.start);
    let mut groups: Vec<Group> = Vec::new();
    let mut cursor = 0;
    for e in changes {
        ensure!(cursor <= e.start, "overlapping edits");
        cursor = e.end;
        let start = source.boundaries[source.boundaries.partition_point(|&x| x <= e.start) - 1];
        let end = source.boundaries[source.boundaries.partition_point(|&x| x < e.end)];
        if let Some(group) = groups.last_mut().filter(|g| start <= g.end) {
            group.end = end;
            group.edits.push(e);
        } else {
            groups.push(Group {
                start,
                end,
                edits: vec![e],
            });
        }
    }
    ensure!(!groups.is_empty(), "no edits supplied");
    ensure!(
        groups.iter().map(|g| g.end - g.start).sum::<usize>() < source.samples.len(),
        "all GOPs affected; full export required"
    );
    Ok(groups)
}

fn render(source: &Video, g: &Group, path: &Path) -> Result<Video> {
    let mut cmd = command("ffmpeg");
    cmd.arg("-xerror");
    let mut filters = Vec::new();
    let mut inputs = String::new();
    let mut add = |video: &Video, start: usize, count: usize| {
        let micros =
            (media::Rational::from_integer(start as i128 * 1_000_000) / video.fps).to_integer();
        let i = filters.len();
        cmd.arg("-ss")
            .arg(format!("{micros}us"))
            .arg("-i")
            .arg(&video.path);
        filters.push(format!(
            "[{i}:v:0]trim=end_frame={count},setpts=PTS-STARTPTS[v{i}]"
        ));
        inputs.push_str(&format!("[v{i}]"));
    };
    let mut cursor = g.start;
    for e in &g.edits {
        if cursor < e.start {
            add(source, cursor, e.start - cursor);
        }
        add(&e.video, 0, e.end - e.start);
        cursor = e.end;
    }
    if cursor < g.end {
        add(source, cursor, g.end - cursor);
    }
    filters.push(format!("{inputs}concat=n={}:v=1:a=0[out]", filters.len()));
    let s = &source.stream;
    let profile = match s["profile"].as_str() {
        Some("High") => "high",
        Some("Main") => "main",
        Some("Constrained Baseline") => "baseline",
        _ => anyhow::bail!("unsupported H.264 profile"),
    };
    cmd.arg("-filter_complex").arg(filters.join(";")).args([
        "-map",
        "[out]",
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        source.encoder_crf.as_deref().unwrap_or("18"),
        "-pix_fmt",
        "yuv420p",
        "-profile:v",
        profile,
    ]);
    cmd.arg("-level:v")
        .arg(s["level"].to_string())
        .arg("-r")
        .arg(source.fps.to_string());
    if s["has_b_frames"] == 0 {
        cmd.args(["-bf", "0"]);
    }
    run(cmd.arg(path))?;
    let part = media::inspect(path)?;
    ensure!(
        part.samples.len() == g.end - g.start && part.fps == source.fps,
        "rendered frame count or rate mismatch"
    );
    ensure!(
        s["extradata_hash"].is_string() && s["extradata_hash"] == part.stream["extradata_hash"],
        "rebuilt GOP SPS/PPS differs; full export required"
    );
    Ok(part)
}

fn same_audio(a: &Packet, b: &Packet) -> bool {
    a.pts == b.pts
        && a.dts == b.dts
        && a.duration == b.duration
        && a.data_hash == b.data_hash
        && a.side_data_list == b.side_data_list
}
fn verify(source: &Video, parts: &[Rendered], path: &Path) -> Result<()> {
    let out = media::inspect_output(path)?;
    ensure!(
        out.fps == source.fps
            && out.samples.len() == source.samples.len()
            && out.step == source.step
            && DISPLAY
                .iter()
                .chain(["time_base", "extradata_hash"].iter())
                .all(|&k| out.stream[k] == source.stream[k]),
        "output video format or duration changed"
    );
    let mut k = 0;
    for (i, (old, new)) in source.samples.iter().zip(&out.samples).enumerate() {
        if k < parts.len() && i == parts[k].end {
            k += 1;
        }
        let (want, pts) = if let Some(part) = parts.get(k).filter(|p| i >= p.start) {
            let s = &part.video.samples[i - part.start];
            (
                s,
                (part.start as i64 + s.pts / part.video.step) * source.step,
            )
        } else {
            (old, old.pts)
        };
        ensure!(
            want.data_hash == new.data_hash
                && want.flags == new.flags
                && pts == new.pts
                && old.dts == new.dts,
            "packet or timing mismatch at index {i}"
        );
    }
    ensure!(
        source.audio == out.audio
            && source.audio_samples.len() == out.audio_samples.len()
            && source
                .audio_samples
                .iter()
                .zip(&out.audio_samples)
                .all(|(a, b)| same_audio(a, b)),
        "audio stream or timing changed"
    );
    run(command("ffmpeg")
        .args(["-xerror", "-err_detect", "explode", "-i"])
        .arg(path)
        .args(["-map", "0:v", "-map", "0:a?", "-f", "null", "-"]))?;
    Ok(())
}

#[derive(Serialize, Debug)]
pub struct Report {
    pub frames: usize,
    pub fps: String,
    pub encoded_frames: Vec<[usize; 2]>,
    pub reused_frames: Vec<[usize; 2]>,
    pub reencoded_frame_count: usize,
    pub copied_replacement_packets: usize,
    pub replacement_packet_ranges: Vec<[usize; 2]>,
    pub verified_reused_packets: usize,
    pub staged_gop_bytes: u64,
    pub output_media_payload_bytes: u64,
    pub output_bytes: u64,
    pub elapsed_ms: u128,
    pub analyze_ms: u128,
    pub encode_ms: u128,
    pub mux_ms: u128,
    pub verify_ms: u128,
}
pub fn patch(original: &Path, edits: &[Edit], output: &Path) -> Result<Report> {
    let start = Instant::now();
    ensure!(
        !output.try_exists()?
            && output
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("mp4")),
        "choose a new .mp4 output path"
    );
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let source = media::inspect(original).context("analyze original")?;
    let groups = plan(&source, edits)?;
    let analyze_ms = start.elapsed().as_millis();
    let work = tempfile::Builder::new()
        .prefix(".incremental-")
        .tempdir_in(parent)?;
    let mut parts = Vec::new();
    let mut staged_gop_bytes = 0;
    let timer = Instant::now();
    let mut encoded_frames = Vec::new();
    let mut replacement_packet_ranges = Vec::new();
    for (i, task) in groups
        .into_iter()
        .flat_map(|g| split_group(&source, g))
        .enumerate()
    {
        let g = match task {
            Work::Copy(change) => {
                replacement_packet_ranges.push([change.start, change.end]);
                parts.push(Rendered {
                    start: change.start,
                    end: change.end,
                    video: change.video,
                });
                continue;
            }
            Work::Encode(g) => g,
        };
        let path = work.path().join(format!("gop{i}.mp4"));
        let video = render(&source, &g, &path)?;
        encoded_frames.push([g.start, g.end]);
        staged_gop_bytes += path.metadata()?.len();
        parts.push(Rendered {
            start: g.start,
            end: g.end,
            video,
        });
    }
    let encode_ms = timer.elapsed().as_millis();
    let timer = Instant::now();
    let target = tempfile::Builder::new()
        .prefix(".verified-")
        .suffix(".mp4")
        .tempfile_in(parent)?;
    let output_media_payload_bytes = mp4::write(&source, &parts, target.path())?;
    let mux_ms = timer.elapsed().as_millis();
    let timer = Instant::now();
    verify(&source, &parts, target.path())?;
    let verify_ms = timer.elapsed().as_millis();
    let output_bytes = target.as_file().metadata()?.len();
    target.persist_noclobber(output)?;
    let mut reused_frames = Vec::new();
    let mut cursor = 0;
    for g in &parts {
        if cursor < g.start {
            reused_frames.push([cursor, g.start]);
        }
        cursor = g.end;
    }
    let frames = source.samples.len();
    if cursor < frames {
        reused_frames.push([cursor, frames]);
    }
    let reencoded_frame_count = encoded_frames.iter().map(|[a, b]| b - a).sum();
    let copied_replacement_packets = replacement_packet_ranges.iter().map(|[a, b]| b - a).sum();
    Ok(Report {
        frames,
        fps: source.fps.to_string(),
        encoded_frames,
        reused_frames,
        reencoded_frame_count,
        copied_replacement_packets,
        replacement_packet_ranges,
        verified_reused_packets: frames - reencoded_frame_count - copied_replacement_packets,
        staged_gop_bytes,
        output_media_payload_bytes,
        output_bytes,
        elapsed_ms: start.elapsed().as_millis(),
        analyze_ms,
        encode_ms,
        mux_ms,
        verify_ms,
    })
}
