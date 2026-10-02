// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Timelapse orchestration, separate from the document save worker.
use super::*;
use efude_io::timelapse::video::{self, Control, Fit, Options, Size};
use efude_io::timelapse::{self, Session};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(crate) enum Task {
    Frame {
        document_id: u64,
        index: u32,
        document: Document,
        session: Session,
        repaint: egui::Context,
    },
    Export {
        session: Session,
        path: PathBuf,
        required_index: u32,
        options: Options,
        ffmpeg: PathBuf,
        avi: bool,
        control: Control,
        repaint: egui::Context,
    },
    Shutdown,
}

pub(crate) enum Completion {
    Frame {
        document_id: u64,
        session_folder: PathBuf,
        index: u32,
    },
    Exported {
        path: PathBuf,
        count: u32,
    },
    ExportFailed(String),
    Failed {
        document_id: u64,
        session_folder: PathBuf,
        message: String,
    },
}

pub(crate) struct Worker {
    pub sender: SyncSender<Task>,
    pub receiver: Receiver<Completion>,
    pub join: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn new() -> Self {
        let (sender, input) = mpsc::sync_channel(2);
        let (output, receiver) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("efude-timelapse-worker".into())
            .spawn(move || {
                let mut failed_sessions = std::collections::HashSet::new();
                let mut exports: Vec<(Control, JoinHandle<()>)> = Vec::new();
                while let Ok(task) = input.recv() {
                    let (completion, repaint) = match task {
                        Task::Frame {
                            document_id,
                            index,
                            document,
                            session,
                            repaint,
                        } => {
                            let result = timelapse::write_frame(&session, index, &document);
                            (
                                match result {
                                    Ok(_) => Completion::Frame { document_id, session_folder: session.folder, index },
                                    Err(error) => {
                                        failed_sessions.insert(session.folder.clone());
                                        Completion::Failed { document_id, session_folder: session.folder, message: format!("タイムラプスの保存に失敗: {error}") }
                                    },
                                },
                                repaint,
                            )
                        }
                        Task::Export {
                            session,
                            path,
                            required_index,
                            options,
                            ffmpeg,
                            avi,
                            control,
                            repaint,
                        } => {
                            // This queue entry is a barrier after the last captured frame.
                            // Freeze paths here, then free the capture worker for other documents.
                            let frames = timelapse::frame_paths(&session).map_err(|e| e.to_string());
                            let error = if failed_sessions.contains(&session.folder) {
                                Some("保存できなかったフレームがあります。記録フォルダーを確認してください".into())
                            } else if required_index > 0 && frames.as_ref().is_ok_and(|f| f.len() != required_index as usize) {
                                Some("最後の編集まで記録できていません。動画を書き出しませんでした".into())
                            } else { frames.as_ref().err().cloned() };
                            if let Some(error) = error {
                                (Completion::ExportFailed(error), repaint)
                            } else {
                                let frames = frames.unwrap();
                                let output = output.clone();
                                let cancel = control.clone();
                                let join = std::thread::spawn(move || {
                                    let result = if avi {
                                        timelapse::export_avi_frames(&session, &path, &frames, Some(&control.cancel), Some(&control.frames_done)).map_err(|e| e.to_string())
                                    } else {
                                        video::plan(&session, frames.len(), &options).and_then(|plan| {
                                            video::export_mp4_frames(&session, &frames, &path, &options, &plan, &ffmpeg, &control)
                                        })
                                    };
                                    let completion = match result {
                                        Ok(count) => Completion::Exported { path, count },
                                        Err(error) => Completion::ExportFailed(error),
                                    };
                                    let _ = output.send(completion);
                                    repaint.request_repaint();
                                });
                                exports.push((cancel, join));
                                continue;
                            }
                        }
                        Task::Shutdown => break,
                    };
                    let _ = output.send(completion);
                    repaint.request_repaint();
                    let mut i = 0;
                    while i < exports.len() {
                        if exports[i].1.is_finished() { let (_, join) = exports.swap_remove(i); let _ = join.join(); }
                        else { i += 1; }
                    }
                }
                for (control, join) in exports {
                    control.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    let _ = join.join();
                }
            })
            .expect("could not start timelapse worker");
        Self {
            sender,
            receiver,
            join: Some(join),
        }
    }
}

