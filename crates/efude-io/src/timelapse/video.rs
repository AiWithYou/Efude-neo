// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One frame schedule and one layout transform for previews and MP4 export.
use super::{FRAME_RATE, Session, frame_paths};
use image::{Rgb, RgbImage, imageops::FilterType};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Recorded,
    Long720,
    Long1080,
    Portrait,
    Landscape,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    #[default]
    Contain,
    Cover,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Includes the final still; rounded to the nearest 1/30 second.
    pub duration: f64,
    pub hold: f64,
    pub size: Size,
    pub fit: Fit,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            duration: 30.0,
            hold: 2.0,
            size: Size::Recorded,
            fit: Fit::Contain,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub indices: Vec<usize>,
    pub width: u32,
    pub height: u32,
    pub hold_frames: u32,
}
impl Plan {
    pub fn duration(&self) -> f64 {
        self.indices.len() as f64 / FRAME_RATE as f64
    }
}

pub fn plan(session: &Session, source_count: usize, options: &Options) -> Result<Plan, String> {
    if source_count == 0 {
        return Err("記録にフレームがありません".into());
    }
    if !options.duration.is_finite()
        || !(1.0..=600.0).contains(&options.duration)
        || !options.hold.is_finite()
        || options.hold < 0.0
        || options.hold >= options.duration
    {
        return Err("動画は1〜600秒、完成画像の静止は動画の長さ未満にしてください".into());
    }
    let frames = (options.duration * FRAME_RATE as f64).round() as u32;
    let hold = (options.hold * FRAME_RATE as f64).round() as u32;
    let body = frames.saturating_sub(hold);
    if body == 0 || (source_count > 1 && body < 2) {
        return Err("制作過程に少なくとも2フレーム分の時間を残してください".into());
    }
    let (width, height) = match options.size {
        Size::Recorded => (session.width, session.height),
        Size::Portrait => (1080, 1920),
        Size::Landscape => (1920, 1080),
        Size::Square => (1080, 1080),
        Size::Long720 | Size::Long1080 => {
            let limit = if options.size == Size::Long720 {
                720.0
            } else {
                1080.0
            };
            let scale = (limit / session.width.max(session.height) as f64).min(1.0);
            let even = |n: u32| n.max(2) & !1;
            (
                even((session.width as f64 * scale).round() as u32),
                even((session.height as f64 * scale).round() as u32),
            )
        }
    };
    if !(2..=2162).contains(&width)
        || !(2..=2162).contains(&height)
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
    {
        return Err("動画の寸法が不正です".into());
    }
    let mut indices = Vec::with_capacity(frames as usize);
    // Integer rounding keeps both endpoints even when frames are skipped.
    for i in 0..body as u64 {
        let index = if body <= 1 {
            0
        } else {
            ((i * (source_count - 1) as u64 + (body - 1) as u64 / 2) / (body - 1) as u64) as usize
        };
        indices.push(index);
    }
    indices.extend(std::iter::repeat_n(source_count - 1, hold as usize));
    Ok(Plan {
        indices,
        width,
        height,
        hold_frames: hold,
    })
}

pub fn layout(source: &RgbImage, width: u32, height: u32, fit: Fit) -> RgbImage {
    layout_with_intermediate_limit(source, width, height, fit, 16 * 1024 * 1024)
}

fn layout_with_intermediate_limit(
    source: &RgbImage,
    width: u32,
    height: u32,
    fit: Fit,
    intermediate_limit: u64,
) -> RgbImage {
    let sx = width as f64 / source.width() as f64;
    let sy = height as f64 / source.height() as f64;
    let scale = if fit == Fit::Contain {
        sx.min(sy)
    } else {
        sx.max(sy)
    };
    let w = (source.width() as f64 * scale).round().max(1.0) as u32;
    let h = (source.height() as f64 * scale).round().max(1.0) as u32;
    let resized = if fit == Fit::Cover {
        resize_cover_region(
            source,
            w,
            h,
            width.min(w),
            height.min(h),
            intermediate_limit,
        )
    } else {
        image::imageops::resize(source, w, h, FilterType::Triangle)
    };
    let (w, h) = resized.dimensions();
    let mut output = RgbImage::from_pixel(width, height, Rgb([255; 3]));
    let draw_w = width.min(w);
    let draw_h = height.min(h);
    let from_x = w.saturating_sub(width) / 2;
    let from_y = h.saturating_sub(height) / 2;
    let to_x = width.saturating_sub(w) / 2;
    let to_y = height.saturating_sub(h) / 2;
    for y in 0..draw_h {
        for x in 0..draw_w {
            output.put_pixel(
                to_x + x,
                to_y + y,
                *resized.get_pixel(from_x + x, from_y + y),
            );
        }
    }
    output
}

