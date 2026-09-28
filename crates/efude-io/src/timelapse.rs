// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Document-derived timelapse frames. The editor is never captured from the screen.
use efude_canvas::{Document, composite, composite_with_guide};
use image::{ColorType, ImageBuffer, Rgba, codecs::jpeg::JpegEncoder, imageops::FilterType};
use std::{
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const FRAME_RATE: u32 = 30;
pub mod video;
const MAX_AVI_BYTES: u64 = 1_900_000_000;

#[derive(Clone, Debug)]
pub struct Session {
    pub folder: PathBuf,
    pub width: u32,
    pub height: u32,
    pub include_guide: bool,
}

pub fn default_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Efude-neo")
        .join("timelapse")
}

pub fn create_session(
    root: &Path,
    doc: &Document,
    include_guide: bool,
    max_side: u32,
) -> Result<Session, Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height)
        || !(240..=2160).contains(&max_side)
    {
        return Err("invalid timelapse dimensions".into());
    }
    fs::create_dir_all(root)?;
    let scale = (max_side as f64 / doc.width.max(doc.height) as f64).min(1.0);
    let even = |n: u32| (n.max(2) + 1) & !1;
    let width = even((doc.width as f64 * scale).round() as u32);
    let height = even((doc.height as f64 * scale).round() as u32);
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut folder = None;
    for attempt in 0..100u32 {
        let candidate = root.join(format!("{stamp}-{}-{attempt}", std::process::id()));
        if fs::create_dir(&candidate).is_ok() {
            folder = Some(candidate);
            break;
        }
    }
    let folder = folder.ok_or("could not create a unique timelapse folder")?;
    let manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "version": 1, "width": width, "height": height,
        "include_guide": include_guide, "frame_rate": FRAME_RATE
    }))?;
    let mut file = File::create(folder.join("session.json"))?;
    file.write_all(&manifest)?;
    file.sync_all()?;
    Ok(Session {
        folder,
        width,
        height,
        include_guide,
    })
}

pub fn open_session(folder: &Path) -> Result<Session, Box<dyn std::error::Error>> {
    let bytes = fs::read(folder.join("session.json"))?;
    if bytes.len() > 64 * 1024 {
        return Err("timelapse manifest is too large".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value["version"].as_u64() != Some(1)
        || value["frame_rate"].as_u64() != Some(FRAME_RATE as u64)
    {
        return Err("unsupported timelapse manifest".into());
    }
    let width = u32::try_from(value["width"].as_u64().ok_or("missing width")?)?;
    let height = u32::try_from(value["height"].as_u64().ok_or("missing height")?)?;
    if width < 2
        || height < 2
        || width > 2162
        || height > 2162
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
    {
        return Err("invalid timelapse frame size".into());
    }
    Ok(Session {
        folder: folder.to_path_buf(),
        width,
        height,
        include_guide: value["include_guide"]
            .as_bool()
            .ok_or("missing guide mode")?,
    })
}

pub fn frame_paths(session: &Session) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths = Vec::new();
    for item in fs::read_dir(&session.folder)? {
        let path = item?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.len() == 18
            && name.starts_with("frame_")
            && name.ends_with(".jpg")
            && name[6..14].bytes().all(|byte| byte.is_ascii_digit())
        {
            paths.push(path);
        }
    }
    paths.sort();
    for (index, path) in paths.iter().enumerate() {
        if path.file_name().and_then(|name| name.to_str())
            != Some(format!("frame_{:08}.jpg", index + 1).as_str())
        {
            return Err("記録フレームに欠落があります。記録フォルダーを確認してください".into());
        }
    }
    Ok(paths)
}

/// The `include_guide` choice is frozen in `Session`. OFF uses precisely the
/// guide-free artwork compositor used by normal merged-image output.
pub fn write_frame(
    session: &Session,
    index: u32,
    doc: &Document,
) -> Result<usize, Box<dyn std::error::Error>> {
    if index == 0 || !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err("invalid timelapse frame".into());
    }
    let pixels = if session.include_guide {
        composite_with_guide(doc)
    } else {
        composite(doc)
    };
    let source = ImageBuffer::<Rgba<u8>, _>::from_raw(doc.width, doc.height, pixels)
        .ok_or("invalid frame pixels")?;
    let scale =
        (session.width as f64 / doc.width as f64).min(session.height as f64 / doc.height as f64);
    let draw_w = ((doc.width as f64 * scale).round() as u32).clamp(1, session.width);
    let draw_h = ((doc.height as f64 * scale).round() as u32).clamp(1, session.height);
    let resized = image::imageops::resize(&source, draw_w, draw_h, FilterType::Triangle);
    let mut rgb =
        image::RgbImage::from_pixel(session.width, session.height, image::Rgb([255, 255, 255]));
    let offset_x = (session.width - draw_w) / 2;
    let offset_y = (session.height - draw_h) / 2;
    for (x, y, pixel) in resized.enumerate_pixels() {
        rgb.put_pixel(
            offset_x + x,
            offset_y + y,
            image::Rgb([pixel[0], pixel[1], pixel[2]]),
        );
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 85).encode(
        rgb.as_raw(),
        session.width,
        session.height,
        ColorType::Rgb8.into(),
    )?;
    let target = session.folder.join(format!("frame_{index:08}.jpg"));
    let temp = session.folder.join(format!("frame_{index:08}.tmp"));
    let mut file = File::create_new(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, target)?;
    Ok(bytes.len())
}

