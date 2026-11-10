#![allow(dead_code)]
use anyhow::Result;
use incremental_export::media::{command, run};
use std::path::{Path, PathBuf};

pub fn fixture(root: &Path, size: &str) -> Result<(PathBuf, PathBuf)> {
    let orig = root.join("첫 내보내기.mp4");
    let rep = root.join("수정 구간.mp4");
    for (path, sec, full) in [(&orig, 30, true), (&rep, 5, false)] {
        let mut c = command("ffmpeg");
        c.args(["-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size={size}:rate=24:duration={sec}"));
        if full {
            c.args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=997:sample_rate=48000:duration=30",
                "-c:a",
                "aac",
            ]);
        } else {
            c.args(["-vf", "negate"]);
        }
        run(c
            .args([
                "-c:v",
                "libx264",
                "-crf",
                "18",
                "-preset",
                "medium",
                "-pix_fmt",
                "yuv420p",
                "-x264-params",
                "keyint=48:min-keyint=48:scenecut=0",
            ])
            .arg(path))?;
    }
    Ok((orig, rep))
}
pub struct Pictures {
    pub hashes: Vec<String>,
    pub gray: Vec<u8>,
}
pub fn pictures(path: &Path) -> Result<Pictures> {
    let md5 = run(command("ffmpeg")
        .arg("-i")
        .arg(path)
        .args(["-map", "0:v", "-f", "framemd5", "-"]))?;
    let hashes = String::from_utf8(md5)?
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| l.rsplit(',').next().unwrap().trim().to_string())
        .collect();
    let gray = run(command("ffmpeg").arg("-i").arg(path).args([
        "-map",
        "0:v",
        "-vf",
        "scale=32:18",
        "-pix_fmt",
        "gray",
        "-f",
        "rawvideo",
        "-",
    ]))?;
    Ok(Pictures { hashes, gray })
}
pub fn pcm(path: &Path) -> Result<Vec<u8>> {
    run(command("ffmpeg").arg("-i").arg(path).args([
        "-map",
        "0:a:0",
        "-c:a",
        "pcm_s16le",
        "-f",
        "s16le",
        "-",
    ]))
}
