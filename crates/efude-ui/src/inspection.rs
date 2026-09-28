// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
use super::*;
use efude_canvas::inspection::{self, Kind, Options, Report};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

struct Job {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<Report, String>>,
    document_id: u64,
    state_token: u64,
}

#[derive(Default)]
pub(crate) struct State {
    pub open: bool,
    document_id: u64,
    state_token: u64,
    layers: Vec<u64>,
    options: Options,
    region: u8,
    custom: [u32; 4],
    job: Option<Job>,
    report: Option<Report>,
    ignored: std::collections::HashSet<usize>,
    message: String,
    overlay: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl EfudeApp {
    pub(crate) fn open_finishing_check(&mut self) {
        let id = self.history.document_id();
        if self.inspection.document_id != id {
            self.inspection = State::default();
            self.inspection.document_id = id;
            let layer = self.doc.layers.get(self.selected_layer);
            self.inspection.layers = layer.map_or_else(Vec::new, |layer| {
                if layer.kind == LayerKind::Raster {
                    vec![layer.id]
                } else {
                    efude_canvas::subtree_ids(&self.doc.layers, layer.id)
                        .into_iter()
                        .filter(|id| {
                            self.doc
                                .layers
                                .iter()
                                .any(|l| l.id == *id && l.kind == LayerKind::Raster)
                        })
                        .collect()
                }
            });
            self.inspection.custom = [0, 0, self.doc.width, self.doc.height];
            self.inspection.region = u8::from(self.comic_doc().is_some());
            self.inspection.overlay = true;
        }
        self.inspection.open = true;
    }

    pub(crate) fn finishing_check_dialog(&mut self, ctx: &egui::Context) {
        let doc_id = self.history.document_id();
        let token = self.history.state_token();
        if self.inspection.document_id != doc_id {
            if self.inspection.open {
                self.open_finishing_check();
            } else {
                self.inspection.report = None;
            }
        }
        if self.inspection.report.is_some() && self.inspection.state_token != token {
            self.inspection.report = None;
            self.inspection.message = self
                .text(
                    "編集されたため、もう一度チェックしてください",
                    "Artwork changed. Check again.",
                )
                .into();
        }
        if let Some(job) = &self.inspection.job {
            if job.document_id != doc_id || job.state_token != token {
                job.cancel.store(true, Ordering::Relaxed);
                self.inspection.job = None;
                self.inspection.message = self
                    .text(
                        "編集されたためチェックを中止しました",
                        "Check cancelled because the artwork changed.",
                    )
                    .into();
            } else if let Ok(result) = job.receiver.try_recv() {
                self.inspection.job = None;
                match result {
                    Ok(report) => {
                        self.inspection.state_token = token;
                        self.inspection.report = Some(report);
                        self.inspection.ignored.clear();
                        self.inspection.message.clear();
                    }
                    Err(message) => self.inspection.message = message,
                }
            }
        }
        if !self.inspection.open {
            return;
        }
        let en = self.language_english;
        let trim = self
            .comic_doc()
            .map(|c| c.page.geometry())
            .filter(|g| g.canvas_width == self.doc.width && g.canvas_height == self.doc.height)
            .map(|g| {
                [
                    g.trim[0].x.floor() as u32,
                    g.trim[0].y.floor() as u32,
                    g.trim[1].x.ceil() as u32,
                    g.trim[1].y.ceil() as u32,
                ]
            });
        let mut open = true;
        let mut changed = false;
        let mut start = false;
        let mut focus = None;
        egui::Window::new(if en {
            "Finishing check"
        } else {
            "塗り残し・消し忘れチェック"
        })
        .open(&mut open)
        .default_width(430.0)
        .show(ctx, |ui| {
            ui.collapsing(
                if en {
                    "Layers to check"
                } else {
                    "対象レイヤー"
                },
                |ui| {
                    ui.label(if en {
                        "Exclude an opaque background to detect transparent holes."
                    } else {
                        "透明穴を調べるときは、不透明な背景を対象から外してください。"
                    });
                    egui::ScrollArea::vertical()
                        .id_salt("check_layers")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for layer in self
                                .doc
                                .layers
                                .iter()
                                .filter(|l| l.kind == LayerKind::Raster)
                            {
                                let mut checked = self.inspection.layers.contains(&layer.id);
                                if ui.checkbox(&mut checked, &layer.name).changed() {
                                    self.inspection.layers.retain(|id| *id != layer.id);
                                    if checked {
                                        self.inspection.layers.push(layer.id);
                                    }
                                    changed = true;
                                }
                            }
                        });
                },
            );
            ui.horizontal(|ui| {
                ui.label(if en {
                    "Maximum hole area"
                } else {
                    "透明穴の最大面積"
                });
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.inspection.options.hole_area)
                            .range(0..=10_000)
                            .suffix(" px²"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(if en {
                    "Maximum speck area"
                } else {
                    "孤立点の最大面積"
                });
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.inspection.options.speck_area)
                            .range(0..=10_000)
                            .suffix(" px²"),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(if en {
                    "Alpha threshold"
                } else {
                    "透明とみなすアルファ"
                });
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.inspection.options.alpha_threshold)
                            .range(0..=254),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(if en {
                    "Finishing area"
                } else {
                    "仕上がり範囲"
                });
                changed |= ui
                    .selectable_value(
                        &mut self.inspection.region,
                        0,
                        if en { "Canvas" } else { "キャンバス" },
                    )
                    .changed();
                if trim.is_some() {
                    changed |= ui
                        .selectable_value(
                            &mut self.inspection.region,
                            1,
                            if en { "Trim" } else { "漫画の仕上がり" },
                        )
                        .changed();
                }
                changed |= ui
                    .selectable_value(
                        &mut self.inspection.region,
                        2,
                        if en { "Custom" } else { "指定" },
                    )
                    .changed();
            });
            if self.inspection.region == 2 {
                ui.horizontal(|ui| {
                    for (i, name) in ["X₁", "Y₁", "X₂", "Y₂"].into_iter().enumerate() {
                        ui.label(name);
                        changed |= ui
                            .add(egui::DragValue::new(&mut self.inspection.custom[i]).range(
                                0..=if i % 2 == 0 {
                                    self.doc.width
                                } else {
                                    self.doc.height
                                },
                            ))
                            .changed();
                    }
                });
            }
            if self.inspection.region == 1 {
                ui.label(if en {
                    "Bleed beyond the trim can be intentional."
                } else {
                    "仕上がり外の塗り足しは、意図した描画の場合もあります。"
                });
            }
            ui.horizontal(|ui| {
                start = ui
                    .add_enabled(
                        self.inspection.job.is_none()
                            && !self.inspection.layers.is_empty()
                            && self.active.is_empty()
                            && !self.history.is_active(),
                        egui::Button::new(if en { "Check" } else { "チェック" }),
                    )
                    .clicked();
                if let Some(job) = &self.inspection.job {
                    ui.spinner();
                    if ui.button(if en { "Cancel" } else { "中止" }).clicked() {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                }
                ui.checkbox(
                    &mut self.inspection.overlay,
                    if en { "Highlight" } else { "候補を強調" },
                );
            });
            if !self.inspection.message.is_empty() {
                ui.label(&self.inspection.message);
            }
            if let Some(report) = &self.inspection.report {
                ui.label(if en {
                    format!(
                        "Holes {} · Specks {} · Outside {}",
                        report.counts[0], report.counts[1], report.counts[2]
                    )
                } else {
                    format!(
                        "透明穴 {} · 孤立点 {} · 範囲外 {}",
                        report.counts[0], report.counts[1], report.counts[2]
                    )
                });
                if report.hidden_layers > 0 {
                    ui.label(if en {
                        format!(
                            "{} hidden or zero-opacity layers were excluded.",
                            report.hidden_layers
                        )
                    } else {
                        format!(
                            "非表示・不透明度0の{}レイヤーは検出に含まれていません。",
                            report.hidden_layers
                        )
                    });
                }
                if report.counts.iter().sum::<usize>() > report.candidates.len() {
                    ui.label(if en {
                        "Showing the first 500 candidates."
                    } else {
                        "先頭500候補を表示しています。"
                    });
                }
                egui::ScrollArea::vertical()
                    .id_salt("check_results")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (i, candidate) in report.candidates.iter().enumerate() {
                            if self.inspection.ignored.contains(&i) {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                let name = match (candidate.kind, en) {
                                    (Kind::Hole, false) => "透明穴",
                                    (Kind::Speck, false) => "孤立点",
                                    (Kind::Outside, false) => "範囲外",
                                    (Kind::Hole, true) => "Hole",
                                    (Kind::Speck, true) => "Speck",
                                    (Kind::Outside, true) => "Outside",
                                };
                                if ui
                                    .button(format!(
                                        "{name} · {} px² · ({}, {})",
                                        candidate.area, candidate.bounds[0], candidate.bounds[1]
                                    ))
                                    .clicked()
                                {
                                    focus = Some(candidate.bounds);
                                }
                                if ui
                                    .small_button(if en { "Ignore" } else { "対象外" })
                                    .clicked()
                                {
                                    self.inspection.ignored.insert(i);
                                }
                            });
                        }
                    });
            }
        });
        self.inspection.open = open;
        if changed {
            self.inspection.report = None;
            if let Some(job) = self.inspection.job.take() {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        if let Some(b) = focus {
            self.navigator_center =
                Vec2::new((b[0] + b[2]) as f32 * 0.5, (b[1] + b[3]) as f32 * 0.5);
            self.zoom = (220.0
                / ((b[2] - b[0]).max(b[3] - b[1]).max(16) as f32
                    * self.canvas_viewport.1.max(0.001)))
            .clamp(0.01, 64.0);
        }
        if start {
            self.inspection.options.finishing_bounds = match self.inspection.region {
                1 => trim,
                2 => Some(self.inspection.custom),
                _ => None,
            };
            let options = self.inspection.options.clone();
            let document = self.document_snapshot();
            let layers = self.inspection.layers.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = cancel.clone();
            let (sender, receiver) = mpsc::channel();
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("efude-finishing-check".into())
                .spawn(move || {
                    let result = inspection::inspect(&document, &layers, &options, &flag);
                    let _ = sender.send(result);
                    repaint.request_repaint();
                }) {
                Ok(_) => {
                    self.inspection.job = Some(Job {
                        cancel,
                        receiver,
                        document_id: doc_id,
                        state_token: token,
                    });
                    self.inspection.report = None;
                    self.inspection.message.clear();
                }
                Err(error) => self.inspection.message = error.to_string(),
            }
        }
    }

    pub(crate) fn paint_finishing_check(
        &self,
        painter: &egui::Painter,
        to_screen: &impl Fn(Vec2) -> Pos2,
    ) {
        if !self.inspection.overlay
            || self.inspection.document_id != self.history.document_id()
            || self.inspection.state_token != self.history.state_token()
            || !self.active.is_empty()
            || self.history.is_active()
        {
            return;
        }
        let Some(report) = &self.inspection.report else {
            return;
        };
        for (i, candidate) in report.candidates.iter().enumerate() {
            if self.inspection.ignored.contains(&i) {
                continue;
            }
            let b = candidate.bounds;
            let color = match candidate.kind {
                Kind::Hole => Color32::from_rgb(0, 210, 255),
                Kind::Speck => Color32::from_rgb(255, 80, 190),
                Kind::Outside => Color32::from_rgb(255, 170, 30),
            };
            let points = [
                Vec2::new(b[0] as f32, b[1] as f32),
                Vec2::new(b[2] as f32, b[1] as f32),
                Vec2::new(b[2] as f32, b[3] as f32),
                Vec2::new(b[0] as f32, b[3] as f32),
            ]
            .map(to_screen);
            for j in 0..4 {
                painter.line_segment([points[j], points[(j + 1) % 4]], Stroke::new(2.0, color));
            }
            let center = to_screen(Vec2::new(
                (b[0] + b[2]) as f32 * 0.5,
                (b[1] + b[3]) as f32 * 0.5,
            ));
            painter.circle_stroke(center, 5.0, Stroke::new(1.5, color));
        }
    }
}