fn fourcc(out: &mut File, code: &[u8; 4]) -> std::io::Result<()> {
    out.write_all(code)
}
fn u32le(out: &mut File, value: u32) -> std::io::Result<()> {
    out.write_all(&value.to_le_bytes())
}
fn begin_chunk(out: &mut File, kind: &[u8; 4]) -> std::io::Result<u64> {
    let start = out.stream_position()?;
    fourcc(out, kind)?;
    u32le(out, 0)?;
    Ok(start)
}
fn end_chunk(out: &mut File, start: u64) -> std::io::Result<()> {
    let end = out.stream_position()?;
    let size = u32::try_from(end - start - 8).map_err(std::io::Error::other)?;
    out.seek(SeekFrom::Start(start + 4))?;
    u32le(out, size)?;
    out.seek(SeekFrom::Start(end))?;
    if size % 2 != 0 {
        out.write_all(&[0])?;
    }
    Ok(())
}
fn begin_list(out: &mut File, name: &[u8; 4]) -> std::io::Result<u64> {
    let start = begin_chunk(out, b"LIST")?;
    fourcc(out, name)?;
    Ok(start)
}

/// Exports a standards-shaped AVI 1.0 MJPEG file. The source JPEG folder is
/// kept so an interrupted export can be retried without losing the recording.
pub fn export_avi(session: &Session, path: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    let frames = frame_paths(session)?;
    export_avi_frames(session, path, &frames, None, None)
}