fn resize_cover_region(
    source: &RgbImage,
    width: u32,
    height: u32,
    crop_width: u32,
    crop_height: u32,
    intermediate_limit: u64,
) -> RgbImage {
    if width as u64 * height as u64 <= intermediate_limit
        || source.width() == 0
        || source.height() == 0
        || crop_width == 0
        || crop_height == 0
    {
        return image::imageops::resize(source, width, height, FilterType::Triangle);
    }
    // Keep image::resize's vertical-then-horizontal Triangle sampling and pixel
    // centres, but evaluate only the central crop and its source kernel support.
    // Matches image 0.25.10: https://docs.rs/image/0.25.10/src/image/imageops/sample.rs.html
    let from_x = (width - crop_width) / 2;
    let from_y = (height - crop_height) / 2;
    let columns: Vec<_> = (0..crop_width)
        .map(|x| triangle_weights(from_x + x, source.width(), width))
        .collect();
    let first = columns[0].0;
    let last = columns.last().unwrap();
    let right = last.0 + last.1.len() as u32;
    let mut row = vec![[0.0f32; 3]; (right - first) as usize];
    let mut output = RgbImage::new(crop_width, crop_height);
    for y in 0..crop_height {
        let (top, weights) = triangle_weights(from_y + y, source.height(), height);
        for (x, value) in row.iter_mut().enumerate() {
            *value = [0.0; 3];
            for (i, weight) in weights.iter().enumerate() {
                let pixel = source.get_pixel(first + x as u32, top + i as u32);
                for channel in 0..3 {
                    value[channel] += pixel[channel] as f32 * weight;
                }
            }
        }
        for (x, (left, weights)) in columns.iter().enumerate() {
            let mut value = [0.0f32; 3];
            for (i, weight) in weights.iter().enumerate() {
                let pixel = row[(*left - first) as usize + i];
                for channel in 0..3 {
                    value[channel] += pixel[channel] * weight;
                }
            }
            output.put_pixel(
                x as u32,
                y,
                Rgb(value.map(|v| v.clamp(0.0, 255.0).round() as u8)),
            );
        }
    }
    output
}

fn triangle_weights(position: u32, input_length: u32, output_length: u32) -> (u32, Vec<f32>) {
    let ratio = input_length as f32 / output_length as f32;
    let support = ratio.max(1.0);
    let centre = (position as f32 + 0.5) * ratio;
    let left = (centre - support)
        .floor()
        .clamp(0.0, input_length as f32 - 1.0) as u32;
    let right = (centre + support)
        .ceil()
        .clamp(left as f32 + 1.0, input_length as f32) as u32;
    let centre = centre - 0.5;
    let mut weights: Vec<_> = (left..right)
        .map(|i| (1.0 - ((i as f32 - centre) / support).abs()).max(0.0))
        .collect();
    let total: f32 = weights.iter().sum();
    for weight in &mut weights {
        *weight /= total;
    }
    (left, weights)
}

pub fn read_frame(session: &Session, path: &Path) -> Result<RgbImage, String> {
    if path.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("記録フレームが大きすぎます".into());
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(session.width);
    limits.max_image_height = Some(session.height);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgb8();
    if image.dimensions() != (session.width, session.height) {
        return Err(format!("寸法の異なる記録フレームです: {}", path.display()));
    }
    Ok(image)
}

