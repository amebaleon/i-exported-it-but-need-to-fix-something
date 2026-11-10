mod common;
use anyhow::Result;
use incremental_export::{
    core::{Edit, patch},
    media::{command, inspect, run},
};
use std::{fs, path::Path, process::Command};
fn encode(path: &Path, rate: &str, frames: &str, opts: &[&str]) -> Result<()> {
    run(command("ffmpeg")
        .args(["-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size=192x108:rate={rate}"))
        .args([
            "-frames:v",
            frames,
            "-c:v",
            "libx264",
            "-g",
            "48",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(opts)
        .arg(path))?;
    Ok(())
}
#[test]
fn fractional_cfr_aligned_copy_without_audio() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let original = dir.path().join("원본.mp4");
    let rep = dir.path().join("replacement.mp4");
    let output = dir.path().join("수정 결과.mp4");
    encode(&original, "30000/1001", "192", &["-bf", "0"])?;
    encode(&rep, "30000/1001", "48", &["-bf", "0", "-vf", "negate"])?;
    let cli = Command::new(env!("CARGO_BIN_EXE_incremental-export"))
        .args(["patch", "--original"])
        .arg(&original)
        .arg("--replacement")
        .arg(&rep)
        .args([
            "--start",
            "0",
            "--end",
            "48048/30000",
            "--edit",
            "96096/30000",
            "144144/30000",
        ])
        .arg(&rep)
        .arg("--output")
        .arg(&output)
        .output()?;
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&cli.stdout)?;
    assert_eq!(report["reencoded_frame_count"], 0);
    assert_eq!(report["copied_replacement_packets"], 96);
    assert_eq!(report["staged_gop_bytes"], 0);
    assert_eq!(report["verified_reused_packets"], 96);
    assert!(inspect(&output)?.audio.is_none());
    Ok(())
}
#[test]
fn replacement_with_different_parameter_sets() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let orig = dir.path().join("orig.mp4");
    let rep = dir.path().join("rep.mp4");
    let out = dir.path().join("out.mp4");
    encode(&orig, "24", "240", &[])?;
    encode(
        &rep,
        "24",
        "72",
        &["-profile:v", "baseline", "-bf", "0", "-vf", "negate"],
    )?;
    let edit = Edit {
        start: "3".into(),
        end: "6".into(),
        replacement: rep.clone(),
    };
    let report = patch(&orig, &[edit], &out)?;
    assert_eq!(report.encoded_frames, [[48, 144]]);
    assert_eq!(report.copied_replacement_packets, 0);
    let got = common::pictures(&out)?;
    let want = common::pictures(&rep)?;
    for (x, y) in got.gray[72 * 576..144 * 576]
        .as_chunks::<576>()
        .0
        .iter()
        .zip(want.gray.as_chunks::<576>().0)
    {
        assert!(
            x.iter()
                .zip(y)
                .map(|(a, b)| (i32::from(*a) - i32::from(*b)).pow(2))
                .sum::<i32>()
                < 12 * 576
        );
    }
    Ok(())
}
#[test]
fn unsupported_vfr_and_encoder_mismatch_leave_no_output() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let orig = dir.path().join("orig.mp4");
    let rep = dir.path().join("rep.mp4");
    let out = dir.path().join("out.mp4");
    let vfr = dir.path().join("vfr.mp4");
    encode(
        &vfr,
        "24",
        "120",
        &["-vf", "select=not(eq(mod(n\\,7)\\,3))", "-fps_mode", "vfr"],
    )?;
    assert!(inspect(&vfr).is_err());
    encode(&orig, "24", "240", &["-refs", "1"])?;
    encode(&rep, "24", "72", &[])?;
    let err = patch(
        &orig,
        &[Edit {
            start: "3".into(),
            end: "6".into(),
            replacement: rep,
        }],
        &out,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("SPS/PPS"), "{err:#}");
    assert!(!out.exists());
    assert!(
        fs::read_dir(dir.path())?.all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.'))
    );
    Ok(())
}