pub(crate) struct Recording {
    pub session: Session,
    pub next_index: u32,
    pub frames_written: u32,
    pub last_content_revision: u64,
    pub last_capture: Instant,
    pub recording: bool,
    pub paused: bool,
}

pub(crate) struct ExportDialog {
    session: Session,
    required_index: u32,
    name: String,
    options: Options,
    avi: bool,
    preview_position: f32,
    preview_key: Option<(usize, u32, u32, Fit)>,
    preview: Option<egui::TextureHandle>,
    error: String,
}

#[derive(Default)]
pub(crate) struct ExportState {
    dialog: Option<ExportDialog>,
    pub ffmpeg: PathBuf,
    job: Option<(Control, u32)>,
    failures: HashMap<PathBuf, String>,
}

impl EfudeApp {
    pub(crate) fn timelapse_menu_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let id = self.history.document_id();
        let state = self.timelapse_sessions.get(&id).map(|session| {
            (
                session.recording,
                session.paused,
                session.frames_written,
                session.session.include_guide,
            )
        });
        if state.is_none_or(|(recording, _, _, _)| !recording)
            && ui
                .button(self.text("タイムラプスの記録を開始…", "Start timelapse recording…"))
                .clicked()
        {
            self.show_timelapse_start = true;
            ui.close_menu();
        }
        if let Some((recording, paused, frames, include_guide)) = state {
            if recording {
                if ui
                    .button(if paused {
                        self.text("タイムラプスを再開", "Resume timelapse")
                    } else {
                        self.text("タイムラプスを一時停止", "Pause timelapse")
                    })
                    .clicked()
                {
                    if let Some(session) = self.timelapse_sessions.get_mut(&id) {
                        session.paused = !paused;
                    }
                    ui.close_menu();
                }
                if ui
                    .button(self.text("記録を終了", "Finish recording"))
                    .clicked()
                {
                    self.finish_timelapse(ctx);
                    ui.close_menu();
                }
            }
            let guide = if include_guide {
                self.text("ガイドあり", "Guide included")
            } else {
                self.text("ガイドなし", "No guide")
            };
            ui.label(format!("{frames} frames · {guide}"));
            if ui
                .add_enabled(
                    frames > 0 || !recording,
                    egui::Button::new(self.text("動画を書き出す…", "Export video…")),
                )
                .clicked()
            {
                self.export_timelapse_dialog(ctx);
                ui.close_menu();
            }
        }
        ui.separator();
        if ui
            .button(self.text(
                "以前の記録から動画を書き出す…",
                "Export previous recording…",
            ))
            .clicked()
        {
            self.export_previous_timelapse(ctx);
            ui.close_menu();
        }
    }

    pub(crate) fn timelapse_start_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_timelapse_start {
            return;
        }
        let mut open = self.show_timelapse_start;
        egui::Window::new(self.text("タイムラプスの記録", "Timelapse recording"))
            .open(&mut open).resizable(false).show(ctx, |ui| {
                let guide_label = self.text("下絵ガイドを動画に含める", "Include tracing guide in video");
                ui.checkbox(&mut self.timelapse_include_guide, guide_label);
                ui.label(self.text("OFFでも編集中の画面にはガイドを表示します。記録中は変更できません。", "The guide stays visible while editing when OFF. This cannot change during recording."));
                ui.horizontal(|ui| {
                    ui.label(self.text("長辺の上限", "Maximum long side"));
                    ui.selectable_value(&mut self.timelapse_max_side, 720, "720 px");
                    ui.selectable_value(&mut self.timelapse_max_side, 1080, "1080 px");
                    ui.selectable_value(&mut self.timelapse_max_side, 2160, "2160 px");
                });
                ui.label(self.text("キャンバス内容からフレームを作成します。画面は撮影しません。", "Frames come from canvas content. The screen is never captured."));
                if ui.button(self.text("記録を開始", "Start recording")).clicked() {
                    self.start_timelapse(ctx);
                    self.show_timelapse_start = false;
                }
            });
        if self.show_timelapse_start {
            self.show_timelapse_start = open;
        }
    }

    fn start_timelapse(&mut self, ctx: &egui::Context) {
        let id = self.history.document_id();
        match timelapse::create_session(
            &timelapse::default_root(),
            &self.doc,
            self.timelapse_include_guide,
            self.timelapse_max_side,
        ) {
            Ok(session) => {
                self.timelapse_sessions.insert(
                    id,
                    Recording {
                        session,
                        next_index: 1,
                        frames_written: 0,
                        last_content_revision: self.history.content_revision(),
                        last_capture: Instant::now() - Duration::from_secs(2),
                        recording: true,
                        paused: false,
                    },
                );
                self.capture_timelapse_frame(ctx, true, false);
                self.status = self
                    .text(
                        "タイムラプスの記録を開始しました",
                        "Timelapse recording started",
                    )
                    .into();
            }
            Err(error) => self.status = format!("タイムラプスを開始できません: {error}"),
        }
    }

    pub(crate) fn capture_timelapse_frame(
        &mut self,
        ctx: &egui::Context,
        force: bool,
        reliable: bool,
    ) {
        let id = self.history.document_id();
        let revision = self.history.content_revision();
        let Some(state) = self.timelapse_sessions.get(&id) else {
            return;
        };
        if !state.recording || state.paused {
            return;
        }
        if !force && revision == state.last_content_revision {
            return;
        }
        // Finishing a recording must retain the latest edit even if it was
        // made immediately after the previous frame.
        if !force && !reliable && state.last_capture.elapsed() < Duration::from_millis(500) {
            return;
        }
        if !force
            && (!self.active.is_empty()
                || self.stroke_builder.is_some()
                || self.history.is_active())
        {
            return;
        }
        let mut document = self.document_snapshot();
        if !state.session.include_guide {
            document.guide = None;
        }
        let task = Task::Frame {
            document_id: id,
            index: state.next_index,
            document,
            session: state.session.clone(),
            repaint: ctx.clone(),
        };
        let accepted = if reliable {
            self.timelapse_worker.sender.send(task).is_ok()
        } else {
            match self.timelapse_worker.sender.try_send(task) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => false,
                Err(TrySendError::Disconnected(_)) => {
                    self.status = "タイムラプスの処理が停止しました".into();
                    false
                }
            }
        };
        if accepted && let Some(state) = self.timelapse_sessions.get_mut(&id) {
            state.next_index += 1;
            state.last_content_revision = revision;
            state.last_capture = Instant::now();
        }
    }

    fn finish_timelapse(&mut self, ctx: &egui::Context) {
        let id = self.history.document_id();
        if self
            .timelapse_sessions
            .get(&id)
            .is_some_and(|state| state.recording)
        {
            if let Some(state) = self.timelapse_sessions.get_mut(&id) {
                state.paused = false;
            }
            self.capture_timelapse_frame(ctx, false, true);
            if let Some(state) = self.timelapse_sessions.get_mut(&id) {
                state.recording = false;
            }
            self.status = self
                .text(
                    "記録を終了しました。動画を書き出せます。",
                    "Recording finished. You can export the video.",
                )
                .into();
        }
    }

    pub(crate) fn finish_timelapse_on_tab_close(&mut self, document_id: u64) {
        let Some(state) = self.timelapse_sessions.remove(&document_id) else {
            return;
        };
        if !state.recording {
            return;
        }
        let (mut document, revision) = if self.history.document_id() == document_id {
            (self.document_snapshot(), self.history.content_revision())
        } else if let Some(tab) = self
            .tabs
            .slots
            .iter()
            .filter_map(|slot| slot.parked.as_ref())
            .find(|tab| tab.history.document_id() == document_id)
        {
            (tab.doc.clone(), tab.history.content_revision())
        } else {
            return;
        };
        if revision == state.last_content_revision {
            return;
        }
        if !state.session.include_guide {
            document.guide = None;
        }
        // Closing is also a recording stop. Preserve its latest committed edit,
        // including one that has not reached the normal capture interval yet.
        if self
            .timelapse_worker
            .sender
            .send(Task::Frame {
                document_id,
                index: state.next_index,
                document,
                session: state.session,
                repaint: egui::Context::default(),
            })
            .is_err()
        {
            self.status = self
                .text(
                    "タイムラプスの処理が停止しました",
                    "Timelapse worker stopped",
                )
                .into();
        }
    }

    fn queue_timelapse_export(
        &mut self,
        dialog: &ExportDialog,
        path: PathBuf,
        ctx: &egui::Context,
    ) {
        let control = Control::default();
        let source_count = timelapse::frame_paths(&dialog.session).map_or(0, |p| p.len());
        let total = if dialog.avi {
            source_count as u32
        } else {
            video::plan(&dialog.session, source_count, &dialog.options)
                .map_or(0, |p| p.indices.len() as u32)
        };
        self.status = match self.timelapse_worker.sender.send(Task::Export {
            session: dialog.session.clone(),
            path,
            required_index: dialog.required_index,
            options: dialog.options.clone(),
            ffmpeg: if self.timelapse_export.ffmpeg.as_os_str().is_empty() {
                PathBuf::from("ffmpeg")
            } else {
                self.timelapse_export.ffmpeg.clone()
            },
            avi: dialog.avi,
            control: control.clone(),
            repaint: ctx.clone(),
        }) {
            Ok(()) => {
                self.timelapse_export.job = Some((control, total));
                self.text("動画を書き出し中…", "Exporting video…").into()
            }
            Err(_) => self
                .text(
                    "タイムラプスの処理が停止しました",
                    "Timelapse worker stopped",
                )
                .into(),
        };
    }

    fn export_timelapse_dialog(&mut self, ctx: &egui::Context) {
        if self.history.is_active() || !self.active.is_empty() || self.stroke_builder.is_some() {
            self.status = self
                .text(
                    "描画中の操作を確定してから書き出してください",
                    "Finish the current edit before exporting",
                )
                .into();
            return;
        }
        self.commit_pending_guide_edit();
        self.finish_timelapse(ctx);
        let Some((session, required_index)) = self
            .timelapse_sessions
            .get(&self.history.document_id())
            .map(|recording| (recording.session.clone(), recording.next_index - 1))
        else {
            return;
        };
        let name = self
            .doc_path
            .as_deref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("Artwork");
        self.timelapse_export.dialog = Some(ExportDialog {
            session,
            required_index,
            name: format!("{name}-timelapse"),
            options: Options::default(),
            avi: false,
            preview_position: 1.0,
            preview_key: None,
            preview: None,
            error: String::new(),
        });
    }

    fn export_previous_timelapse(&mut self, _ctx: &egui::Context) {
        let Some(folder) = rfd::FileDialog::new()
            .set_directory(timelapse::default_root())
            .pick_folder()
        else {
            return;
        };
        let session = match timelapse::open_session(&folder) {
            Ok(session) => session,
            Err(error) => {
                self.status = format!("記録を開けません: {error}");
                return;
            }
        };
        self.timelapse_export.dialog = Some(ExportDialog {
            session,
            required_index: 0,
            name: "timelapse".into(),
            options: Options::default(),
            avi: false,
            preview_position: 1.0,
            preview_key: None,
            preview: None,
            error: String::new(),
        });
    }

    pub(crate) fn timelapse_export_dialog(&mut self, ctx: &egui::Context) {
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        if let Some((control, total)) = &self.timelapse_export.job {
            egui::Window::new(t("動画を書き出し中", "Exporting video"))
                .resizable(false)
                .show(ctx, |ui| {
                    let done = control
                        .frames_done
                        .load(std::sync::atomic::Ordering::Relaxed);
                    ui.add(
                        egui::ProgressBar::new(done as f32 / (*total).max(1) as f32)
                            .text(format!("{done} / {total}")),
                    );
                    if ui.button(t("中止", "Cancel")).clicked() {
                        control
                            .cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        let Some(mut dialog) = self.timelapse_export.dialog.take() else {
            return;
        };
        let mut open = true;
        let mut export = false;
        egui::Window::new(t("タイムラプスを書き出す", "Export timelapse"))
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut dialog.avi, false, "MP4 (H.264)");
                    ui.selectable_value(&mut dialog.avi, true, "AVI (MJPEG)");
                });
                if dialog.avi {
                    ui.label(t(
                        "AVIは記録時の寸法・30fpsで書き出します。",
                        "AVI uses recorded dimensions at 30 fps.",
                    ));
                } else {
                    ui.horizontal(|ui| {
                        ui.label(t("仕上がり時間", "Total duration"));
                        ui.add(
                            egui::DragValue::new(&mut dialog.options.duration)
                                .speed(1.0)
                                .range(1.0..=600.0)
                                .suffix(" s"),
                        );
                        ui.label(t("うち完成画像", "Final still"));
                        ui.add(
                            egui::DragValue::new(&mut dialog.options.hold)
                                .speed(0.1)
                                .range(0.0..=599.0)
                                .suffix(" s"),
                        );
                    });
                    let label = match dialog.options.size {
                        Size::Recorded => t("記録の寸法", "Recorded dimensions"),
                        Size::Long720 => "720 px",
                        Size::Long1080 => "1080 px",
                        Size::Portrait => "9:16 · 1080×1920",
                        Size::Landscape => "16:9 · 1920×1080",
                        Size::Square => "1:1 · 1080×1080",
                    };
                    egui::ComboBox::from_id_salt("timelapse-size")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for (size, label) in [
                                (Size::Recorded, t("記録の寸法", "Recorded dimensions")),
                                (Size::Long720, "720 px"),
                                (Size::Long1080, "1080 px"),
                                (Size::Portrait, "9:16 · 1080×1920"),
                                (Size::Landscape, "16:9 · 1920×1080"),
                                (Size::Square, "1:1 · 1080×1080"),
                            ] {
                                ui.selectable_value(&mut dialog.options.size, size, label);
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut dialog.options.fit,
                            Fit::Contain,
                            t("全体を収める", "Fit entire canvas"),
                        );
                        ui.selectable_value(
                            &mut dialog.options.fit,
                            Fit::Cover,
                            t("中央で切り抜く", "Crop to fill"),
                        );
                    });
                    ui.horizontal(|ui| {
                        let executable = if self.timelapse_export.ffmpeg.as_os_str().is_empty() {
                            "FFmpeg: PATH".into()
                        } else {
                            self.timelapse_export.ffmpeg.display().to_string()
                        };
                        ui.label(executable);
                        if ui.small_button(t("指定…", "Choose…")).clicked()
                            && let Some(path) = rfd::FileDialog::new().pick_file()
                        {
                            self.timelapse_export.ffmpeg = path;
                        }
                    });
                }
                let paths = timelapse::frame_paths(&dialog.session).map_err(|e| e.to_string());
                let plan = paths.as_ref().map_err(Clone::clone).and_then(|paths| {
                    if paths.is_empty() {
                        return Err("記録にフレームがありません".into());
                    }
                    if dialog.avi {
                        Ok(video::Plan {
                            indices: (0..paths.len()).collect(),
                            width: dialog.session.width,
                            height: dialog.session.height,
                            hold_frames: 0,
                        })
                    } else {
                        video::plan(&dialog.session, paths.len(), &dialog.options)
                    }
                });
                if let (Ok(paths), Ok(plan)) = (&paths, &plan) {
                    ui.label(format!(
                        "{} {} · {}×{} · {:.2} s",
                        paths.len(),
                        t("記録フレーム", "recorded frames"),
                        plan.width,
                        plan.height,
                        plan.duration()
                    ));
                    ui.add(
                        egui::Slider::new(&mut dialog.preview_position, 0.0..=1.0)
                            .text(t("プレビュー", "Preview")),
                    );
                    let index = plan.indices[((plan.indices.len() - 1) as f32
                        * dialog.preview_position)
                        .round() as usize];
                    let fit = if dialog.avi {
                        Fit::Contain
                    } else {
                        dialog.options.fit
                    };
                    let key = (index, plan.width, plan.height, fit);
                    if dialog.preview_key != Some(key) {
                        match video::read_frame(&dialog.session, &paths[index]) {
                            Ok(frame) => {
                                let image = video::layout(&frame, plan.width, plan.height, fit);
                                // The preview uses exactly the same crop before downscaling for display.
                                let scale = (360.0 / image.width() as f32)
                                    .min(230.0 / image.height() as f32)
                                    .min(1.0);
                                let image = image::imageops::resize(
                                    &image,
                                    (image.width() as f32 * scale).max(1.0) as u32,
                                    (image.height() as f32 * scale).max(1.0) as u32,
                                    image::imageops::FilterType::Triangle,
                                );
                                dialog.preview = Some(ctx.load_texture(
                                    "timelapse-preview",
                                    egui::ColorImage::from_rgb(
                                        [image.width() as usize, image.height() as usize],
                                        image.as_raw(),
                                    ),
                                    egui::TextureOptions::LINEAR,
                                ));
                                dialog.error.clear();
                            }
                            Err(error) => dialog.error = error,
                        }
                        dialog.preview_key = Some(key);
                    }
                    if let Some(texture) = &dialog.preview {
                        ui.image(texture);
                    }
                } else if let Err(error) = &plan {
                    ui.label(error);
                }
                if !dialog.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &dialog.error);
                }
                let failure = self.timelapse_export.failures.get(&dialog.session.folder);
                let ready = failure.is_none()
                    && paths.as_ref().is_ok_and(|p| {
                        !p.is_empty()
                            && (dialog.required_index == 0
                                || p.len() == dialog.required_index as usize)
                    });
                if let Some(failure) = failure {
                    ui.colored_label(Color32::LIGHT_RED, failure);
                } else if !ready && paths.is_ok() {
                    ui.label(t(
                        "最後のフレームの保存を待っています",
                        "Waiting for the final frame",
                    ));
                    ctx.request_repaint_after(Duration::from_millis(150));
                }
                export = ui
                    .add_enabled(
                        ready
                            && (dialog.avi || plan.is_ok())
                            && self.timelapse_export.job.is_none(),
                        egui::Button::new(t("書き出す…", "Export…")),
                    )
                    .clicked();
            });
        if export {
            let (format, extension) = if dialog.avi {
                ("AVI", "avi")
            } else {
                ("MP4", "mp4")
            };
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(format, &[extension])
                .set_file_name(format!("{}.{extension}", dialog.name))
                .save_file()
            {
                self.queue_timelapse_export(&dialog, path, ctx);
                open = false;
            }
        }
        if open {
            self.timelapse_export.dialog = Some(dialog);
        }
    }

    pub(crate) fn poll_timelapse_worker(&mut self) {
        while let Ok(completion) = self.timelapse_worker.receiver.try_recv() {
            match completion {
                Completion::Frame {
                    document_id,
                    session_folder,
                    index,
                } => {
                    if let Some(state) = self.timelapse_sessions.get_mut(&document_id)
                        && state.session.folder == session_folder
                    {
                        state.frames_written = state.frames_written.max(index);
                    }
                }
                Completion::Exported { path, count } => {
                    self.timelapse_export.job = None;
                    self.status = format!(
                        "タイムラプスを書き出しました（{count}フレーム）: {}",
                        path.display()
                    );
                }
                Completion::ExportFailed(message) => {
                    self.timelapse_export.job = None;
                    self.status = message;
                }
                Completion::Failed {
                    document_id,
                    session_folder,
                    message,
                } => {
                    // A finished recording can still have queued captures when
                    // another recording starts on the same document.
                    if let Some(state) = self.timelapse_sessions.get_mut(&document_id)
                        && state.session.folder == session_folder
                    {
                        state.paused = true;
                    }
                    self.timelapse_export
                        .failures
                        .insert(session_folder, message.clone());
                    self.status = message;
                }
            }
        }
    }

    pub(crate) fn timelapse_status(&self) -> Option<String> {
        let session = self.timelapse_sessions.get(&self.history.document_id())?;
        let mode = if session.session.include_guide {
            self.text("ガイドあり", "with guide")
        } else {
            self.text("ガイドなし", "no guide")
        };
        let state = if !session.recording {
            self.text("記録終了", "finished")
        } else if session.paused {
            self.text("一時停止", "paused")
        } else {
            self.text("記録中", "recording")
        };
        let frames = if self.language_english {
            format!("{} frames", session.frames_written)
        } else {
            format!("{}枚", session.frames_written)
        };
        Some(format!("● {state} · {frames} · {mode}"))
    }
}

