//! Narrow, non-fragmented MP4 sample-table editor. The original sample entry,
//! audio tables and edit lists survive. Each video sample stays in its original
//! chunk and decode slot. Only its length, composition offset and sync flag may change.
use crate::media::Video;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

fn u32at(b: &[u8], i: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        b.get(i..i + 4).context("short MP4 field")?.try_into()?,
    ))
}
fn atom(kind: &[u8; 4], data: &[u8]) -> Result<Vec<u8>> {
    let size = u32::try_from(data.len() + 8)?;
    let mut out = Vec::with_capacity(size as usize);
    out.extend(size.to_be_bytes());
    out.extend(kind);
    out.extend(data);
    Ok(out)
}
type Atom<'a> = (&'a [u8; 4], &'a [u8]);

fn children(mut bytes: &[u8]) -> Result<Vec<Atom<'_>>> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let n = u32at(bytes, 0)? as usize;
        ensure!(n >= 8 && n <= bytes.len(), "unsupported nested box length");
        result.push((bytes[4..8].try_into()?, &bytes[8..n]));
        bytes = &bytes[n..];
    }
    Ok(result)
}

fn offsets(kind: &[u8; 4], data: &[u8], state: &Tables) -> Result<Vec<u8>> {
    let count = u32at(data, 4)? as usize;
    let width = if kind == b"stco" { 4 } else { 8 };
    ensure!(data.len() == 8 + count * width, "invalid chunk offsets");
    let mut out = data[..8].to_vec();
    for chunk in data[8..].chunks_exact(width) {
        let old = if width == 4 {
            u32at(chunk, 0)? as u64
        } else {
            u64::from_be_bytes(chunk.try_into()?)
        };
        let pos = state
            .chunk_offsets
            .get(&old)
            .context("chunk does not start at a known sample")?;
        out.extend(pos.to_be_bytes());
    }
    atom(b"co64", &out)
}

struct Tables {
    sizes: Vec<u32>,
    ctts: Vec<(u32, u32)>,
    keys: Vec<u32>,
    chunk_offsets: HashMap<u64, u64>,
}

fn full_table(rows: usize, values: impl Iterator<Item = u32>) -> Result<Vec<u8>> {
    let mut bytes = vec![0; 4];
    bytes.extend(u32::try_from(rows)?.to_be_bytes());
    for v in values {
        bytes.extend(v.to_be_bytes());
    }
    Ok(bytes)
}

fn sample_table(bytes: &[u8], state: &Tables, video: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut chunks = 0;
    let mut sizes = 0;
    for (kind, data) in children(bytes)? {
        match kind {
            b"stco" | b"co64" => {
                out.extend(offsets(kind, data, state)?);
                chunks += 1;
            }
            b"stsd" => {
                ensure!(
                    u32at(data, 4)? == 1,
                    "multiple sample descriptions unsupported"
                );
                out.extend(atom(kind, data)?);
            }
            b"stsz" if video => {
                ensure!(
                    u32at(data, 8)? as usize == state.sizes.len(),
                    "sample table length mismatch"
                );
                let mut s = vec![0; 4];
                s.extend(full_table(state.sizes.len(), state.sizes.iter().copied())?);
                out.extend(atom(kind, &s)?);
                sizes += 1;
            }
            b"ctts" | b"stss" if video => (),
            b"stsc" | b"stts" | b"stsz" | b"free" => out.extend(atom(kind, data)?),
            b"sgpd" | b"sbgp" if !video => out.extend(atom(kind, data)?),
            _ => bail!("unsupported sample table {}", String::from_utf8_lossy(kind)),
        }
    }
    ensure!(chunks == 1, "expected one chunk-offset table");
    if video {
        ensure!(sizes == 1, "expected one sample-size table");
        out.extend(atom(
            b"stss",
            &full_table(state.keys.len(), state.keys.iter().copied())?,
        )?);
        let values = state
            .ctts
            .iter()
            .flat_map(|(count, offset)| [*count, *offset]);
        out.extend(atom(b"ctts", &full_table(state.ctts.len(), values)?)?);
    }
    Ok(out)
}

fn rewrite(bytes: &[u8], state: &Tables, video: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (kind, data) in children(bytes)? {
        let body = match kind {
            b"trak" | b"minf" => rewrite(data, state, video)?,
            b"mdia" => {
                let boxes = children(data)?;
                let h = boxes
                    .iter()
                    .find(|(k, _)| *k == b"hdlr")
                    .context("missing media handler")?
                    .1;
                let handler = h.get(8..12).context("invalid media handler")?;
                ensure!(
                    handler == b"vide" || handler == b"soun",
                    "unsupported track handler"
                );
                rewrite(data, state, handler == b"vide")?
            }
            b"stbl" => sample_table(data, state, video)?,
            b"mvex" | b"saio" | b"iloc" => {
                bail!("unsupported fragmented or offset-bearing metadata")
            }
            _ => {
                out.extend(atom(kind, data)?);
                continue;
            }
        };
        out.extend(atom(kind, &body)?);
    }
    Ok(out)
}

