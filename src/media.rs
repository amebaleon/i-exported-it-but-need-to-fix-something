use anyhow::{Context, Result, bail, ensure};
use num_rational::Ratio;
use serde::Deserialize;
use serde_json::Value;
use std::{
    env,
    ffi::OsStr,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::Command,
};

pub type Rational = Ratio<i128>;

/// Decimal seconds and exact ratios (e.g. 1001/30000).
pub fn rational(s: &str) -> Result<Rational> {
    let parse = |v: &str| -> Result<i128> { Ok(v.parse::<i64>()?.into()) };
    if let Some((n, d)) = s.split_once('/') {
        let d = parse(d)?;
        ensure!(d > 0, "invalid denominator");
        return Ok(Ratio::new(parse(n)?, d));
    }
    if let Some((whole, frac)) = s.split_once('.') {
        ensure!(
            !frac.is_empty() && frac.len() <= 12 && frac.bytes().all(|c| c.is_ascii_digit()),
            "invalid decimal"
        );
        return Ok(Ratio::new(
            parse(&format!("{whole}{frac}"))?,
            10i128.pow(frac.len() as u32),
        ));
    }
    Ok(Ratio::from_integer(parse(s)?))
}

pub fn command(name: &str) -> Command {
    let mut cmd = Command::new(env::var_os(name.to_uppercase()).unwrap_or_else(|| name.into()));
    cmd.args(["-v", "error"]);
    if name == "ffmpeg" {
        cmd.arg("-nostdin");
    }
    cmd
}