pub(crate) type Sessions = HashMap<u64, Recording>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn deliver_capture_after_restart(
        fail: bool,
        restart: bool,
    ) -> (EfudeApp, tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let mut app = EfudeApp::default();
        app.doc = Document::new(32, 16);
        let id = app.history.document_id();
        let old = timelapse::create_session(root.path(), &app.doc, false, 720).unwrap();
        let old_folder = old.folder.clone();
        if fail {
            std::fs::rename(&old.folder, root.path().join("moved-recording")).unwrap();
        }
        app.timelapse_worker
            .sender
            .send(Task::Frame {
                document_id: id,
                index: 1,
                document: app.doc.clone(),
                session: old.clone(),
                repaint: egui::Context::default(),
            })
            .unwrap();
        let session = if restart {
            timelapse::create_session(root.path(), &app.doc, false, 720).unwrap()
        } else {
            old
        };
        let current_folder = session.folder.clone();
        app.timelapse_sessions.insert(
            id,
            Recording {
                session,
                next_index: 1,
                frames_written: 0,
                last_content_revision: app.history.content_revision(),
                last_capture: Instant::now(),
                recording: true,
                paused: false,
            },
        );
        // Deliver the real worker result only after the recording was restarted.
        let completion = app
            .timelapse_worker
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        sender.send(completion).unwrap();
        app.timelapse_worker.receiver = receiver;
        app.poll_timelapse_worker();
        (app, root, old_folder, current_folder)
    }

    #[test]
    fn old_capture_completion_does_not_change_a_restarted_recording_count() {
        let (app, _root, old, current) = deliver_capture_after_restart(false, true);
        assert_ne!(old, current);
        assert!(old.join("frame_00000001.jpg").exists());
        let state = &app.timelapse_sessions[&app.history.document_id()];
        assert_eq!(
            state.frames_written, 0,
            "old frames do not belong to the new recording"
        );
        assert!(!state.paused);
    }

    #[test]
    fn old_capture_failure_does_not_pause_or_poison_a_restarted_recording() {
        let (app, _root, old, current) = deliver_capture_after_restart(true, true);
        let state = &app.timelapse_sessions[&app.history.document_id()];
        assert!(
            !state.paused,
            "a previous recording's failure must not stop the new one"
        );
        assert!(app.timelapse_export.failures.contains_key(&old));
        assert!(!app.timelapse_export.failures.contains_key(&current));
    }

    #[test]
    fn current_capture_completion_still_updates_counts_and_reports_failure() {
        for fail in [false, true] {
            let (app, _root, _, current) = deliver_capture_after_restart(fail, false);
            let state = &app.timelapse_sessions[&app.history.document_id()];
            assert_eq!(state.frames_written, u32::from(!fail));
            assert_eq!(state.paused, fail);
            assert_eq!(app.timelapse_export.failures.contains_key(&current), fail);
        }
    }

    #[test]
    fn switching_to_avi_refreshes_preview_dimensions_and_crop() {
        let root = tempfile::tempdir().unwrap();
        let doc = Document::new(64, 32);
        let session = timelapse::create_session(root.path(), &doc, false, 720).unwrap();
        timelapse::write_frame(&session, 1, &doc).unwrap();
        let mut h = crate::tool_tests::Harness::new(64, 32);
        h.app.timelapse_export.dialog = Some(ExportDialog {
            session,
            required_index: 1,
            name: "preview".into(),
            options: Options {
                size: Size::Square,
                fit: Fit::Cover,
                ..Options::default()
            },
            avi: false,
            preview_position: 1.0,
            preview_key: None,
            preview: None,
            error: String::new(),
        });
        h.frames(3);
        assert_eq!(
            h.app.timelapse_export.dialog.as_ref().unwrap().preview_key,
            Some((0, 1080, 1080, Fit::Cover))
        );
        h.click_label("AVI (MJPEG)");
        let dialog = h.app.timelapse_export.dialog.as_ref().unwrap();
        assert_eq!(dialog.preview_key, Some((0, 64, 32, Fit::Contain)));
        assert_eq!(dialog.preview.as_ref().unwrap().size(), [64, 32]);
    }

    #[test]
    fn encoding_does_not_block_capture_and_in_flight_cancel_preserves_output() {
        let ffmpeg = PathBuf::from("ffmpeg");
        if !video::ffmpeg_available(&ffmpeg) {
            assert!(
                std::env::var_os("EFUDE_REQUIRE_FFMPEG").is_none(),
                "FFmpeg required"
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let doc = Document::new(64, 32);
        let first = timelapse::create_session(root.path(), &doc, false, 720).unwrap();
        let second = timelapse::create_session(root.path(), &doc, false, 720).unwrap();
        let output = root.path().join("result.mp4");
        std::fs::write(&output, b"previous output").unwrap();
        let control = Control::default();
        let mut worker = Worker::new();
        let ctx = egui::Context::default();
        for index in 1..=2 {
            worker
                .sender
                .send(Task::Frame {
                    document_id: 1,
                    index,
                    document: doc.clone(),
                    session: first.clone(),
                    repaint: ctx.clone(),
                })
                .unwrap();
        }
        worker
            .sender
            .send(Task::Export {
                session: first,
                path: output.clone(),
                required_index: 2,
                options: Options {
                    duration: 600.0,
                    size: Size::Square,
                    ..Options::default()
                },
                ffmpeg,
                avi: false,
                control: control.clone(),
                repaint: ctx.clone(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while control.frames_done.load(Ordering::Relaxed) < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            control.frames_done.load(Ordering::Relaxed) >= 3,
            "encoder did not start"
        );
        worker
            .sender
            .send(Task::Frame {
                document_id: 2,
                index: 1,
                document: doc,
                session: second.clone(),
                repaint: ctx,
            })
            .unwrap();
        loop {
            match worker
                .receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
            {
                Completion::Frame {
                    document_id: 2,
                    index: 1,
                    ..
                } => break,
                Completion::Frame { .. } => {}
                _ => panic!("capture was blocked or failed during encoding"),
            }
        }
        assert!(second.folder.join("frame_00000001.jpg").exists());
        assert!(control.frames_done.load(Ordering::Relaxed) < 18_000);
        control.cancel.store(true, Ordering::Relaxed);
        match worker
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            Completion::ExportFailed(message) => assert!(message.contains("中止"), "{message}"),
            _ => panic!("cancelled export unexpectedly succeeded"),
        }
        assert_eq!(std::fs::read(output).unwrap(), b"previous output");
        worker.sender.send(Task::Shutdown).unwrap();
        worker.join.take().unwrap().join().unwrap();
    }

    #[test]
    fn an_export_barrier_rejects_a_missing_final_capture() {
        let root = tempfile::tempdir().unwrap();
        let doc = Document::new(8, 8);
        let session = timelapse::create_session(root.path(), &doc, false, 720).unwrap();
        let path = root.path().join("incomplete.avi");
        let mut worker = Worker::new();
        let ctx = egui::Context::default();
        worker
            .sender
            .send(Task::Frame {
                document_id: 1,
                index: 1,
                document: doc,
                session: session.clone(),
                repaint: ctx.clone(),
            })
            .unwrap();
        worker
            .sender
            .send(Task::Export {
                session,
                path: path.clone(),
                required_index: 2,
                options: Options::default(),
                ffmpeg: PathBuf::new(),
                avi: true,
                control: Control::default(),
                repaint: ctx,
            })
            .unwrap();
        assert!(matches!(
            worker
                .receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            Completion::Frame { index: 1, .. }
        ));
        assert!(matches!(
            worker
                .receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            Completion::ExportFailed(_)
        ));
        assert!(!path.exists());
        worker.sender.send(Task::Shutdown).unwrap();
        worker.join.take().unwrap().join().unwrap();
    }
}