struct Layout {
    ftyp: Vec<u8>,
    moov: Vec<u8>,
    start: u64,
    end: u64,
}
fn layout(file: &mut File) -> Result<Layout> {
    let len = file.metadata()?.len();
    let mut layout = Layout {
        ftyp: Vec::new(),
        moov: Vec::new(),
        start: 0,
        end: 0,
    };
    let mut pos = 0;
    while pos < len {
        file.seek(SeekFrom::Start(pos))?;
        let mut h = [0; 8];
        file.read_exact(&mut h)?;
        let mut size = u32at(&h, 0)? as u64;
        let mut header = 8;
        if size == 1 {
            let mut ext = [0; 8];
            file.read_exact(&mut ext)?;
            size = u64::from_be_bytes(ext);
            header = 16;
        }
        if size == 0 {
            size = len - pos;
        }
        ensure!(
            size >= header && size <= len - pos,
            "invalid top-level box extent"
        );
        match &h[4..] {
            b"mdat" => {
                ensure!(layout.start == 0, "multiple mdat boxes unsupported");
                layout.start = pos + header;
                layout.end = pos + size;
            }
            b"moov" | b"ftyp" => {
                ensure!(size < 256 * 1024 * 1024, "metadata too large");
                let slot = if &h[4..] == b"moov" {
                    &mut layout.moov
                } else {
                    &mut layout.ftyp
                };
                ensure!(slot.is_empty(), "duplicate MP4 metadata");
                slot.resize((size - header) as usize, 0);
                file.read_exact(slot)?;
            }
            b"free" | b"skip" | b"wide" => (),
            k => bail!(
                "unsupported top-level MP4 box {}",
                String::from_utf8_lossy(k)
            ),
        }
        pos += size;
    }
    ensure!(
        !layout.moov.is_empty() && layout.ftyp.len() >= 8 && layout.start > 0,
        "missing MP4 metadata"
    );
    ensure!(
        layout
            .ftyp
            .as_chunks::<4>()
            .0
            .iter()
            .any(|b| matches!(b, b"isom" | b"iso2" | b"mp41" | b"mp42" | b"avc1")),
        "unsupported MP4 brand"
    );
    Ok(layout)
}

pub struct Rendered {
    pub start: usize,
    pub end: usize,
    pub video: Video,
}

fn copy_exact(src: &mut impl Read, out: &mut impl Write, len: u64) -> Result<()> {
    let n = io::copy(&mut src.take(len), out)?;
    ensure!(n == len, "short sample read");
    Ok(())
}

/// Returns bytes physically submitted to the MP4 media-data writer (payload only).
pub fn write(source: &Video, parts: &[Rendered], target: &Path) -> Result<u64> {
    let mut file = File::open(&source.path)?;
    let layout = layout(&mut file)?;
    let mut src = BufReader::new(file);
    let mut inputs: Vec<_> = parts
        .iter()
        .map(|p| File::open(&p.video.path).map(BufReader::new))
        .collect::<io::Result<_>>()?;
    let mut changes = HashMap::new();
    for (k, part) in parts.iter().enumerate() {
        for j in 0..part.end - part.start {
            changes.insert(part.start + j, (k, j));
        }
    }
    let mut state = Tables {
        sizes: Vec::new(),
        ctts: Vec::new(),
        keys: Vec::new(),
        chunk_offsets: HashMap::new(),
    };
    let mut ordered = Vec::new();
    for (i, p) in source.samples.iter().enumerate() {
        let (sample, pts) = if let Some(&(k, j)) = changes.get(&i) {
            let part = &parts[k];
            let s = &part.video.samples[j];
            (
                s,
                (part.start as i64 + s.pts / part.video.step) * source.step,
            )
        } else {
            (p, p.pts)
        };
        let composition =
            u32::try_from(pts - p.dts).context("incompatible B-frame decode delay")?;
        if let Some((count, _)) = state.ctts.last_mut().filter(|(_, v)| *v == composition) {
            *count += 1;
        } else {
            state.ctts.push((1, composition));
        }
        if sample.flags.contains('K') {
            state.keys.push(u32::try_from(i + 1)?);
        }
        state.sizes.push(u32::try_from(sample.size)?);
        ordered.push((p.pos, p.size, changes.get(&i).copied()));
    }
    ordered.extend(source.audio_samples.iter().map(|p| (p.pos, p.size, None)));
    ordered.sort_unstable_by_key(|p| p.0);
    let mut out = BufWriter::new(File::create(target)?);
    out.write_all(&atom(b"ftyp", &layout.ftyp)?)?;
    let header = out.stream_position()?;
    out.write_all(&[0, 0, 0, 1, b'm', b'd', b'a', b't', 0, 0, 0, 0, 0, 0, 0, 0])?;
    let mut next = header + 16;
    let mut cursor = layout.start;
    let mut pending = layout.start;
    src.seek(SeekFrom::Start(pending))?;
    for (pos, size, change) in ordered {
        ensure!(
            pos >= cursor && pos.checked_add(size).is_some_and(|n| n <= layout.end),
            "overlap or sample outside mdat"
        );
        next += pos - cursor;
        state.chunk_offsets.insert(pos, next);
        if let Some((k, j)) = change {
            copy_exact(&mut src, &mut out, pos - pending)?;
            let s = &parts[k].video.samples[j];
            inputs[k].seek(SeekFrom::Start(s.pos))?;
            copy_exact(&mut inputs[k], &mut out, s.size)?;
            pending = pos + size;
            src.seek(SeekFrom::Start(pending))?;
            next += s.size;
        } else {
            next += size;
        }
        cursor = pos + size;
    }
    copy_exact(&mut src, &mut out, layout.end - pending)?;
    let mdat_end = out.stream_position()?;
    out.seek(SeekFrom::Start(header + 8))?;
    out.write_all(&(mdat_end - header).to_be_bytes())?;
    out.seek(SeekFrom::Start(mdat_end))?;
    out.write_all(&atom(b"moov", &rewrite(&layout.moov, &state, false)?)?)?;
    out.flush()?;
    Ok(mdat_end - header - 16)
}