pub fn run(cmd: &mut Command) -> Result<Vec<u8>> {
    let result = cmd
        .output()
        .with_context(|| format!("could not start {:?}", cmd.get_program()))?;
    ensure!(
        result.status.success() && result.stderr.is_empty(),
        "{:?} {}: {}",
        cmd.get_program(),
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(result.stdout)
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct Packet {
    pub stream_index: usize,
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    #[serde(deserialize_with = "number_string")]
    pub pos: u64,
    #[serde(deserialize_with = "number_string")]
    pub size: u64,
    pub flags: String,
    pub data_hash: String,
    #[serde(default)]
    pub side_data_list: Value,
}

fn number_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    String::deserialize(d)?
        .parse()
        .map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Value>,
    packets: Vec<Packet>,
    #[serde(default)]
    chapters: Vec<Value>,
}

pub const DISPLAY: &[&str] = &[
    "width",
    "height",
    "pix_fmt",
    "field_order",
    "sample_aspect_ratio",
    "color_range",
    "color_space",
    "color_primaries",
    "color_transfer",
    "chroma_location",
];

pub struct Video {
    pub path: PathBuf,
    pub stream: Value,
    pub audio: Option<Value>,
    pub samples: Vec<Packet>,
    pub audio_samples: Vec<Packet>,
    pub fps: Rational,
    pub step: i64,
    pub boundaries: Vec<usize>,
    pub(crate) encoder_crf: Option<String>,
}

impl Video {
    pub fn frame(&self, seconds: &str) -> Result<usize> {
        let f = rational(seconds)? * self.fps;
        ensure!(f.is_integer(), "time {seconds} falls between frames");
        Ok(f.to_integer().try_into()?)
    }
}

pub fn inspect(path: &Path) -> Result<Video> {
    inspect_timeline(path, true)
}

/// Callers verify every output packet hash against an already inspected input.
pub(crate) fn inspect_output(path: &Path) -> Result<Video> {
    inspect_timeline(path, false)
}

fn inspect_timeline(path: &Path, inspect_nals: bool) -> Result<Video> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot open {}", path.display()))?;
    let path = canonical.as_path();
    ensure!(
        path.extension()
            .and_then(OsStr::to_str)
            .is_some_and(|x| x.eq_ignore_ascii_case("mp4")),
        "expected .mp4"
    );
    let bytes = run(command("ffprobe")
        .args([
            "-show_streams",
            "-show_packets",
            "-show_chapters",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ])
        .arg(path))?;
    let Probe {
        streams,
        packets,
        chapters,
    } = serde_json::from_slice(&bytes).context("missing or invalid packet timing")?;
    ensure!(chapters.is_empty(), "chapter tracks unsupported");
    let vids: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .collect();
    let auds: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    ensure!(
        vids.len() == 1 && auds.len() <= 1 && streams.len() == 1 + auds.len(),
        "expected one video and at most one audio track"
    );
    let s = vids[0];
    ensure!(
        s["codec_name"] == "h264" && s["codec_tag_string"] == "avc1" && s["nal_length_size"] == "4",
        "requires avc1/H.264 with 4-byte NAL lengths"
    );
    ensure!(
        s["pix_fmt"] == "yuv420p"
            && s["field_order"] == "progressive"
            && s["side_data_list"].is_null()
            && matches!(s["sample_aspect_ratio"].as_str(), None | Some("1:1"))
            && !matches!(
                s["color_transfer"].as_str(),
                Some("smpte2084" | "arib-std-b67")
            ),
        "requires square-pixel progressive SDR yuv420p"
    );
    let fps = rational(s["avg_frame_rate"].as_str().context("missing FPS")?)?;
    let base = rational(s["time_base"].as_str().context("missing timebase")?)?;
    ensure!(
        fps > Ratio::from_integer(0) && base > Ratio::from_integer(0),
        "invalid FPS/timebase"
    );
    let ticks = (fps * base).recip();
    ensure!(ticks.is_integer(), "variable or irregular frame timing");
    let step = i64::try_from(ticks.to_integer())?;
    let (samples, audio_samples): (Vec<_>, Vec<_>) = packets
        .into_iter()
        .partition(|p| s["index"].as_u64() == Some(p.stream_index as u64));
    ensure!(
        !samples.is_empty()
            && s["start_pts"] == 0
            && s["duration_ts"].as_i64() == (samples.len() as i64).checked_mul(step),
        "unsupported edit list or duration"
    );
    let mut order = vec![false; samples.len()];
    let mut reader = BufReader::new(File::open(path)?);
    let len = reader.get_ref().metadata()?.len();
    let mut boundaries = Vec::new();
    let mut encoder_crf = None;
    let mut reader_pos = 0;
    for (i, p) in samples.iter().enumerate() {
        ensure!(
            p.duration == step
                && p.dts == samples[0].dts + i as i64 * step
                && p.pts >= p.dts
                && p.pts >= 0
                && p.pts % step == 0
                && !p.flags.contains(['C', 'D']),
            "irregular CFR/DTS or discarded frame"
        );
        let slot = order
            .get_mut((p.pts / step) as usize)
            .context("PTS outside timeline")?;
        ensure!(!*slot, "duplicate presentation time");
        *slot = true;
        ensure!(
            p.pos.checked_add(p.size).is_some_and(|e| e <= len),
            "invalid sample extent"
        );
        let key = p.flags.contains('K');
        if inspect_nals {
            reader.seek_relative(i64::try_from(p.pos)? - reader_pos)?;
            let mut remain = p.size;
            let mut slice = false;
            while remain > 0 {
                let mut head = [0; 5];
                ensure!(remain >= 5, "truncated AVC sample");
                reader.read_exact(&mut head)?;
                let n = u32::from_be_bytes(head[..4].try_into()?) as u64;
                ensure!(n > 0 && n <= remain - 4, "invalid NAL length");
                match head[4] & 31 {
                    1 if !key => slice = true,
                    5 if key => slice = true,
                    6 | 9 | 12 => (),
                    _ => bail!("non-IDR keyframe, in-band parameter change or unsupported NAL"),
                }
                if i == 0 && head[4] & 31 == 6 && n <= 8192 {
                    // x264's unregistered SEI can carry the original CRF. This is only
                    // an encoder hint: the encoded avcC must still match byte-for-byte.
                    let mut sei = vec![0; n as usize - 1];
                    reader.read_exact(&mut sei)?;
                    let text = String::from_utf8_lossy(&sei);
                    if let Some((_, options)) = text
                        .split_once("x264 - core")
                        .and_then(|(_, s)| s.split_once("options:"))
                    {
                        encoder_crf = options
                            .split_whitespace()
                            .find_map(|w| w.strip_prefix("crf="))
                            .and_then(|s| s.parse::<f32>().ok())
                            .filter(|q| (0.0..=51.0).contains(q))
                            .map(|q| q.to_string());
                    }
                } else {
                    reader.seek_relative(n as i64 - 1)?;
                }
                remain -= n + 4;
            }
            ensure!(slice, "missing video slice");
            reader_pos = i64::try_from(p.pos + p.size)?;
        }
        if key {
            ensure!(p.pts / step == i as i64, "open GOP");
            boundaries.push(i);
        }
    }
    ensure!(
        boundaries.first() == Some(&0),
        "first packet must be an IDR"
    );
    boundaries.push(samples.len());
    for w in boundaries.windows(2) {
        ensure!(
            samples[w[0]..w[1]]
                .iter()
                .all(|p| (w[0]..w[1]).contains(&((p.pts / step) as usize))),
            "GOP has leading or trailing pictures"
        );
    }
    Ok(Video {
        path: canonical,
        stream: s.clone(),
        audio: auds.first().map(|a| (*a).clone()),
        samples,
        audio_samples,
        fps,
        step,
        boundaries,
        encoder_crf,
    })
}