#[derive(Clone, Default)]
pub struct Control {
    pub cancel: Arc<AtomicBool>,
    pub frames_done: Arc<AtomicU32>,
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn stop_process(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    #[cfg(windows)]
    {
        // Some package managers start FFmpeg through a shim. Terminate that tree,
        // including the process which owns the pipe handles, before joining readers.
        let mut cmd = Command::new("taskkill");
        hidden(&mut cmd);
        let _ = cmd
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

pub fn ffmpeg_available(executable: &Path) -> bool {
    let mut cmd = Command::new(executable);
    hidden(&mut cmd);
    cmd.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

struct Encoder {
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
    watcher: Option<JoinHandle<()>>,
    errors: Option<JoinHandle<Vec<u8>>>,
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock() {
            stop_process(&mut child);
            let _ = child.wait();
        }
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        if let Some(errors) = self.errors.take() {
            let _ = errors.join();
        }
    }
}

/// Frame paths are frozen before encoding, so later captures never enter this export.
pub fn export_mp4(
    session: &Session,
    path: &Path,
    options: &Options,
    executable: &Path,
    control: &Control,
) -> Result<u32, String> {
    let paths = frame_paths(session).map_err(|e| e.to_string())?;
    let plan = plan(session, paths.len(), options)?;
    export_mp4_frames(session, &paths, path, options, &plan, executable, control)
}

pub fn export_mp4_frames(
    session: &Session,
    paths: &[PathBuf],
    path: &Path,
    options: &Options,
    plan: &Plan,
    executable: &Path,
    control: &Control,
) -> Result<u32, String> {
    if *plan != self::plan(session, paths.len(), options)? {
        return Err("書き出し計画が変わりました".into());
    }
    if paths.is_empty() || plan.indices.iter().any(|i| *i >= paths.len()) {
        return Err("書き出し対象のフレームが不正です".into());
    }
    if control.cancel.load(Ordering::Relaxed) {
        return Err("書き出しを中止しました".into());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::Builder::new()
        .prefix(".efude-video-")
        .suffix(".mp4")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?
        .into_temp_path();
    let mut cmd = Command::new(executable);
    hidden(&mut cmd);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgb24",
        "-video_size",
    ])
    .arg(format!("{}x{}", plan.width, plan.height))
    .arg("-framerate")
    .arg(FRAME_RATE.to_string())
    .args([
        "-i",
        "pipe:0",
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "20",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
        "-f",
        "mp4",
    ])
    .args([
        "-vf",
        "scale=out_color_matrix=bt709",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
    ])
    .arg(&temp)
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        format!("FFmpegを起動できません（書き出し画面で実行ファイルを指定できます）: {e}")
    })?;
    let mut stdin = child.stdin.take().ok_or("FFmpegの入力を開けません")?;
    let mut stderr = child.stderr.take().ok_or("FFmpegのログを開けません")?;
    let errors = std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut buffer = [0u8; 2048];
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buffer[..n]);
            if tail.len() > 16_384 {
                tail.drain(..tail.len() - 16_384);
            }
        }
        tail
    });
    let shared = Arc::new(Mutex::new(child));
    let stopped = Arc::new(AtomicBool::new(false));
    let watch_child = shared.clone();
    let watch_stop = stopped.clone();
    let cancel = control.cancel.clone();
    let watcher = std::thread::spawn(move || {
        while !watch_stop.load(Ordering::Relaxed) {
            if cancel.load(Ordering::Relaxed) {
                if let Ok(mut child) = watch_child.lock() {
                    stop_process(&mut child);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    });
    let mut encoder = Encoder {
        child: shared,
        stopped,
        watcher: Some(watcher),
        errors: Some(errors),
    };
    let mut cached: Option<(usize, RgbImage)> = None;
    let mut pipe_error = None;
    for (frame, &index) in plan.indices.iter().enumerate() {
        if control.cancel.load(Ordering::Relaxed) {
            return Err("書き出しを中止しました".into());
        }
        if cached.as_ref().is_none_or(|(last, _)| *last != index) {
            let source = read_frame(session, &paths[index])?;
            cached = Some((index, layout(&source, plan.width, plan.height, options.fit)));
        }
        if let Err(error) = stdin.write_all(cached.as_ref().unwrap().1.as_raw()) {
            pipe_error = Some(error.to_string());
            break;
        }
        control
            .frames_done
            .store((frame + 1) as u32, Ordering::Relaxed);
    }
    drop(stdin);
    // Do not hold the child mutex while waiting: the cancellation watcher must be able to kill it.
    let status = loop {
        if let Some(status) = encoder
            .child
            .lock()
            .map_err(|e| e.to_string())?
            .try_wait()
            .map_err(|e| e.to_string())?
        {
            break status;
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    encoder.stopped.store(true, Ordering::Relaxed);
    let errors = encoder.errors.take().unwrap().join().unwrap_or_default();
    if control.cancel.load(Ordering::Relaxed) {
        return Err("書き出しを中止しました".into());
    }
    if !status.success() || pipe_error.is_some() {
        return Err(format!(
            "MP4を書き出せません: {} {}",
            String::from_utf8_lossy(&errors).trim(),
            pipe_error.unwrap_or_default()
        ));
    }
    if temp.metadata().map_err(|e| e.to_string())?.len() == 0 {
        return Err("動画が空です".into());
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(plan.indices.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        Session {
            folder: PathBuf::new(),
            width: 64,
            height: 32,
            include_guide: false,
        }
    }

    #[test]
    fn schedule_preserves_endpoints_and_total_duration_including_hold() {
        for count in [1, 2, 7, 500] {
            let plan = plan(
                &session(),
                count,
                &Options {
                    duration: 2.0,
                    hold: 0.5,
                    ..Options::default()
                },
            )
            .unwrap();
            assert_eq!(plan.indices.len(), 60);
            assert_eq!(plan.duration(), 2.0);
            assert_eq!(plan.indices[0], 0);
            assert_eq!(plan.indices[44], count - 1);
            assert!(plan.indices[45..].iter().all(|i| *i == count - 1));
            assert!(plan.indices.windows(2).all(|w| w[0] <= w[1]));
        }
        assert!(
            plan(
                &session(),
                2,
                &Options {
                    duration: 1.0,
                    hold: 0.99,
                    ..Options::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn layout_shares_letterbox_and_crop_geometry() {
        let image = RgbImage::from_pixel(8, 4, Rgb([20, 40, 80]));
        let contain = layout(&image, 8, 8, Fit::Contain);
        assert_eq!(contain.get_pixel(4, 0).0, [255; 3]);
        assert_eq!(contain.get_pixel(4, 4).0, [20, 40, 80]);
        let cover = layout(&image, 8, 8, Fit::Cover);
        assert!(cover.pixels().all(|p| p.0 == [20, 40, 80]));
    }

    #[test]
    fn cover_resizes_only_the_needed_region_when_intermediate_is_large() {
        let source = RgbImage::from_pixel(64, 2, Rgb([20, 40, 80]));
        let resized = resize_cover_region(&source, 256, 8, 8, 8, 64);
        assert_eq!(resized.dimensions(), (8, 8));
        assert_eq!(resized.as_raw().len(), 8 * 8 * 3);
    }

    #[test]
    fn bounded_cover_matches_full_triangle_resize() {
        for (source_width, source_height) in [(64, 32), (7, 3), (3, 7), (1, 8), (8, 1), (8, 8)] {
            let source = RgbImage::from_fn(source_width, source_height, |x, y| {
                Rgb([
                    (x * 37 + y * 61) as u8,
                    (x * 73 + y * 19) as u8,
                    (x * 11 + y * 29) as u8,
                ])
            });
            for (width, height) in [(8, 8), (11, 5), (6, 13), (32, 32)] {
                assert_eq!(
                    layout_with_intermediate_limit(&source, width, height, Fit::Cover, 0),
                    layout(&source, width, height, Fit::Cover),
                    "{source_width}x{source_height} to {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn thin_recording_can_use_the_portrait_cover_preset() {
        let directory = tempfile::tempdir().unwrap();
        let document = efude_canvas::Document::new(2160, 2);
        let session =
            super::super::create_session(directory.path(), &document, false, 2160).unwrap();
        super::super::write_frame(&session, 1, &document).unwrap();
        let options = Options {
            duration: 1.0,
            hold: 0.0,
            size: Size::Portrait,
            fit: Fit::Cover,
        };
        let plan = plan(&session, 1, &options).unwrap();
        let frame = read_frame(&session, &session.folder.join("frame_00000001.jpg")).unwrap();
        let output = layout(&frame, plan.width, plan.height, options.fit);
        assert_eq!(output.dimensions(), (1080, 1920));
        assert_eq!(output.as_raw().len(), 1080 * 1920 * 3);
        assert!(
            output
                .pixels()
                .all(|pixel| pixel.0.iter().all(|v| *v > 245))
        );
    }

    #[test]
    fn actual_mp4_has_requested_time_and_cancel_preserves_existing_output() {
        let executable = Path::new("ffmpeg");
        if !ffmpeg_available(executable) {
            assert!(
                std::env::var_os("EFUDE_REQUIRE_FFMPEG").is_none(),
                "FFmpeg is required for this verification"
            );
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let mut document = efude_canvas::Document::new(64, 32);
        let session =
            super::super::create_session(directory.path(), &document, false, 720).unwrap();
        super::super::write_frame(&session, 1, &document).unwrap();
        for y in 0..32 {
            for x in 0..64 {
                document.layers[0]
                    .pixels
                    .set_pixel(x, y, [230, 10, 20, 255]);
            }
        }
        super::super::write_frame(&session, 2, &document).unwrap();
        let options = Options {
            duration: 1.0,
            hold: 0.5,
            ..Options::default()
        };
        let path = directory.path().join("result.mp4");
        assert_eq!(
            export_mp4(&session, &path, &options, executable, &Control::default()).unwrap(),
            30
        );
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,pix_fmt,width,height,nb_frames,duration",
                "-of",
                "json",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let stream = &info["streams"][0];
        assert_eq!(stream["codec_name"], "h264");
        assert_eq!(stream["pix_fmt"], "yuv420p");
        assert_eq!(stream["nb_frames"], "30");
        assert_eq!(stream["duration"], "1.000000");
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        let frame_bytes = (session.width * session.height * 3) as usize;
        assert!(decoded.stdout[..3].iter().all(|v| *v > 240));
        let last = &decoded.stdout[29 * frame_bytes..29 * frame_bytes + 3];
        assert!(last[0] > 200 && last[1] < 40 && last[2] < 50, "{last:?}");
        let before = std::fs::read(&path).unwrap();
        let control = Control::default();
        control.cancel.store(true, Ordering::Relaxed);
        assert!(export_mp4(&session, &path, &options, executable, &control).is_err());
        assert_eq!(before, std::fs::read(&path).unwrap());
    }
}
