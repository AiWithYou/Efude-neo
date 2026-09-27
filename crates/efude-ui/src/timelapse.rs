// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Timelapse orchestration, separate from the document save worker.
use super::*;
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
        document_id: u64,
        session: Session,
        path: PathBuf,
        repaint: egui::Context,
    },
    Shutdown,
}

pub(crate) enum Completion {
    Frame { document_id: u64, index: u32 },
    Exported { path: PathBuf, count: u32 },
    Failed { document_id: u64, message: String },
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
                                    Ok(_) => Completion::Frame { document_id, index },
                                    Err(error) => Completion::Failed {
                                        document_id,
                                        message: format!("タイムラプスの保存に失敗: {error}"),
                                    },
                                },
                                repaint,
                            )
                        }
                        Task::Export {
                            document_id,
                            session,
                            path,
                            repaint,
                        } => {
                            let result = timelapse::export_avi(&session, &path);
                            (
                                match result {
                                    Ok(count) => Completion::Exported { path, count },
                                    Err(error) => Completion::Failed {
                                        document_id,
                                        message: format!("動画を書き出せません: {error}"),
                                    },
                                },
                                repaint,
                            )
                        }
                        Task::Shutdown => break,
                    };
                    let _ = output.send(completion);
                    repaint.request_repaint();
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

    fn queue_timelapse_export(&mut self, session: Session, path: PathBuf, ctx: &egui::Context) {
        self.status = match self.timelapse_worker.sender.send(Task::Export {
            document_id: self.history.document_id(),
            session,
            path,
            repaint: ctx.clone(),
        }) {
            Ok(()) => self.text("動画を書き出し中…", "Exporting video…").into(),
            Err(_) => self
                .text(
                    "タイムラプスの処理が停止しました",
                    "Timelapse worker stopped",
                )
                .into(),
        };
    }

    fn export_timelapse_dialog(&mut self, ctx: &egui::Context) {
        self.finish_timelapse(ctx);
        let Some(session) = self
            .timelapse_sessions
            .get(&self.history.document_id())
            .map(|recording| recording.session.clone())
        else {
            return;
        };
        let name = self
            .doc_path
            .as_deref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("Artwork");
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("AVI video", &["avi"])
            .set_file_name(format!("{name}-timelapse.avi"))
            .save_file()
        {
            self.queue_timelapse_export(session, path, ctx);
        }
    }

    fn export_previous_timelapse(&mut self, ctx: &egui::Context) {
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
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("AVI video", &["avi"])
            .set_file_name("timelapse.avi")
            .save_file()
        {
            self.queue_timelapse_export(session, path, ctx);
        }
    }

    pub(crate) fn poll_timelapse_worker(&mut self) {
        while let Ok(completion) = self.timelapse_worker.receiver.try_recv() {
            match completion {
                Completion::Frame { document_id, index } => {
                    if let Some(state) = self.timelapse_sessions.get_mut(&document_id) {
                        state.frames_written = state.frames_written.max(index);
                    }
                }
                Completion::Exported { path, count } => {
                    self.status = format!(
                        "タイムラプスを書き出しました（{count}フレーム）: {}",
                        path.display()
                    );
                }
                Completion::Failed {
                    document_id,
                    message,
                } => {
                    if let Some(state) = self.timelapse_sessions.get_mut(&document_id) {
                        state.paused = true;
                    }
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
