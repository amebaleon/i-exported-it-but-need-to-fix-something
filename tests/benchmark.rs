mod common;
use anyhow::Result;
use incremental_export::{
    core::{self, Edit},
    media::{command, inspect, run},
};
use serde_json::{Value, json};
use std::{path::Path, time::Instant};

#[cfg(windows)]
mod accounting {
    use anyhow::Result;
    use std::{
        io,
        mem::{size_of, zeroed},
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{JobObjects::*, Threading::GetCurrentProcess},
    };
    pub struct Meter(HANDLE);
    impl Meter {
        pub fn new() -> Result<Self> {
            // A private job accounts for this benchmark and every FFmpeg/FFprobe child.
            unsafe {
                let h = CreateJobObjectW(null(), null());
                if h.is_null() {
                    return Err(io::Error::last_os_error().into());
                }
                let job = Self(h);
                if AssignProcessToJobObject(h, GetCurrentProcess()) == 0 {
                    return Err(io::Error::last_os_error().into());
                }
                Ok(job)
            }
        }
        pub fn read(&self) -> Result<[u64; 2]> {
            unsafe {
                let mut info: JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION = zeroed();
                if QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAndIoAccountingInformation,
                    (&raw mut info).cast(),
                    size_of::<JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                ) == 0
                {
                    return Err(io::Error::last_os_error().into());
                }
                Ok([
                    info.IoInfo.ReadTransferCount,
                    info.IoInfo.WriteTransferCount,
                ])
            }
        }
    }
    impl Drop for Meter {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
#[cfg(not(windows))]
mod accounting {
    pub struct Meter;
    impl Meter {
        pub fn new() -> anyhow::Result<Self> {
            Ok(Self)
        }
        pub fn read(&self) -> anyhow::Result<[u64; 2]> {
            Ok([0, 0])
        }
    }
}

fn measure(job: &accounting::Meter, f: impl FnOnce() -> Result<Value>) -> Result<Value> {
    let before = job.read()?;
    let now = Instant::now();
    let detail = f()?;
    let elapsed = now.elapsed().as_millis();
    let after = job.read()?;
    Ok(
        json!({"ms":elapsed,"detail":detail,"windows_job_read_bytes":cfg!(windows).then(||after[0]-before[0]),
        "windows_job_write_bytes":cfg!(windows).then(||after[1]-before[1])}),
    )
}
fn full(original: &Path, rep: &Path, out: &Path) -> Result<Value> {
    run(command("ffmpeg").arg("-i").arg(original).arg("-i").arg(rep).args(["-filter_complex",
        "[1:v]setpts=PTS+11/TB[p];[0:v][p]overlay=eof_action=pass:repeatlast=0:enable='gte(t,11)*lt(t,16)'[v]",
        "-map","[v]","-map","0:a:0","-c:a","copy","-c:v","libx264","-preset","medium","-crf","18","-pix_fmt","yuv420p"]).arg(out))?;
    Ok(json!({"output_bytes":out.metadata()?.len()}))
}
#[test]
#[ignore = "three timed runs of both encoders; run separately"]
fn compare_exports() -> Result<()> {
    let meter = accounting::Meter::new()?;
    let dir = tempfile::tempdir()?;
    let (orig, rep) = common::fixture(dir.path(), "1280x720")?;
    for i in 0..3 {
        let patched = dir.path().join(format!("patch{i}.mp4"));
        let full_file = dir.path().join(format!("full{i}.mp4"));
        let partial = || {
            measure(&meter, || {
                Ok(serde_json::to_value(core::patch(
                    &orig,
                    &[Edit {
                        start: "11".into(),
                        end: "16".into(),
                        replacement: rep.clone(),
                    }],
                    &patched,
                )?)?)
            })
        };
        let baseline = || measure(&meter, || full(&orig, &rep, &full_file));
        let (patch, full) = if i % 2 == 0 {
            (partial()?, baseline()?)
        } else {
            let f = baseline()?;
            (partial()?, f)
        };
        assert_eq!(inspect(&full_file)?.samples.len(), 720);
        // Independent comparison of benchmark content, including both splice boundaries.
        let a = common::pictures(&patched)?;
        let b = common::pictures(&full_file)?;
        assert_eq!(a.gray.len(), b.gray.len());
        for (x, y) in a
            .gray
            .as_chunks::<576>()
            .0
            .iter()
            .zip(b.gray.as_chunks::<576>().0)
        {
            let mse = x
                .iter()
                .zip(y)
                .map(|(u, v)| (i32::from(*u) - i32::from(*v)).pow(2))
                .sum::<i32>() as f64
                / 576.;
            assert!(mse < 12., "benchmark pictures differ: {mse}");
        }
        println!("{}", json!({"run":i+1,"patch":patch,"full":full}));
    }
    Ok(())
}