pub fn export_avi_frames(
    session: &Session,
    path: &Path,
    frames: &[PathBuf],
    cancel: Option<&std::sync::atomic::AtomicBool>,
    progress: Option<&std::sync::atomic::AtomicU32>,
) -> Result<u32, Box<dyn std::error::Error>> {
    let cancelled = || cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed));
    if frames.is_empty() {
        return Err("timelapse contains no frames".into());
    }
    let mut sizes = Vec::with_capacity(frames.len());
    let mut expected_bytes = 512u64 + frames.len() as u64 * 24;
    for frame in frames {
        if cancelled() {
            return Err("書き出しを中止しました".into());
        }
        let size = frame.metadata()?.len();
        if !(4..=64 * 1024 * 1024).contains(&size) {
            return Err("invalid timelapse JPEG size".into());
        }
        expected_bytes = expected_bytes.saturating_add(size + size % 2);
        sizes.push(size as u32);
    }
    if expected_bytes > MAX_AVI_BYTES || frames.len() > u32::MAX as usize / 16 {
        return Err("AVI would exceed the 1.9 GB compatibility limit; use the JPEG frames or export a shorter recording".into());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let mut out = temp.reopen()?;
    let riff = begin_chunk(&mut out, b"RIFF")?;
    fourcc(&mut out, b"AVI ")?;
    let hdrl = begin_list(&mut out, b"hdrl")?;
    let avih = begin_chunk(&mut out, b"avih")?;
    for value in [
        1_000_000 / FRAME_RATE,
        sizes
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_mul(FRAME_RATE),
        0,
        0x10,
        frames.len() as u32,
        0,
        1,
        sizes.iter().copied().max().unwrap_or(0),
        session.width,
        session.height,
        0,
        0,
        0,
        0,
    ] {
        u32le(&mut out, value)?;
    }
    end_chunk(&mut out, avih)?;
    let strl = begin_list(&mut out, b"strl")?;
    let strh = begin_chunk(&mut out, b"strh")?;
    fourcc(&mut out, b"vids")?;
    fourcc(&mut out, b"MJPG")?;
    u32le(&mut out, 0)?;
    out.write_all(&0u16.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?;
    for value in [
        0,
        1,
        FRAME_RATE,
        0,
        frames.len() as u32,
        sizes.iter().copied().max().unwrap_or(0),
        u32::MAX,
        0,
    ] {
        u32le(&mut out, value)?;
    }
    for value in [0i16, 0, session.width as i16, session.height as i16] {
        out.write_all(&value.to_le_bytes())?;
    }
    end_chunk(&mut out, strh)?;
    let strf = begin_chunk(&mut out, b"strf")?;
    u32le(&mut out, 40)?;
    u32le(&mut out, session.width)?;
    u32le(&mut out, session.height)?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&24u16.to_le_bytes())?;
    fourcc(&mut out, b"MJPG")?;
    for value in [sizes.iter().copied().max().unwrap_or(0), 0, 0, 0, 0] {
        u32le(&mut out, value)?;
    }
    end_chunk(&mut out, strf)?;
    end_chunk(&mut out, strl)?;
    end_chunk(&mut out, hdrl)?;
    let movi = begin_list(&mut out, b"movi")?;
    let mut index = Vec::with_capacity(frames.len());
    for (frame_index, (path, size)) in frames.iter().zip(sizes).enumerate() {
        if cancelled() {
            return Err("書き出しを中止しました".into());
        }
        let chunk_start = out.stream_position()?;
        let bytes = fs::read(path)?;
        if bytes.len() != size as usize
            || !bytes.starts_with(&[0xff, 0xd8])
            || !bytes.ends_with(&[0xff, 0xd9])
        {
            return Err(format!("invalid or incomplete JPEG frame: {}", path.display()).into());
        }
        video::read_frame(session, path)?;
        let offset =
            u32::try_from(chunk_start - movi - 8).map_err(|_| "AVI index offset overflow")?;
        let chunk = begin_chunk(&mut out, b"00dc")?;
        out.write_all(&bytes)?;
        end_chunk(&mut out, chunk)?;
        index.push((offset, size));
        if let Some(progress) = progress {
            progress.store(
                (frame_index + 1) as u32,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
    }
    end_chunk(&mut out, movi)?;
    let idx1 = begin_chunk(&mut out, b"idx1")?;
    for (offset, size) in index {
        fourcc(&mut out, b"00dc")?;
        u32le(&mut out, 0x10)?;
        u32le(&mut out, offset)?;
        u32le(&mut out, size)?;
    }
    end_chunk(&mut out, idx1)?;
    end_chunk(&mut out, riff)?;
    out.sync_all()?;
    drop(out);
    if cancelled() {
        return Err("書き出しを中止しました".into());
    }
    temp.persist(path)?;
    Ok(frames.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_setting_applies_to_every_document_derived_frame() {
        let directory = tempfile::tempdir().unwrap();
        let mut doc = Document::new(64, 64);
        doc.guide = efude_canvas::GuideImage::fit_to_canvas(
            64,
            64,
            [255, 0, 255, 255].repeat(64 * 64),
            &doc,
        );
        let without = create_session(directory.path(), &doc, false, 720).unwrap();
        let with = create_session(directory.path(), &doc, true, 720).unwrap();
        write_frame(&without, 1, &doc).unwrap();
        write_frame(&with, 1, &doc).unwrap();
        let pixel = |session: &Session| {
            image::open(frame_paths(session).unwrap().remove(0))
                .unwrap()
                .to_rgb8()
                .get_pixel(32, 32)
                .0
        };
        let plain = pixel(&without);
        let guided = pixel(&with);
        assert!(
            plain.iter().all(|channel| *channel > 245),
            "guide leaked: {plain:?}"
        );
        assert!(
            guided[1] < 200 && guided[0] > 245 && guided[2] > 245,
            "guide missing: {guided:?}"
        );
        doc.layers[0].pixels.set_pixel(32, 32, [0, 0, 0, 255]);
        write_frame(&without, 2, &doc).unwrap();
        let avi = directory.path().join("recording.avi");
        assert_eq!(export_avi(&without, &avi).unwrap(), 2);
        let bytes = fs::read(avi).unwrap();
        assert!(bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI ");
        assert!(bytes.windows(4).any(|window| window == b"idx1"));
        if let Ok(output) = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,width,height,nb_frames",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(directory.path().join("recording.avi"))
            .output()
        {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let info = String::from_utf8_lossy(&output.stdout);
            assert!(
                info.contains("codec_name=mjpeg") && info.contains("nb_frames=2"),
                "{info}"
            );
        }
    }
}
