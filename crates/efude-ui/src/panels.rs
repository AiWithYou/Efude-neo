// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Panel contents. The layout that places them is in `layout.rs`.

use super::*;

impl EfudeApp {
    pub(crate) fn commit_pending_guide_edit(&mut self) {
        if let Some(document_id) = self.guide_edit_pending.take()
            && document_id == self.history.document_id()
        {
            self.history.commit();
        }
    }

    pub(crate) fn guide_menu_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(240.0);
        if ui
            .button(self.text("画像を読み込む…", "Import image…"))
            .clicked()
        {
            self.commit_pending_guide_edit();
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Image", &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
                .pick_file()
            {
                self.status = match self.io_task_sender.send(IoTask::LoadGuide {
                    document_id: self.history.document_id(),
                    path,
                    repaint: ctx.clone(),
                }) {
                    Ok(()) => self
                        .text("下絵ガイドを読み込み中…", "Loading tracing guide…")
                        .into(),
                    Err(_) => self
                        .text("画像読み込みワーカーが停止しました", "Image worker stopped")
                        .into(),
                };
            }
            ui.close_menu();
        }
        if let Some(mut guide) = self.doc.guide.clone() {
            ui.separator();
            ui.label(self.text("描画レイヤーの下に表示", "Shown beneath paint layers"));
            let mut changed = ui
                .checkbox(&mut guide.visible, self.text("表示", "Visible"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut guide.opacity, 0.0..=1.0)
                        .text(self.text("不透明度", "Opacity")),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut guide.offset_x)
                        .speed(1.0)
                        .prefix("X "),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut guide.offset_y)
                        .speed(1.0)
                        .prefix("Y "),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut guide.scale)
                        .speed(0.005)
                        .range(0.01..=100.0)
                        .prefix(self.text("倍率 ", "Scale ")),
                )
                .changed();
            if changed {
                if ui.input(|input| input.pointer.primary_down())
                    && self.guide_edit_pending.is_none()
                {
                    self.history.begin();
                    self.guide_edit_pending = Some(self.history.document_id());
                }
                self.history.set_guide(&mut self.doc, Some(guide));
                self.canvas_texture_dirty = true;
                self.navigator_texture_dirty = true;
            }
            if ui
                .button(self.text("キャンバスに合わせる", "Fit to canvas"))
                .clicked()
            {
                self.commit_pending_guide_edit();
                if let Some(mut guide) = self.doc.guide.clone() {
                    guide.scale = (self.doc.width as f32 / guide.width as f32)
                        .min(self.doc.height as f32 / guide.height as f32);
                    guide.offset_x =
                        (self.doc.width as f32 - guide.width as f32 * guide.scale) / 2.0;
                    guide.offset_y =
                        (self.doc.height as f32 - guide.height as f32 * guide.scale) / 2.0;
                    self.history.set_guide(&mut self.doc, Some(guide));
                    self.canvas_texture_dirty = true;
                    self.navigator_texture_dirty = true;
                }
            }
            ui.separator();
            if ui
                .button(self.text("下絵ガイドを削除", "Remove tracing guide"))
                .clicked()
            {
                self.commit_pending_guide_edit();
                self.history.set_guide(&mut self.doc, None);
                self.canvas_texture_dirty = true;
                self.navigator_texture_dirty = true;
                ui.close_menu();
            }
        }
    }
    /// Asks for a file and opens it the way its type needs: Efude
    /// documents and PSD files with their layers, PNG, JPEG, BMP and GIF
    /// images as a one-layer picture.
    pub(crate) fn open_document_dialog(&mut self, ctx: &egui::Context) {
        let english = self.language_english;
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                if english {
                    "All supported"
                } else {
                    "対応するすべて"
                },
                &["efude", "psd", "png", "jpg", "jpeg", "bmp", "gif"],
            )
            .add_filter("Efude", &["efude"])
            .add_filter("PSD", &["psd"])
            .add_filter(
                if english { "Images" } else { "画像" },
                &["png", "jpg", "jpeg", "bmp", "gif"],
            )
            .pick_file()
        else {
            return;
        };
        if !self.confirm_document_replacement() {
            return;
        }
        self.open_path(path, ctx);
    }

    /// Opens `path` by its extension (see `open_document_dialog`).
    pub(crate) fn open_path(&mut self, path: std::path::PathBuf, ctx: &egui::Context) {
        self.cancel_studio_session();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.status = match extension.as_str() {
            "psd" => match self.queue_document_load(path, true, ctx) {
                Ok(()) => self.text("PSD読み込み中…", "Loading PSD…").into(),
                Err(error) => error,
            },
            "png" | "jpg" | "jpeg" | "bmp" | "gif" => match self.queue_image_document(path, ctx) {
                Ok(()) => self.text("画像を読み込み中…", "Loading image…").into(),
                Err(error) => error,
            },
            _ => match self.queue_document_load(path, false, ctx) {
                Ok(()) => self.text("読み込み中…", "Loading…").into(),
                Err(error) => error,
            },
        };
    }

    /// Asks for a file name and exports the picture in the format chosen
    /// (by the file type, or the name's extension).
    pub(crate) fn export_dialog(&mut self, ctx: &egui::Context) {
        let stem = self
            .doc_path
            .as_deref()
            .and_then(std::path::Path::file_stem)
            .and_then(|name| name.to_str())
            .unwrap_or("Artwork")
            .to_owned();
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter(
                if self.language_english {
                    "PSD (layers)"
                } else {
                    "PSD（レイヤー付き）"
                },
                &["psd"],
            )
            .set_file_name(format!("{stem}.png"))
            .save_file()
        else {
            return;
        };
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let format = match extension.as_str() {
            "jpg" | "jpeg" => ExportFormat::Jpeg,
            "psd" => ExportFormat::Psd,
            "png" => ExportFormat::Png,
            _ => {
                path.set_extension("png");
                ExportFormat::Png
            }
        };
        let message = match format {
            ExportFormat::Png => self.text("PNGを書き出し中…", "Exporting PNG…"),
            ExportFormat::Jpeg => self.text("JPEGを書き出し中…", "Exporting JPEG…"),
            ExportFormat::Psd => {
                self.text("レイヤー付きPSDを書き出し中…", "Exporting layered PSD…")
            }
        }
        .to_owned();
        self.status = match self.queue_export(path, format, ctx) {
            Ok(()) => message,
            Err(error) => error,
        };
    }

    /// Asks for a file name and saves the document there.
    pub(crate) fn save_as_dialog(&mut self, ctx: &egui::Context) {
        let path = rfd::FileDialog::new()
            .add_filter("Efude", &["efude"])
            .set_file_name(self.suggested_save_name())
            .save_file();
        match self.save_to_chosen_path(path, ctx) {
            Ok(true) => self.status = self.text("別名保存中…", "Saving As…").into(),
            Ok(false) => {}
            Err(error) => self.status = error,
        }
    }

    /// Replaces every brush preset (keeping the current tool).
    pub(crate) fn replace_brushes(&mut self, brushes: Vec<efude_brush::Brush>) {
        if brushes.is_empty() {
            return;
        }
        self.brushes = brushes;
        self.tool_brushes = [None; 4];
        self.brush_previews.clear();
        let tool = self.tool;
        self.selected_brush = self
            .brushes
            .iter()
            .position(|brush| crate::brush_fits_tool(brush.kind, tool))
            .unwrap_or(0);
        self.size = self.brushes[self.selected_brush].size;
    }

    /// Deletes every layer and leaves one empty raster layer, as a single
    /// step that Undo reverses.
    pub(crate) fn clear_all_layers(&mut self) {
        self.finish_pending_canvas_gesture();
        let english = self.language_english;
        let id = self.next_layer_id();
        let layer = efude_canvas::Layer::new(
            id,
            if english {
                format!("Layer {id}")
            } else {
                format!("レイヤー {id}")
            },
            self.doc.width,
            self.doc.height,
        );
        self.history.begin();
        let at = self.doc.layers.len();
        self.history.insert_layer(&mut self.doc.layers, at, layer);
        while self.doc.layers.len() > 1 {
            let before = self.doc.layers.len();
            self.delete_document_layer(0);
            if self.doc.layers.len() == before {
                break;
            }
        }
        self.history
            .commit_labeled(self.text("レイヤーをすべて削除", "Delete all layers"));
        self.selected_layer = 0;
        self.status = self
            .text("レイヤーをすべて削除しました", "Deleted all layers")
            .into();
    }

    /// Adds an empty raster layer above the selected one and selects it.
    pub(crate) fn add_raster_layer(&mut self) {
        self.finish_pending_canvas_gesture();
        if !self.can_add_layers(1) {
            return;
        }
        let source_id = self
            .doc
            .layers
            .get(self.selected_layer)
            .map_or(0, |layer| layer.id);
        let english = self.language_english;
        let id = self.next_layer_id();
        let layer = efude_canvas::Layer::new(
            id,
            if english {
                format!("Layer {id}")
            } else {
                format!("レイヤー {id}")
            },
            self.doc.width,
            self.doc.height,
        );
        self.insert_layer_above_selected(layer);
        self.record_macro_creation(macros::Step::NewRaster, source_id, &[id]);
    }

    /// File actions: new, open, save and export.
    #[allow(unused_variables)]
    pub(crate) fn file_menu_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        if ui
            .add(egui::Button::new(self.text("新規…", "New…")).shortcut_text("Ctrl+N"))
            .clicked()
        {
            self.show_new_document = true;
        }
        if ui
            .add(egui::Button::new(self.text("開く", "Open")).shortcut_text("Ctrl+O"))
            .clicked()
        {
            self.open_document_dialog(ctx);
        }
        if ui
            .add(egui::Button::new(self.text("保存", "Save")).shortcut_text("Ctrl+S"))
            .clicked()
        {
            self.status = match self.request_native_save(ctx) {
                Ok(true) => self.text("保存中…", "Saving…").into(),
                Ok(false) => self.status.clone(),
                Err(error) => error,
            };
        }
        if ui
            .add(
                egui::Button::new(self.text("別名で保存", "Save As")).shortcut_text("Ctrl+Shift+S"),
            )
            .clicked()
        {
            self.save_as_dialog(ctx);
        }
        if ui
            .button(self.text("書き出し…（PNG・JPEG・PSD）", "Export… (PNG, JPEG, PSD)"))
            .clicked()
        {
            self.export_dialog(ctx);
        }
        if ui
            .button(self.text("作業の復旧…", "Recover work…"))
            .clicked()
        {
            self.recovery.show_dialog = true;
        }
        ui.separator();
        if ui.button(self.text("環境設定…", "Preferences…")).clicked() {
            self.show_settings = true;
        }
    }

    /// Undo and redo buttons.
    #[allow(unused_variables)]
    pub(crate) fn undo_redo_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        if ui
            .add_enabled(
                self.history.can_undo(),
                egui::Button::new(if english { "Undo" } else { "取り消し" })
                    .shortcut_text("Ctrl+Z"),
            )
            .clicked()
        {
            self.undo();
        }
        if ui
            .add_enabled(
                self.history.can_redo(),
                egui::Button::new(if english { "Redo" } else { "やり直し" })
                    .shortcut_text("Ctrl+Y"),
            )
            .clicked()
        {
            self.redo();
        }
    }

    pub(crate) fn select_all(&mut self) {
        self.change_selection(|selection, width, height| {
            selection.rectangle(width, height, (0, 0), (width as i32 - 1, height as i32 - 1))
        });
    }

    /// Tablet input, shortcuts, latency and backups.
    #[allow(unused_variables)]
    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.collapsing(self.text("入力診断", "Input diagnostics"), |ui| {
            self.input_diagnostics_ui(ui)
        });
        let english = self.language_english;
        #[cfg(target_os = "windows")]
        egui::ComboBox::from_label(self.text("タブレット入力", "Tablet Input"))
            .selected_text(if self.use_wintab {
                "WinTab"
            } else if self.use_windows_ink {
                "Windows Ink"
            } else {
                self.text("ウィンドウ入力", "Window Input")
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.use_windows_ink, "Windows Ink")
                    .clicked()
                {
                    self.use_windows_ink = true;
                    self.use_wintab = false;
                    self.sync_windows_ink_hook();
                    self.frame_pen_packets.clear();
                    self.pen_queue.clear();
                }
                if ui.selectable_label(self.use_wintab, "WinTab").clicked() {
                    self.use_windows_ink = false;
                    self.use_wintab = true;
                    self.sync_windows_ink_hook();
                    self.frame_pen_packets.clear();
                    self.pen_queue.clear();
                }
                if ui
                    .selectable_label(
                        !self.use_windows_ink && !self.use_wintab,
                        self.text("ウィンドウ入力", "Window Input"),
                    )
                    .clicked()
                {
                    self.use_windows_ink = false;
                    self.use_wintab = false;
                    self.sync_windows_ink_hook();
                    self.frame_pen_packets.clear();
                    self.pen_queue.clear();
                }
            });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(self.text("筆圧（アプリ全体）", "Pen Pressure (all brushes)"))
                .strong(),
        );
        ui.label(
            egui::RichText::new(self.text(
                "ペンの筆圧をまずこのカーブで整えてから、各ブラシの筆圧カーブに渡します。点をドラッグして調整します。",
                "Pen pressure passes through this curve first, then through each brush's own curve. Drag the points to adjust.",
            ))
            .small()
            .color(crate::layout::MUTED_TEXT),
        );
        let last_pressure = self.input_diagnostics.max_pressure;
        pressure_curve_editor(
            ui,
            "app-pressure-curve",
            &mut self.pressure_curve_points,
            1.0,
            last_pressure,
        );
        if ui
            .small_button(self.text("直線に戻す", "Reset to linear"))
            .clicked()
        {
            self.pressure_curve_points = [0.25, 0.25, 0.75, 0.75];
        }
        ui.collapsing(
            self.text("ショートカット設定", "Shortcut Settings"),
            |ui| {
                ui.label(self.text(
                    "Ctrl+操作は英字1文字を設定",
                    "Set one letter for each Ctrl shortcut",
                ));
                ui.horizontal(|ui| {
                    ui.label("Undo");
                    ui.text_edit_singleline(&mut self.shortcuts.undo);
                });
                ui.horizontal(|ui| {
                    ui.label("Redo");
                    ui.text_edit_singleline(&mut self.shortcuts.redo);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("保存", "Save"));
                    ui.text_edit_singleline(&mut self.shortcuts.save);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("コピー", "Copy"));
                    ui.text_edit_singleline(&mut self.shortcuts.copy);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("切り取り", "Cut"));
                    ui.text_edit_singleline(&mut self.shortcuts.cut);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("貼り付け", "Paste"));
                    ui.text_edit_singleline(&mut self.shortcuts.paste);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("全選択", "Select All"));
                    ui.text_edit_singleline(&mut self.shortcuts.select_all);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("選択解除", "Deselect"));
                    ui.text_edit_singleline(&mut self.shortcuts.deselect);
                });
                ui.horizontal(|ui| {
                    ui.label(self.text("一時消しゴム", "Temporary Eraser"));
                    ui.text_edit_singleline(&mut self.shortcuts.eraser);
                });
                ui.horizontal(|ui| {
                    ui.label(if self.language_english {
                        "Brush"
                    } else {
                        "ブラシ"
                    });
                    ui.text_edit_singleline(&mut self.shortcuts.brush_tool);
                });
                ui.horizontal(|ui| {
                    ui.label(if self.language_english {
                        "Rectangle Select"
                    } else {
                        "矩形選択"
                    });
                    ui.text_edit_singleline(&mut self.shortcuts.rectangle_tool);
                });
                ui.horizontal(|ui| {
                    ui.label(if self.language_english {
                        "Fill"
                    } else {
                        "塗りつぶし"
                    });
                    ui.text_edit_singleline(&mut self.shortcuts.fill_tool);
                });
                ui.horizontal(|ui| {
                    ui.label(if self.language_english {
                        "Eyedropper"
                    } else {
                        "スポイト"
                    });
                    ui.text_edit_singleline(&mut self.shortcuts.eyedropper_tool);
                });
                ui.horizontal(|ui| {
                    ui.label(if self.language_english {
                        "Move"
                    } else {
                        "移動"
                    });
                    ui.text_edit_singleline(&mut self.shortcuts.move_tool);
                });
                ui.horizontal(|ui| {
                    ui.label(if self.language_english {
                        "Pan"
                    } else {
                        "手のひら"
                    });
                    ui.text_edit_singleline(&mut self.shortcuts.pan_tool);
                });
                for (english, japanese, shortcut) in [
                    ("Blur", "ぼかし", &mut self.shortcuts.blur_tool),
                    ("Smudge", "指先", &mut self.shortcuts.smudge_tool),
                    (
                        "Ellipse Select",
                        "楕円選択",
                        &mut self.shortcuts.ellipse_tool,
                    ),
                    ("Lasso", "投げ縄", &mut self.shortcuts.lasso_tool),
                    (
                        "Polygon Select",
                        "多角形選択",
                        &mut self.shortcuts.polygon_tool,
                    ),
                    (
                        "Magic Wand",
                        "自動選択",
                        &mut self.shortcuts.magic_wand_tool,
                    ),
                    (
                        "Color Range",
                        "色域選択",
                        &mut self.shortcuts.color_range_tool,
                    ),
                    (
                        "Line Ruler",
                        "直線定規",
                        &mut self.shortcuts.line_ruler_tool,
                    ),
                    (
                        "Ellipse Ruler",
                        "楕円定規",
                        &mut self.shortcuts.ellipse_ruler_tool,
                    ),
                    (
                        "Bezier Ruler",
                        "ベジェ曲線定規",
                        &mut self.shortcuts.bezier_ruler_tool,
                    ),
                    (
                        "Perspective Ruler",
                        "透視定規",
                        &mut self.shortcuts.perspective_ruler_tool,
                    ),
                    (
                        "Selection Brush",
                        "選択範囲ペン",
                        &mut self.shortcuts.selection_brush_tool,
                    ),
                    (
                        "Gradient",
                        "グラデーション",
                        &mut self.shortcuts.gradient_tool,
                    ),
                    ("Shape", "図形", &mut self.shortcuts.shape_tool),
                    ("Crop", "切り抜き", &mut self.shortcuts.crop_tool),
                    (
                        "Quick Mask",
                        "クイックマスク",
                        &mut self.shortcuts.quick_mask_tool,
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(if self.language_english {
                            english
                        } else {
                            japanese
                        });
                        ui.text_edit_singleline(shortcut);
                    });
                }
                let mut duplicates = self
                    .conflicting_tool_keys()
                    .into_iter()
                    .map(|key| format!("{key:?}"))
                    .collect::<Vec<_>>();
                duplicates.sort();
                if !duplicates.is_empty() {
                    ui.colored_label(
                        Color32::YELLOW,
                        format!(
                            "{} {}",
                            self.text(
                                "複数のツールに割り当てたキーは無効です",
                                "Disabled keys assigned to multiple tools"
                            ),
                            duplicates.join(", ")
                        ),
                    );
                }
                let mut conflict_labels = Vec::new();
                let tool_bindings = self.tool_bindings();
                for key in [egui::Key::X, egui::Key::D, egui::Key::H, egui::Key::R] {
                    if tool_bindings
                        .iter()
                        .filter(|(bound, _)| *bound == key)
                        .count()
                        == 1
                    {
                        conflict_labels.push(format!("{key:?}"));
                    }
                }
                let bindings = [
                    &self.shortcuts.undo,
                    &self.shortcuts.redo,
                    &self.shortcuts.save,
                    &self.shortcuts.copy,
                    &self.shortcuts.cut,
                    &self.shortcuts.paste,
                    &self.shortcuts.select_all,
                    &self.shortcuts.deselect,
                ];
                for key in ["E", "T"] {
                    if bindings.iter().any(|s| s.trim().eq_ignore_ascii_case(key)) {
                        conflict_labels.push(format!("Ctrl+{key}"));
                    }
                }
                conflict_labels.sort();
                if !conflict_labels.is_empty() {
                    ui.colored_label(
                        Color32::YELLOW,
                        if self.language_english {
                            format!(
                                "Conflicting shortcuts (configured commands take priority): {}",
                                conflict_labels.join(", ")
                            )
                        } else {
                            format!(
                                "重複キー（設定したコマンドを優先）: {}",
                                conflict_labels.join("、")
                            )
                        },
                    );
                }
            },
        );
        ui.collapsing(self.text("応答速度（このセッション）", "Latency (This Session)"), |ui| {
                    ui.label(self.text("ペン入力から前フレームのGPUキュー完了通知まで。画面走査表示時間は含みません。", "Time from pen input until prior-frame GPU queue completion; excludes display scan-out."));
                    ui.label(self.text("Windows Ink / WinTabはフック受信時刻、通常入力はUI受信時刻から計測します。", "Starts at Windows Ink / WinTab hook receipt, or at UI receipt for standard input."));
                    if let Some(latest) = self.last_canvas_latency_ms {
                        let mut sorted = self
                            .canvas_latency_samples_ms
                            .iter()
                            .copied()
                            .collect::<Vec<_>>();
                        sorted.sort_by(f32::total_cmp);
                        let mean = sorted.iter().sum::<f32>() / sorted.len().max(1) as f32;
                        let p95_index = ((sorted.len() as f32 * 0.95).ceil() as usize)
                            .saturating_sub(1)
                            .min(sorted.len() - 1);
                        ui.label(format!(
                            "最新 {:.2} ms / 平均 {:.2} ms / P95 {:.2} ms（{} samples）",
                            latest,
                            mean,
                            sorted[p95_index],
                            sorted.len()
                        ));
                        if ui.button(self.text("計測ログをJSON保存", "Save Metrics as JSON")).clicked()
                            && let Some(path) = rfd::FileDialog::new()
                                .add_filter("Latency log", &["json"])
                                .set_file_name("efude-latency.json")
                                .save_file()
                            {
                                let log = serde_json::json!({
                                    "metric": "input_to_gpu_queue_completion_ms",
                                    "note": "Measured asynchronously until completion of GPU work submitted for the frame after input; excludes display scan-out.",
                                    "samples_ms": self.canvas_latency_samples_ms.iter().copied().collect::<Vec<_>>(),
                                });
                                self.status = match std::fs::write(
                                    path,
                                    serde_json::to_vec_pretty(&log).unwrap_or_default(),
                                ) {
                                    Ok(()) => "応答速度ログを保存しました".into(),
                                    Err(error) => format!("応答速度ログを保存できません: {error}"),
                                };
                            }
                    } else {
                        ui.label(self.text("キャンバス上でペンまたはブラシを動かすと計測します。", "Move a pen or brush on the canvas to record measurements."));
                    }
                });
        ui.collapsing(
            self.text("自動バックアップ", "Automatic Backups"),
            |ui| {
                ui.add(
                    egui::Slider::new(&mut self.backup_interval_minutes, 1..=120).text(
                        if self.language_english {
                            "Interval (min)"
                        } else {
                            "間隔（分）"
                        },
                    ),
                );
                ui.add(
                    egui::Slider::new(&mut self.backup_generations, 1..=100).text(
                        if self.language_english {
                            "Generations to Keep"
                        } else {
                            "保持世代数"
                        },
                    ),
                );
            },
        );
    }

    /// Navigator thumbnail.
    #[allow(unused_variables)]
    pub(crate) fn navigator_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        ui.collapsing(self.text("ナビゲーター", "Navigator"), |ui| {
            let page = Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(self.doc.width as f32, self.doc.height as f32),
            );
            let oversized = self.needs_viewport_texture(ctx);
            let mut scale = ui.available_width() / page.width();
            if oversized {
                let height = ui.available_height().min(ctx.screen_rect().height());
                scale = scale.min(height / page.height());
            }
            let display_size = (page.size() * scale).max(Vec2::splat(1.));
            let region = oversized.then(|| {
                shape_gradient::PreviewRegion::new(
                    page,
                    scale * ctx.pixels_per_point(),
                    self.max_texture_side(ctx),
                )
            });
            let size = region.map_or([self.doc.width as usize, self.doc.height as usize], |r| {
                r.size
            });
            if self.navigator_texture_dirty
                || self
                    .navigator_texture
                    .as_ref()
                    .is_none_or(|t| t.size() != size)
            {
                let image = if let Some(region) = region {
                    self.display_region(region)
                } else {
                    let merged = efude_canvas::composite_display(&self.doc, self.display_checker());
                    egui::ColorImage::from_rgba_unmultiplied(size, &merged)
                };
                if let Some(tex) = &mut self.navigator_texture {
                    tex.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.navigator_texture =
                        Some(ctx.load_texture("navigator", image, egui::TextureOptions::LINEAR));
                }
                self.navigator_texture_dirty = false;
            }
            if let Some(tex) = &self.navigator_texture {
                let (response, painter) =
                    ui.allocate_painter(display_size, egui::Sense::click_and_drag());
                let r = response.rect;
                painter.image(
                    tex.id(),
                    r,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                    Color32::WHITE,
                );
                let (viewport, fit, _) = self.canvas_viewport;
                let available = if viewport.is_positive() {
                    viewport.size()
                } else {
                    ctx.screen_rect().size() - Vec2::new(390., 100.)
                };
                let scale = fit * self.zoom;
                let fw = (available.x / (self.doc.width as f32 * scale)).clamp(0., 1.);
                let fh = (available.y / (self.doc.height as f32 * scale)).clamp(0., 1.);
                let center = Pos2::new(
                    r.left() + self.navigator_center.x / self.doc.width as f32 * r.width(),
                    r.top() + self.navigator_center.y / self.doc.height as f32 * r.height(),
                );
                painter.rect_stroke(
                    Rect::from_center_size(center, Vec2::new(r.width() * fw, r.height() * fh)),
                    0.,
                    Stroke::new(1.5, Color32::YELLOW),
                    egui::StrokeKind::Inside,
                );
                if (response.clicked() || response.dragged())
                    && response.interact_pointer_pos().is_some()
                {
                    let p = response.interact_pointer_pos().unwrap();
                    self.navigator_center = Vec2::new(
                        ((p.x - r.left()) / r.width() * self.doc.width as f32)
                            .clamp(0., self.doc.width as f32),
                        ((p.y - r.top()) / r.height() * self.doc.height as f32)
                            .clamp(0., self.doc.height as f32),
                    );
                }
            }
        });
    }

    /// Options of the selection, fill, ruler and eyedropper tools.
    #[allow(unused_variables)]
    pub(crate) fn tool_options_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        if matches!(self.tool, Tool::Balloon | Tool::Text) {
            ui.label(if self.tool == Tool::Balloon {
                self.text(
                    "ドラッグでフキダシを作成、クリックで文字に合わせた大きさで作成します。フキダシをドラッグで移動、右下の□で大きさ、○でしっぽの先を動かします。Ctrl+ドラッグでしっぽを追加。",
                    "Drag to create a balloon, or click for one sized to its text. Drag a balloon to move it, the square to resize, a circle to move a tail tip. Ctrl-drag adds a tail.",
                )
            } else {
                self.text(
                    "クリックした所に文字を置きます。文字をドラッグで移動します。",
                    "Click to place text. Drag text to move it.",
                )
            });
            let english = self.language_english;
            if self.tool == Tool::Balloon {
                let name = |shape: efude_comic::BalloonShape| match (shape, english) {
                    (efude_comic::BalloonShape::Ellipse, false) => "楕円",
                    (efude_comic::BalloonShape::Ellipse, true) => "Ellipse",
                    (efude_comic::BalloonShape::RoundedRect, false) => "角丸",
                    (efude_comic::BalloonShape::RoundedRect, true) => "Rounded box",
                    (efude_comic::BalloonShape::Cloud, false) => "雲（考え）",
                    (efude_comic::BalloonShape::Cloud, true) => "Cloud (thought)",
                    (efude_comic::BalloonShape::Flash, false) => "トゲ（叫び）",
                    (efude_comic::BalloonShape::Flash, true) => "Spiky (shout)",
                    (efude_comic::BalloonShape::None, false) => "なし",
                    (efude_comic::BalloonShape::None, true) => "None",
                };
                egui::ComboBox::from_label(self.text("形", "Shape"))
                    .selected_text(name(self.balloon_ui.new_shape))
                    .show_ui(ui, |ui| {
                        for shape in efude_comic::BalloonShape::ALL.into_iter().skip(1) {
                            ui.selectable_value(&mut self.balloon_ui.new_shape, shape, name(shape));
                        }
                    });
            }
            ui.horizontal(|ui| {
                ui.label(if english {
                    "Text size"
                } else {
                    "文字サイズ"
                });
                ui.add(
                    egui::DragValue::new(&mut self.balloon_ui.new_points)
                        .range(3.0..=200.0)
                        .speed(0.1)
                        .suffix(" pt"),
                );
                ui.selectable_value(
                    &mut self.balloon_ui.new_vertical,
                    true,
                    if english { "Vertical" } else { "縦書き" },
                );
                ui.selectable_value(
                    &mut self.balloon_ui.new_vertical,
                    false,
                    if english { "Horizontal" } else { "横書き" },
                );
            });
        }
        if self.tool == Tool::PanelSplit {
            ui.label(self.text(
                "コマの上を横切るようにドラッグすると、その線で分割します。Shift で45°刻み。",
                "Drag across a panel to split it along the line. Shift snaps to 45°.",
            ));
            if self.comic_doc().is_none() {
                ui.colored_label(
                    Color32::from_rgb(255, 205, 110),
                    self.text(
                        "漫画 → 原稿の設定 で漫画原稿を作ってから使います。",
                        "Create a manga page first (Manga → Page Setup).",
                    ),
                );
            } else if ui
                .button(self.text("コマの設定…", "Borders and Gutters…"))
                .clicked()
            {
                self.comic_ui.panel_settings_open = true;
            }
        }
        if self.tool == Tool::BezierRuler {
            ui.label(if self.language_english {
                format!(
                    "Curve points: {}/4 (start, two handles, end)",
                    self.bezier_points.len()
                )
            } else {
                format!(
                    "曲線点: {}/4（始点・制御点2つ・終点）",
                    self.bezier_points.len()
                )
            });
            if ui
                .button(self.text("曲線点を取消", "Clear Curve Points"))
                .clicked()
            {
                self.bezier_points.clear();
            }
            if !self.bezier_points.is_empty()
                && ui
                    .button(self.text("最後の点を戻す", "Undo Last Point"))
                    .clicked()
            {
                self.bezier_points.pop();
            }
        }
        if self.tool == Tool::PerspectiveRuler {
            if ui
                .button(if self.language_english {
                    if self.setting_vanishing_point {
                        "Click canvas to set vanishing point"
                    } else {
                        "Add Vanishing Point"
                    }
                } else if self.setting_vanishing_point {
                    "キャンバスをクリックして消失点を設定"
                } else {
                    "消失点を追加"
                })
                .clicked()
            {
                self.setting_vanishing_point = true;
            }
            ui.label(if self.language_english {
                format!("Vanishing points: {} / 3", self.perspective_points.len())
            } else {
                format!("消失点: {} / 3", self.perspective_points.len())
            });
            if !self.perspective_points.is_empty() {
                ui.add(
                    egui::Slider::new(
                        &mut self.perspective_selected,
                        0..=self.perspective_points.len() - 1,
                    )
                    .text(if self.language_english {
                        "Vanishing Point"
                    } else {
                        "使用する消失点"
                    }),
                );
            }
            if ui
                .button(self.text("消失点を消去", "Clear Vanishing Points"))
                .clicked()
            {
                self.perspective_points.clear();
                self.setting_vanishing_point = false;
            }
        }
        if matches!(
            self.tool,
            Tool::RectangleSelect
                | Tool::EllipseSelect
                | Tool::LassoSelect
                | Tool::PolygonSelect
                | Tool::MagicWand
                | Tool::ColorRange
                | Tool::SelectionBrush
                | Tool::QuickMask
        ) && ui.button(self.text("選択解除", "Deselect")).clicked()
        {
            self.change_selection(|selection, _, _| selection.clear());
        }
        if self.tool == Tool::Eyedropper {
            ui.checkbox(
                &mut self.eyedropper_composite,
                if self.language_english {
                    "Eyedropper: Sample Composite"
                } else {
                    "スポイト: 表示画像を合成"
                },
            );
            ui.add(egui::Slider::new(&mut self.eyedropper_radius, 0..=64).text(
                if self.language_english {
                    "Eyedropper Average Radius"
                } else {
                    "スポイト平均半径"
                },
            ));
        }
        if self.tool == Tool::PolygonSelect {
            ui.label(if self.language_english {
                format!(
                    "Vertices: {} (press Enter to finish)",
                    self.selection_points.len()
                )
            } else {
                format!("頂点: {}（Enterで確定）", self.selection_points.len())
            });
            if ui
                .button(self.text("多角形選択を確定", "Finish Polygon Selection"))
                .clicked()
            {
                self.selection
                    .polygon(self.doc.width, self.doc.height, &self.selection_points);
                self.apply_selection_symmetry();
                self.finish_selection_operation();
                self.selection_points.clear();
                self.status = self
                    .text("多角形選択を確定しました", "Polygon selection completed")
                    .into();
            }
            if ui
                .button(self.text("頂点を取消", "Clear Vertices"))
                .clicked()
            {
                self.selection_points.clear();
            }
        }
        if matches!(
            self.tool,
            Tool::RectangleSelect
                | Tool::EllipseSelect
                | Tool::LassoSelect
                | Tool::PolygonSelect
                | Tool::MagicWand
                | Tool::ColorRange
                | Tool::SelectionBrush
                | Tool::QuickMask
        ) {
            ui.label(self.text(
                "Shift: 追加 / Ctrl: 減算 / Shift+Ctrl: 交差",
                "Shift: add / Ctrl: subtract / Shift+Ctrl: intersect",
            ));
            ui.add(egui::Slider::new(&mut self.selection_radius, 1..=64).text(
                if self.language_english {
                    "Selection Border Width"
                } else {
                    "選択境界幅"
                },
            ));
            ui.horizontal(|ui| {
                if ui
                    .button(self.text("選択反転", "Invert Selection"))
                    .clicked()
                {
                    self.change_selection(|selection, width, height| {
                        selection.invert(width, height)
                    });
                }
                if ui.button(self.text("拡張", "Expand")).clicked() {
                    let radius = self.selection_radius;
                    self.change_selection(|selection, width, height| {
                        selection.expand(width, height, radius)
                    });
                }
                if ui.button(self.text("縮小", "Shrink")).clicked() {
                    let radius = self.selection_radius;
                    self.change_selection(|selection, width, height| {
                        selection.shrink(width, height, radius)
                    });
                }
            });
            ui.add(
                egui::Slider::new(&mut self.selection_feather_radius, 1..=64).text(
                    if self.language_english {
                        "Feather (px)"
                    } else {
                        "フェザー幅(px)"
                    },
                ),
            );
            if ui
                .button(self.text("選択境界をぼかす", "Feather Selection"))
                .clicked()
            {
                let radius = self.selection_feather_radius;
                self.change_selection(|selection, width, height| {
                    selection.feather(width, height, radius)
                });
            }
        }
        if matches!(self.tool, Tool::SelectionBrush | Tool::QuickMask) {
            ui.checkbox(
                &mut self.selection_erase,
                if self.language_english {
                    if self.tool == Tool::QuickMask {
                        "Quick Mask: Erase"
                    } else {
                        "Selection Brush: Erase"
                    }
                } else if self.tool == Tool::QuickMask {
                    "クイックマスク: 消去"
                } else {
                    "選択範囲ペン: 消去"
                },
            );
        }
        if matches!(self.tool, Tool::MagicWand | Tool::ColorRange) {
            egui::ComboBox::from_label(if self.language_english {
                "Selection Reference"
            } else {
                "選択範囲の参照元"
            })
            .selected_text(match self.selection_reference_mode {
                1 => {
                    if self.language_english {
                        "Reference Layers"
                    } else {
                        "参照レイヤー"
                    }
                }
                2 => {
                    if self.language_english {
                        "All Visible Layers"
                    } else {
                        "表示レイヤーすべて"
                    }
                }
                3 => self
                    .doc
                    .layers
                    .iter()
                    .find(|layer| layer.id == self.selection_reference_layer)
                    .map(|layer| layer.name.as_str())
                    .unwrap_or(if self.language_english {
                        "Specific Layer"
                    } else {
                        "指定レイヤー"
                    }),
                _ => {
                    if self.language_english {
                        "Selected Layer"
                    } else {
                        "選択レイヤー"
                    }
                }
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.selection_reference_mode,
                    0,
                    if self.language_english {
                        "Selected Layer"
                    } else {
                        "選択レイヤー"
                    },
                );
                ui.selectable_value(
                    &mut self.selection_reference_mode,
                    1,
                    if self.language_english {
                        "Reference Layers"
                    } else {
                        "参照レイヤー"
                    },
                );
                ui.selectable_value(
                    &mut self.selection_reference_mode,
                    2,
                    if self.language_english {
                        "All Visible Layers"
                    } else {
                        "表示レイヤーすべて"
                    },
                );
                for layer in self
                    .doc
                    .layers
                    .iter()
                    .filter(|layer| layer.kind == LayerKind::Raster)
                {
                    if ui
                        .selectable_value(
                            &mut self.selection_reference_mode,
                            3,
                            if self.language_english {
                                format!("Layer: {}", layer.name)
                            } else {
                                format!("レイヤー: {}", layer.name)
                            },
                        )
                        .clicked()
                    {
                        self.selection_reference_layer = layer.id;
                    }
                }
            });
        }
        if self.tool == Tool::Fill {
            ui.add(egui::Slider::new(&mut self.fill_tolerance, 0..=96).text(
                if self.language_english {
                    "Fill Tolerance"
                } else {
                    "塗りつぶし許容値"
                },
            ));
            ui.add(egui::Slider::new(&mut self.fill_gap_close, 0..=8).text(
                if self.language_english {
                    "Close Gaps (px)"
                } else {
                    "隙間閉じ(px)"
                },
            ));
            egui::ComboBox::from_label(if self.language_english {
                "Fill Reference"
            } else {
                "塗りつぶしの参照元"
            })
            .selected_text(match self.fill_reference_mode {
                1 => {
                    if self.language_english {
                        "Reference Layers"
                    } else {
                        "参照レイヤー"
                    }
                }
                2 => {
                    if self.language_english {
                        "All Visible Layers"
                    } else {
                        "表示レイヤーすべて"
                    }
                }
                3 => self
                    .doc
                    .layers
                    .iter()
                    .find(|layer| layer.id == self.fill_reference_layer)
                    .map(|layer| layer.name.as_str())
                    .unwrap_or(if self.language_english {
                        "Specific Layer"
                    } else {
                        "指定レイヤー"
                    }),
                _ => {
                    if self.language_english {
                        "Selected Layer"
                    } else {
                        "選択レイヤー"
                    }
                }
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.fill_reference_mode,
                    0,
                    if self.language_english {
                        "Selected Layer"
                    } else {
                        "選択レイヤー"
                    },
                );
                ui.selectable_value(
                    &mut self.fill_reference_mode,
                    1,
                    if self.language_english {
                        "Reference Layers"
                    } else {
                        "参照レイヤー"
                    },
                );
                ui.selectable_value(
                    &mut self.fill_reference_mode,
                    2,
                    if self.language_english {
                        "All Visible Layers"
                    } else {
                        "表示レイヤーすべて"
                    },
                );
                for layer in self
                    .doc
                    .layers
                    .iter()
                    .filter(|layer| layer.kind == LayerKind::Raster)
                {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_value(
                                &mut self.fill_reference_mode,
                                3,
                                if self.language_english {
                                    format!("Layer: {}", layer.name)
                                } else {
                                    format!("レイヤー: {}", layer.name)
                                },
                            )
                            .clicked()
                        {
                            self.fill_reference_layer = layer.id;
                        }
                    });
                }
            });
        }
        if matches!(
            self.tool,
            Tool::RectangleSelect
                | Tool::EllipseSelect
                | Tool::LassoSelect
                | Tool::PolygonSelect
                | Tool::MagicWand
                | Tool::ColorRange
                | Tool::SelectionBrush
                | Tool::QuickMask
                | Tool::Move
        ) {
            ui.horizontal(|ui| {
                if ui.button(self.text("コピー", "Copy")).clicked() {
                    self.copy_selection(false);
                }
                if ui.button(self.text("切り取り", "Cut")).clicked() {
                    self.copy_selection(true);
                }
                if ui
                    .add_enabled(
                        !self.editing_mask,
                        egui::Button::new(self.text("貼り付け", "Paste")),
                    )
                    .clicked()
                {
                    self.paste_clipboard(ctx);
                }
            });
        }
        if matches!(self.tool, Tool::MagicWand | Tool::ColorRange) {
            ui.add(
                egui::Slider::new(&mut self.selection_tolerance, 0..=96).text(
                    if self.language_english {
                        "Color Tolerance"
                    } else {
                        "色許容値"
                    },
                ),
            );
        }
        if matches!(
            self.tool,
            Tool::Brush | Tool::Eraser | Tool::Blur | Tool::Smudge
        ) {
            ui.label(
                egui::RichText::new(self.text(
                    "ブラシの種類と設定は「ブラシ」タブにあります。",
                    "Brush presets and settings are on the Brush tab.",
                ))
                .color(crate::layout::MUTED_TEXT),
            );
        }
        if matches!(self.tool, Tool::Pan | Tool::Line | Tool::EllipseRuler) {
            ui.label(
                egui::RichText::new(self.text(
                    "このツールに設定項目はありません。",
                    "This tool has no options.",
                ))
                .color(crate::layout::MUTED_TEXT),
            );
        }
    }

    /// Flip, rotate, scale and mesh transforms of the selected layer.
    #[allow(unused_variables)]
    pub(crate) fn transform_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        ui.horizontal(|ui| {
            if ui
                .button(self.text("左右反転", "Flip Horizontal"))
                .clicked()
            {
                self.apply_transform(-1.0, 1.0, 0.0);
            }
            if ui.button(self.text("上下反転", "Flip Vertical")).clicked() {
                self.apply_transform(1.0, -1.0, 0.0);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("↶ 15°").clicked() {
                self.apply_transform(1.0, 1.0, -15f32.to_radians());
            }
            if ui.button("↷ 15°").clicked() {
                self.apply_transform(1.0, 1.0, 15f32.to_radians());
            }
        });
        ui.horizontal(|ui| {
            if ui.button(self.text("縮小 90%", "Scale 90%")).clicked() {
                self.apply_transform(0.9, 0.9, 0.0);
            }
            if ui.button(self.text("拡大 110%", "Scale 110%")).clicked() {
                self.apply_transform(1.1, 1.1, 0.0);
            }
        });
        ui.label(self.text("自由変形", "Free Transform"));
        ui.add(
            egui::Slider::new(&mut self.transform_scale_x, 0.1..=4.0).text(
                if self.language_english {
                    "Scale X"
                } else {
                    "横倍率"
                },
            ),
        );
        ui.add(
            egui::Slider::new(&mut self.transform_scale_y, 0.1..=4.0).text(
                if self.language_english {
                    "Scale Y"
                } else {
                    "縦倍率"
                },
            ),
        );
        ui.add(
            egui::Slider::new(&mut self.transform_angle, -180.0..=180.0).text(
                if self.language_english {
                    "Angle"
                } else {
                    "角度"
                },
            ),
        );
        if ui
            .button(self.text("変形を適用", "Apply Transform"))
            .clicked()
        {
            self.apply_transform(
                self.transform_scale_x,
                self.transform_scale_y,
                self.transform_angle.to_radians(),
            );
        }
        ui.collapsing(
            self.text(
                "メッシュ変形（4×4制御点）",
                "Mesh Transform (4×4 Control Points)",
            ),
            |ui| {
                for i in 0..self.mesh_offsets.len() {
                    ui.label(if self.language_english {
                        format!("Control Point ({}, {})", i % 4 + 1, i / 4 + 1)
                    } else {
                        format!("制御点 ({}, {})", i % 4 + 1, i / 4 + 1)
                    });
                    ui.add(
                        egui::Slider::new(&mut self.mesh_offsets[i][0], -200.0..=200.0).text(
                            if self.language_english {
                                "Move X (px)"
                            } else {
                                "横移動(px)"
                            },
                        ),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.mesh_offsets[i][1], -200.0..=200.0).text(
                            if self.language_english {
                                "Move Y (px)"
                            } else {
                                "縦移動(px)"
                            },
                        ),
                    );
                }
                if ui
                    .button(self.text("メッシュ変形を適用", "Apply Mesh Transform"))
                    .clicked()
                {
                    self.apply_mesh_warp();
                }
            },
        );
    }

    /// Colour wheel, sliders and palette.
    /// The selected brush's settings in their own window, with the 書き味
    /// (stroke feel) options on top.
    pub(crate) fn brush_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_brush_settings {
            return;
        }
        let english = self.language_english;
        let mut open = true;
        let title = format!(
            "{} — {}",
            if english {
                "Brush Settings"
            } else {
                "ブラシの詳細設定"
            },
            crate::layout::brush_label(&self.brushes[self.selected_brush].name, english)
        );
        egui::Window::new(title)
            .id(egui::Id::new("brush-settings-window"))
            .open(&mut open)
            .resizable(true)
            .default_width(360.0)
            .default_height(560.0)
            .default_pos(ctx.screen_rect().right_top() + Vec2::new(-700.0, 80.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("brush-settings-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(if english { "Name" } else { "名前" });
                            let brush = &mut self.brushes[self.selected_brush];
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut brush.name)
                                        .desired_width(200.0),
                                )
                                .changed()
                            {
                                brush.name = brush.name.chars().take(64).collect();
                            }
                        });
                        ui.label(
                            egui::RichText::new(if english { "Stroke feel" } else { "書き味" })
                                .strong(),
                        );
                        self.brush_feel_toggles(ui);
                        ui.separator();
                        self.brush_settings_ui(ui, ctx);
                    });
            });
        self.show_brush_settings = open;
    }

    /// The main switches of the selected brush: settling, wet edge, what
    /// pressure changes and anti-aliasing.
    pub(crate) fn brush_feel_toggles(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let brush = &mut self.brushes[self.selected_brush];
        ui.checkbox(
            &mut brush.settle,
            if english {
                "Settle strokes after drawing (tip preview, end taper, wet edge)"
            } else {
                "描いたあとで線を整える（先端の仮表示・入り抜き・にじみ縁）"
            },
        )
        .on_hover_text(if english {
            "Off: every part of a line is final the moment it appears; nothing spreads or changes afterwards. The end taper and the wet edge at pen-up are not applied."
        } else {
            "オフ: 線は描いたその場で確定し、あとから広がったり変わったりしません。抜き（終わりの入り抜き）とペンを離したときのにじみ縁は付きません。"
        });
        ui.checkbox(
            &mut brush.wet_edge_on,
            if english { "Wet edge" } else { "にじみ縁" },
        );
        ui.horizontal_wrapped(|ui| {
            let mut size = brush.size_source == DynamicSource::Pressure;
            if ui
                .checkbox(
                    &mut size,
                    if english {
                        "Pressure → size"
                    } else {
                        "筆圧で太さ"
                    },
                )
                .changed()
            {
                brush.size_source = if size {
                    DynamicSource::Pressure
                } else {
                    DynamicSource::None
                };
            }
            let mut opacity = brush.opacity_source == DynamicSource::Pressure;
            if ui
                .checkbox(
                    &mut opacity,
                    if english {
                        "Pressure → density"
                    } else {
                        "筆圧で濃さ"
                    },
                )
                .changed()
            {
                brush.opacity_source = if opacity {
                    DynamicSource::Pressure
                } else {
                    DynamicSource::None
                };
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(if english {
                "Anti-aliasing"
            } else {
                "アンチエイリアス"
            });
            let names: [&str; 4] = if english {
                ["None", "Light", "Normal", "Strong"]
            } else {
                ["なし", "弱", "中", "強"]
            };
            for (level, name) in names.iter().enumerate() {
                ui.selectable_value(&mut brush.antialias, level as u8, *name);
            }
        });
    }

    /// Every setting of the selected brush.
    #[allow(unused_variables)]
    pub(crate) fn brush_settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let brush = &mut self.brushes[self.selected_brush];
        let english = self.language_english;
        for (value, ja, en) in [
            (&mut brush.hue_jitter, "色相ジッター", "Hue jitter"),
            (
                &mut brush.saturation_jitter,
                "彩度ジッター",
                "Saturation jitter",
            ),
            (
                &mut brush.brightness_jitter,
                "明度ジッター",
                "Brightness jitter",
            ),
        ] {
            ui.add(egui::Slider::new(value, 0.0..=1.0).text(if english { en } else { ja }));
        }
        ui.checkbox(
            &mut brush.follow_stroke_direction,
            if english {
                "Follow stroke direction"
            } else {
                "ストロークの向きに追従"
            },
        );
        brush.size = self.size;
        ui.add(
            egui::Slider::new(&mut brush.opacity, 0.01..=1.).text(if english {
                "Brush Opacity"
            } else {
                "ブラシ不透明度"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.spacing, 0.005..=1.).text(if english {
                "Spacing"
            } else {
                "間隔"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.stabilization, 0..=15).text(if english {
                "Stabilization"
            } else {
                "手ブレ補正"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.pull_distance, 0.0..=80.0).text(if english {
                "Pull Distance"
            } else {
                "紐引き距離"
            }),
        );
        ui.label(if english {
            "Cyan guides show the input and stabilized positions while drawing."
        } else {
            "補正中はシアンの線で入力位置と補正位置を表示します。"
        });
        ui.add(
            egui::Slider::new(&mut brush.speed_stabilization, 0.0..=1.0).text(if english {
                "Speed Adaptation"
            } else {
                "速度適応"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.pressure_curve, 0.2..=3.0).text(if english {
                "Pressure Curve"
            } else {
                "筆圧カーブ"
            }),
        );
        ui.label(if english {
            "Brush Pressure Curve (drag the points)"
        } else {
            "ブラシの筆圧カーブ（点をドラッグ）"
        });
        let mut points = [
            brush.pressure_curve_x1,
            brush.pressure_curve_y1,
            brush.pressure_curve_x2,
            brush.pressure_curve_y2,
        ];
        let gamma = brush.pressure_curve;
        if pressure_curve_editor(ui, "brush-pressure-curve", &mut points, gamma, None) {
            [
                brush.pressure_curve_x1,
                brush.pressure_curve_y1,
                brush.pressure_curve_x2,
                brush.pressure_curve_y2,
            ] = points;
        }
        if ui
            .small_button(if english {
                "Reset to linear"
            } else {
                "直線に戻す"
            })
            .clicked()
        {
            brush.pressure_curve = 1.0;
            [
                brush.pressure_curve_x1,
                brush.pressure_curve_y1,
                brush.pressure_curve_x2,
                brush.pressure_curve_y2,
            ] = [0.25, 0.25, 0.75, 0.75];
        }
        ui.checkbox(
            &mut brush.taper_in_pixels,
            if english {
                "Set taper length in pixels"
            } else {
                "入り抜き長をpxで指定"
            },
        );
        let taper_range = if brush.taper_in_pixels {
            0.0..=512.0
        } else {
            0.0..=0.5
        };
        ui.add(
            egui::Slider::new(&mut brush.taper_start, taper_range.clone()).text(if english {
                "Taper In"
            } else {
                "入り抜き: 開始"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.taper_end, taper_range).text(if english {
                "Taper Out"
            } else {
                "入り抜き: 終了"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.taper_min, 0.01..=1.0).text(if english {
                "Minimum Size"
            } else {
                "最小幅"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.mix.blend, 0.0..=1.0).text(if english {
                "Color Mixing"
            } else {
                "混色"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.mix.dilution, 0.0..=1.0).text(if english {
                "Dilution"
            } else {
                "水分量"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.mix.persistence, 0.0..=1.0).text(if english {
                "Color Persistence"
            } else {
                "色延び"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.mix.charge, 0.0..=1.0).text(if english {
                "Paint Charge"
            } else {
                "絵の具チャージ"
            }),
        );
        let bleed_label = |bleed: efude_brush::BleedStyle| match bleed {
            efude_brush::BleedStyle::Airy => {
                if english {
                    "Airy"
                } else {
                    "ふんわり"
                }
            }
            efude_brush::BleedStyle::Balanced => {
                if english {
                    "Balanced"
                } else {
                    "ふつう"
                }
            }
            efude_brush::BleedStyle::Dense => {
                if english {
                    "Dense"
                } else {
                    "しっかり"
                }
            }
        };
        egui::ComboBox::from_label(if english { "Bleed" } else { "にじみ方" })
            .selected_text(bleed_label(brush.bleed))
            .show_ui(ui, |ui| {
                for bleed in [
                    efude_brush::BleedStyle::Airy,
                    efude_brush::BleedStyle::Balanced,
                    efude_brush::BleedStyle::Dense,
                ] {
                    ui.selectable_value(&mut brush.bleed, bleed, bleed_label(bleed));
                }
            });
        ui.checkbox(
            &mut brush.wet_edge_on,
            if english { "Wet Edge" } else { "にじみ縁" },
        );
        ui.add_enabled(
            brush.wet_edge_on,
            egui::Slider::new(&mut brush.wet_edge, 0.0..=1.0).text(if english {
                "Wet Edge Strength"
            } else {
                "にじみ縁の強さ"
            }),
        );
        ui.add_enabled(
            brush.wet_edge_on,
            egui::Slider::new(&mut brush.wet_edge_width, 1.0..=32.0).text(if english {
                "Wet Edge Width"
            } else {
                "にじみ縁の幅"
            }),
        );
        if ui
            .button(if english {
                "Reset This Brush"
            } else {
                "このブラシを初期設定に戻す"
            })
            .clicked()
            && let Some(default) = default_brush_like(brush)
        {
            *brush = default;
            self.size = brush.size;
        }
        egui::ComboBox::from_label(if english {
            "Color Sample"
        } else {
            "混色サンプル"
        })
        .selected_text(match brush.mix.sample_range {
            SampleRange::Center => {
                if english {
                    "Center"
                } else {
                    "中心色"
                }
            }
            SampleRange::Average => {
                if english {
                    "Average"
                } else {
                    "周辺平均"
                }
            }
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut brush.mix.sample_range,
                SampleRange::Center,
                if english { "Center" } else { "中心色" },
            );
            ui.selectable_value(
                &mut brush.mix.sample_range,
                SampleRange::Average,
                if english { "Average" } else { "周辺平均" },
            );
        });
        egui::ComboBox::from_label(if english {
            "Reference Layers"
        } else {
            "色の参照先"
        })
        .selected_text(match brush.mix.reference_target {
            ReferenceTarget::CurrentLayer => {
                if english {
                    "Current Layer"
                } else {
                    "現在のレイヤー"
                }
            }
            ReferenceTarget::BelowSelected => {
                if english {
                    "All Layers Below"
                } else {
                    "下のレイヤーすべて"
                }
            }
            ReferenceTarget::VisibleLayers => {
                if english {
                    "All Visible Layers"
                } else {
                    "表示レイヤー全体"
                }
            }
            ReferenceTarget::Layer(layer_id) => self
                .doc
                .layers
                .iter()
                .find(|layer| layer.id == layer_id)
                .map(|layer| layer.name.as_str())
                .unwrap_or(if english {
                    "Selected Layer"
                } else {
                    "指定レイヤー"
                }),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut brush.mix.reference_target,
                ReferenceTarget::CurrentLayer,
                if english {
                    "Current Layer"
                } else {
                    "現在のレイヤー"
                },
            );
            ui.selectable_value(
                &mut brush.mix.reference_target,
                ReferenceTarget::BelowSelected,
                if english {
                    "All Layers Below"
                } else {
                    "下のレイヤーすべて"
                },
            );
            ui.selectable_value(
                &mut brush.mix.reference_target,
                ReferenceTarget::VisibleLayers,
                if english {
                    "All Visible Layers"
                } else {
                    "表示レイヤー全体"
                },
            );
            for layer in self
                .doc
                .layers
                .iter()
                .filter(|layer| layer.kind == LayerKind::Raster)
            {
                ui.selectable_value(
                    &mut brush.mix.reference_target,
                    ReferenceTarget::Layer(layer.id),
                    if english {
                        format!("Layer: {}", layer.name)
                    } else {
                        format!("レイヤー: {}", layer.name)
                    },
                );
            }
        });
        ui.add(
            egui::Slider::new(&mut brush.mix.sample_radius, 0.0..=64.0).text(if english {
                "Sample Radius"
            } else {
                "サンプル半径"
            }),
        );
        ui.collapsing(
            if english {
                "Input Dynamics"
            } else {
                "入力ダイナミクス"
            },
            |ui| {
                dynamic_source_control(
                    ui,
                    if english {
                        "Size Input"
                    } else {
                        "サイズ入力"
                    },
                    &mut brush.size_source,
                    english,
                );
                ui.add(
                    egui::Slider::new(&mut brush.size_min, 0.0..=1.0).text(if english {
                        "Minimum Size"
                    } else {
                        "サイズ最小値"
                    }),
                );
                dynamic_source_control(
                    ui,
                    if english {
                        "Opacity Input"
                    } else {
                        "不透明度入力"
                    },
                    &mut brush.opacity_source,
                    english,
                );
                ui.add(
                    egui::Slider::new(&mut brush.opacity_min, 0.0..=1.0).text(if english {
                        "Minimum Opacity"
                    } else {
                        "不透明度最小値"
                    }),
                );
                dynamic_source_control(
                    ui,
                    if english {
                        "Concentration Input"
                    } else {
                        "濃度入力"
                    },
                    &mut brush.concentration_source,
                    english,
                );
                ui.add(
                    egui::Slider::new(&mut brush.concentration_min, 0.0..=1.0).text(if english {
                        "Minimum Concentration"
                    } else {
                        "濃度最小値"
                    }),
                );
                dynamic_source_control(
                    ui,
                    if english {
                        "Mix Input"
                    } else {
                        "混色量入力"
                    },
                    &mut brush.mix_source,
                    english,
                );
                ui.add(
                    egui::Slider::new(&mut brush.mix_min, 0.0..=1.0).text(if english {
                        "Minimum Mix"
                    } else {
                        "混色量最小値"
                    }),
                );
                dynamic_source_control(
                    ui,
                    if english {
                        "Dilution Input"
                    } else {
                        "水分量入力"
                    },
                    &mut brush.dilution_source,
                    english,
                );
                ui.add(
                    egui::Slider::new(&mut brush.dilution_min, 0.0..=1.0).text(if english {
                        "Minimum Dilution"
                    } else {
                        "水分量最小値"
                    }),
                );
            },
        );
        ui.add(
            egui::Slider::new(&mut brush.speed_size, -1.0..=1.0).text(if english {
                "Speed → Size"
            } else {
                "速度→サイズ"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.speed_opacity, -1.0..=1.0).text(if english {
                "Speed → Opacity"
            } else {
                "速度→不透明度"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.tilt_size, -1.0..=1.0).text(if english {
                "Tilt → Size"
            } else {
                "傾き→サイズ"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.tilt_opacity, -1.0..=1.0).text(if english {
                "Tilt → Opacity"
            } else {
                "傾き→不透明度"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.tilt_flattening, 0.0..=0.9).text(if english {
                "Tilt → Flattening"
            } else {
                "傾き→扁平化"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.tilt_rotation, 0.0..=1.0).text(if english {
                "Tilt → Rotation"
            } else {
                "傾き→回転"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.hardness, 0.0..=1.0).text(if english {
                "Tip Hardness"
            } else {
                "先端の硬さ"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.grain, 0.0..=1.0).text(if english {
                "Grain Amount"
            } else {
                "グレイン量"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.grain_scale, 0.1..=8.0).text(if english {
                "Grain Scale"
            } else {
                "グレイン倍率"
            }),
        );
        ui.label(if english {
            "Grain position"
        } else {
            "グレインの位置"
        });
        ui.horizontal_wrapped(|ui| {
            // 0: fixed, 1: fixed and turned per stroke, 2: follows the brush.
            let current = match (brush.grain_fixed, brush.grain_random_rotation) {
                (true, false) => 0,
                (true, true) => 1,
                (false, _) => 2,
            };
            let choices: [(&str, &str); 3] = if english {
                [
                    ("Fixed to canvas", "The texture stays put on the canvas; strokes reveal it like paper."),
                    ("Fixed, turned each stroke", "Fixed to the canvas while you draw, but every new stroke turns it by a random angle, so repeated strokes do not show the same pattern."),
                    ("Follows the brush", "The texture moves with each dab, like a stamp."),
                ]
            } else {
                [
                    ("キャンバスに固定", "模様はキャンバスに貼り付いたまま。紙の目のように線が模様を拾います。"),
                    ("固定・描くたびに回転", "描いている間はキャンバスに固定し、線を引くたびに模様の向きをランダムに変えます。重ね塗りで同じ模様が目立ちません。"),
                    ("ブラシに追従", "模様が一打ごとにブラシと一緒に動きます（スタンプ風）。"),
                ]
            };
            for (index, (name, hint)) in choices.iter().enumerate() {
                if ui
                    .selectable_label(current == index, *name)
                    .on_hover_text(*hint)
                    .clicked()
                {
                    brush.grain_fixed = index < 2;
                    brush.grain_random_rotation = index == 1;
                }
            }
        });
        ui.add(
            egui::Slider::new(&mut brush.tip_aspect, 0.1..=10.0).text(if english {
                "Tip Aspect Ratio"
            } else {
                "先端の扁平率"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.tip_rotation, -180.0..=180.0).text(if english {
                "Tip Rotation"
            } else {
                "先端の回転"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.scatter, 0.0..=2.0).text(if english {
                "Scatter"
            } else {
                "散布"
            }),
        );
        if ui
            .button(if english {
                "Load Tip Image"
            } else {
                "先端画像を読み込む"
            })
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Tip mask", &["png", "bmp", "jpg", "jpeg"])
                .pick_file()
        {
            match load_brush_texture(&path) {
                Ok(texture) => brush.tip = Some(texture),
                Err(e) => self.status = format!("先端画像を読み込めません: {e}"),
            }
        }
        if brush.tip.is_some()
            && ui
                .button(if english {
                    "Remove Tip Image"
                } else {
                    "先端画像を解除"
                })
                .clicked()
        {
            brush.tip = None;
        }
        ui.group(|ui| {
            ui.label(
                egui::RichText::new(if english {
                    "Grain image"
                } else {
                    "グレイン画像"
                })
                .strong(),
            );
            ui.label(
                egui::RichText::new(if english {
                    "Black paints; white and transparent do not."
                } else {
                    "黒い所が塗られ、白と透明の所は塗られません。"
                })
                .small()
                .color(crate::layout::MUTED_TEXT),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(if english {
                        "Load…"
                    } else {
                        "読み込む…"
                    })
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Grain texture", &["png", "bmp", "jpg", "jpeg"])
                        .pick_file()
                {
                    match load_grain_texture(&path) {
                        Ok(texture) => {
                            brush.set_grain_source(Some(texture));
                            if brush.grain <= 0.0 {
                                brush.grain = 1.0;
                            }
                        }
                        Err(e) => self.status = format!("グレイン画像を読み込めません: {e}"),
                    }
                }
                egui::ComboBox::from_id_salt("grain-template")
                    .selected_text(if english {
                        "Templates"
                    } else {
                        "テンプレート"
                    })
                    .show_ui(ui, |ui| {
                        for (index, (ja, en, _)) in crate::GRAIN_TEMPLATES.iter().enumerate() {
                            if ui
                                .selectable_label(false, if english { *en } else { *ja })
                                .clicked()
                                && let Some(texture) = crate::grain_template(index)
                            {
                                brush.set_grain_source(Some(texture));
                                if brush.grain <= 0.0 {
                                    brush.grain = 1.0;
                                }
                            }
                        }
                    });
                if brush.grain_tip.is_some()
                    && ui.button(if english { "Remove" } else { "解除" }).clicked()
                {
                    brush.grain_source = None;
                    brush.grain_tip = None;
                }
            });
            if brush.grain_tip.is_some() {
                // Grains saved before sources existed become their own source.
                if brush.grain_source.is_none() {
                    brush.grain_source = brush.grain_tip.clone();
                    brush.grain_invert = false;
                    brush.grain_binary = false;
                }
                let before = (brush.grain_invert, brush.grain_binary);
                ui.horizontal_wrapped(|ui| {
                    ui.toggle_value(
                        &mut brush.grain_invert,
                        if english { "Invert" } else { "反転" },
                    );
                    ui.toggle_value(
                        &mut brush.grain_binary,
                        if english {
                            "Two levels (paint only on marks)"
                        } else {
                            "二値化（判定のある所だけ塗る）"
                        },
                    );
                });
                if before != (brush.grain_invert, brush.grain_binary) {
                    brush.refresh_grain();
                    if brush.grain_binary {
                        // Only the marked parts: nothing between them.
                        brush.grain = 1.0;
                    }
                }
                if let Some(grain) = &brush.grain_tip {
                    let key = (
                        grain.width,
                        grain.height,
                        grain
                            .coverage
                            .iter()
                            .step_by(97)
                            .map(|&v| v as u64)
                            .sum::<u64>(),
                        brush.grain_invert,
                        brush.grain_binary,
                    );
                    let stale = self.grain_preview.as_ref().is_none_or(|(k, _)| *k != key);
                    if stale {
                        // Painted parts dark on white, as they will print.
                        let (w, h) = (grain.width.min(256), grain.height.min(256));
                        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                        for y in 0..h {
                            for x in 0..w {
                                let v = 255 - grain.coverage[(y * grain.width + x) as usize];
                                rgba.extend([v, v, v, 255]);
                            }
                        }
                        let texture = ctx.load_texture(
                            "grain-preview",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [w as usize, h as usize],
                                &rgba,
                            ),
                            egui::TextureOptions::NEAREST,
                        );
                        self.grain_preview = Some((key, texture));
                    }
                    if let Some((_, texture)) = &self.grain_preview {
                        let size = texture.size_vec2();
                        let scale = (96.0 / size.x.max(size.y)).clamp(1.0, 4.0);
                        ui.image((texture.id(), size * scale));
                    }
                }
            }
        });
        brush.color = self.color;
    }

    /// The view's zoom in percent (100% = one document pixel per screen
    /// pixel).
    pub(crate) fn zoom_percent(&self) -> f32 {
        let (_, fit, pixels_per_point) = self.canvas_viewport;
        fit * self.zoom * pixels_per_point * 100.0
    }

    pub(crate) fn set_zoom_percent(&mut self, percent: f32) {
        let (_, fit, pixels_per_point) = self.canvas_viewport;
        self.zoom = (percent / 100.0 / (fit * pixels_per_point).max(1e-6)).clamp(0.01, 64.0);
    }

    /// The zoom in the bottom-left corner of the canvas, with the common
    /// zoom levels to pick from.
    pub(super) fn hud_bounds(&self) -> Rect {
        let area = self.canvas_viewport.0;
        Rect::from_min_size(
            area.left_bottom() + Vec2::new(8., -34.),
            Vec2::new((area.width() - 16.).clamp(0., 420.), 28.),
        )
    }
    pub(crate) fn zoom_indicator(&mut self, ui: &mut egui::Ui) {
        let area = self.canvas_viewport.0;
        if !area.is_positive() {
            return;
        }
        let english = self.language_english;
        let place = self.hud_bounds();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(place));
        // Takes presses and drags here, so they never start a stroke on the
        // canvas underneath.
        child.interact(
            place,
            egui::Id::new("zoom-indicator-guard"),
            egui::Sense::click_and_drag(),
        );
        egui::Frame::popup(child.style())
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(&mut child, |ui| {
                ui.horizontal(|ui| {
                    let percent = self.zoom_percent();
                    let label = if (percent - percent.round()).abs() < 0.05 {
                        format!("{percent:.0}%")
                    } else {
                        format!("{percent:.1}%")
                    };
                    egui::ComboBox::from_id_salt("zoom-indicator")
                        .selected_text(label)
                        .width(80.0)
                        .height(420.0)
                        .show_ui(ui, |ui| {
                            for level in ZOOM_LEVELS {
                                let text = if level.fract() == 0.0 {
                                    format!("{level:.0}%")
                                } else {
                                    format!("{level}%")
                                };
                                if ui
                                    .selectable_label((percent - level).abs() < 0.05, text)
                                    .clicked()
                                {
                                    self.set_zoom_percent(level);
                                }
                            }
                            ui.separator();
                            if ui
                                .selectable_label(false, if english { "Fit" } else { "全体" })
                                .clicked()
                            {
                                self.zoom = 1.0;
                                self.navigator_center = Vec2::new(
                                    self.doc.width as f32 / 2.0,
                                    self.doc.height as f32 / 2.0,
                                );
                            }
                        })
                        .response
                        .on_hover_text(if english {
                            "Zoom (Ctrl + / Ctrl − / Ctrl 0)"
                        } else {
                            "表示倍率（Ctrl＋＋／Ctrl＋−／Ctrl＋0）"
                        });

                    if ui
                        .small_button(format!("{:.0}°", self.view_rotation))
                        .clicked()
                    {
                        self.view_rotation = 0.;
                    }
                    if ui
                        .selectable_label(self.flip_x, "↔")
                        .on_hover_text(self.text("左右反転 (H)", "Flip view (H)"))
                        .clicked()
                    {
                        self.flip_x = !self.flip_x;
                    }
                    ui.label(format!("{}×{}", self.doc.width, self.doc.height));
                    if let (Some(pos), Some((rect, scale))) =
                        (ui.ctx().pointer_hover_pos(), self.canvas_screen)
                    {
                        let p = self.document_position_unsnapped(pos, rect, scale);
                        if p.x >= 0.
                            && p.y >= 0.
                            && p.x < self.doc.width as f32
                            && p.y < self.doc.height as f32
                        {
                            ui.label(format!("({}, {})", p.x as i32, p.y as i32));
                        }
                    }
                    ui.add(egui::Label::new(&self.doc.layers[self.selected_layer].name).truncate());
                });
            });
    }

    /// Zoom, rotation, flips, grid and symmetry rulers.
    #[allow(unused_variables)]
    pub(crate) fn view_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        let (_, fit, pixels_per_point) = self.canvas_viewport;
        let mut percent = fit * self.zoom * pixels_per_point * 100.0;
        ui.horizontal(|ui| {
            ui.label(if english { "Zoom" } else { "表示倍率" });
            if ui
                .add(
                    egui::DragValue::new(&mut percent)
                        .range(1.0..=6400.0)
                        .speed(1.0)
                        .suffix(" %"),
                )
                .changed()
            {
                self.zoom =
                    (percent / 100.0 / (fit * pixels_per_point).max(1e-6)).clamp(0.01, 64.0);
            }
        });
        ui.horizontal_wrapped(|ui| {
            for target in ZOOM_LEVELS {
                if ui.button(format!("{target}%")).clicked() {
                    self.set_zoom_percent(target);
                }
            }
            if ui.button(if english { "Fit" } else { "全体" }).clicked() {
                self.zoom = 1.0;
                self.navigator_center =
                    Vec2::new(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0);
            }
        });
        ui.add(
            egui::Slider::new(&mut self.view_rotation, -180.0..=180.0).text(
                if self.language_english {
                    "Rotation"
                } else {
                    "回転"
                },
            ),
        );
        ui.checkbox(
            &mut self.flip_x,
            if self.language_english {
                "Flip Horizontal"
            } else {
                "左右反転"
            },
        );
        ui.checkbox(
            &mut self.flip_y,
            if self.language_english {
                "Flip Vertical"
            } else {
                "上下反転"
            },
        );
        if ui
            .checkbox(
                &mut self.transparency_checker,
                if self.language_english {
                    "Checkerboard Behind Transparency"
                } else {
                    "透明部分を市松模様で表示"
                },
            )
            .changed()
        {
            self.canvas_texture_dirty = true;
            self.navigator_texture_dirty = true;
        }
        ui.checkbox(
            &mut self.show_grid,
            if self.language_english {
                "Grid"
            } else {
                "グリッド"
            },
        );
        ui.add(
            egui::Slider::new(&mut self.grid_size, 8..=256).text(if self.language_english {
                "Grid Spacing"
            } else {
                "グリッド間隔"
            }),
        );
        ui.add_enabled(
            self.show_grid,
            egui::Checkbox::new(
                &mut self.grid_snap,
                if self.language_english {
                    "Snap to Grid"
                } else {
                    "グリッドにスナップ"
                },
            ),
        );
        ui.checkbox(
            &mut self.symmetry_x,
            if self.language_english {
                "Horizontal Symmetry"
            } else {
                "左右対称定規"
            },
        );
        ui.checkbox(
            &mut self.symmetry_y,
            if self.language_english {
                "Vertical Symmetry"
            } else {
                "上下対称定規"
            },
        );
        ui.add(egui::Slider::new(&mut self.symmetry_count, 1..=16).text(
            if self.language_english {
                "Radial Symmetry Divisions"
            } else {
                "回転対称分割数"
            },
        ));
        ui.horizontal(|ui| {
            ui.label(self.text("対称中心", "Symmetry Center"));
            ui.add(
                egui::DragValue::new(&mut self.symmetry_center.x)
                    .range(0.0..=self.doc.width.saturating_sub(1) as f32)
                    .prefix("X "),
            );
            ui.add(
                egui::DragValue::new(&mut self.symmetry_center.y)
                    .range(0.0..=self.doc.height.saturating_sub(1) as f32)
                    .prefix("Y "),
            );
            if ui
                .small_button(if self.language_english {
                    "Center"
                } else {
                    "中央"
                })
                .clicked()
            {
                self.symmetry_center = Vec2::new(
                    (self.doc.width - 1) as f32 / 2.0,
                    (self.doc.height - 1) as f32 / 2.0,
                );
            }
        });
    }

    /// Sub view and reference images.
    #[allow(unused_variables)]
    pub(crate) fn subview_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        if ui
            .button(self.text("画像を新しいレイヤーに読み込む", "Import Image as Layer"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    if self.language_english {
                        "Images"
                    } else {
                        "画像"
                    },
                    &["png", "jpg", "jpeg", "bmp", "gif"],
                )
                .pick_file()
        {
            self.status = match self.queue_image_layer(path, ctx) {
                Ok(()) => self
                    .text(
                        "画像をレイヤーとして読み込み中…",
                        "Importing image as layer…",
                    )
                    .into(),
                Err(error) => error,
            };
        }
        if ui
            .button(self.text("資料ビューで画像を開く", "Open image in reference view"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    if self.language_english {
                        "Images"
                    } else {
                        "画像"
                    },
                    &["png", "jpg", "jpeg", "bmp", "gif"],
                )
                .pick_file()
        {
            self.status = match self.io_task_sender.send(IoTask::LoadReference {
                path,
                repaint: ctx.clone(),
            }) {
                Ok(()) => self
                    .text(
                        "資料ビューの画像を読み込み中…",
                        "Loading reference view image…",
                    )
                    .into(),
                Err(error) => format!("資料ビューの画像を読み込めません: {error}"),
            };
        }
        if self.reference_image.is_some() {
            ui.add(egui::Slider::new(&mut self.reference_zoom, 0.1..=1.5).text(
                if self.language_english {
                    "Sub View Zoom"
                } else {
                    "資料ビュー倍率"
                },
            ));
            ui.add(
                egui::Slider::new(&mut self.reference_opacity, 0.1..=1.).text(
                    if self.language_english {
                        "Opacity"
                    } else {
                        "表示濃度"
                    },
                ),
            );
            let size = (self.reference_size * self.reference_zoom)
                .min(Vec2::splat(ui.available_width().max(1.)));
            if let Some(texture) = &self.reference_image {
                ui.add(
                    egui::Image::new((texture.id(), size)).tint(Color32::from_white_alpha(
                        (self.reference_opacity * 255.) as u8,
                    )),
                );
            }
            if ui
                .button(self.text("資料ビューの画像を閉じる", "Close reference view image"))
                .clicked()
            {
                self.reference_image = None;
                self.reference_image_path = None;
            }
        }
    }

    pub(super) fn replay_last_stroke(&mut self) {
        self.finish_pending_canvas_gesture();
        if let Some(points) = self
            .stroke_log
            .replay(self.stroke_log.strokes.len().saturating_sub(1))
        {
            self.history.begin();
            self.render_replay(&points);
            self.history.commit();
        }
    }

    /// Brush import/export, input logs and brush comparison.
    #[allow(unused_variables)]
    pub(crate) fn brush_io_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        ui.label(
            egui::RichText::new(self.text(
                "ブラシセット（プリセット一式）: 書き出したファイルを assets/brushes/default.efudebrushes に置くと、それが新しくインストールしたときの初期プリセットになります。",
                "Brush set (all presets): an exported set placed at assets/brushes/default.efudebrushes becomes the presets of a new installation.",
            ))
            .small()
            .color(crate::layout::MUTED_TEXT),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(self.text("ブラシセットを書き出し…", "Export Brush Set…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Efude Brush Set", &["efudebrushes"])
                    .set_file_name("default.efudebrushes")
                    .save_file()
            {
                if let Some(brush) = self.brushes.get_mut(self.selected_brush) {
                    brush.size = self.size;
                }
                self.status = match efude_brush::save_set(&path, &self.brushes) {
                    Ok(()) => self
                        .text("ブラシセットを書き出しました", "Brush set exported")
                        .into(),
                    Err(error) => error.to_string(),
                };
            }
            if ui
                .button(self.text("ブラシセットを読み込み…", "Import Brush Set…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Efude Brush Set", &["efudebrushes"])
                    .pick_file()
            {
                self.status = match efude_brush::load_set(&path) {
                    Ok(brushes) => {
                        self.stage_brush_import(brushes, path.display().to_string());
                        self.text(
                            "取り込むブラシを確認してください",
                            "Review the brushes to import",
                        )
                        .into()
                    }
                    Err(error) => error.to_string(),
                };
            }
            if self.material_import_undo.is_some()
                && ui
                    .button(self.text("直前の取り込みを戻す", "Undo last import"))
                    .clicked()
            {
                self.undo_brush_import();
            }
            if ui
                .button(self.text("初期ブラシを追加", "Add Default Brushes"))
                .on_hover_text(self.text(
                    "今のブラシはそのままに、初期ブラシを1列（10本）追加します",
                    "Adds one column of the default brushes, keeping yours",
                ))
                .clicked()
            {
                let first = crate::default_presets();
                let column = first.len().min(10);
                self.brushes.extend(first.into_iter().take(column));
                self.status = self
                    .text("初期ブラシを追加しました", "Default brushes added")
                    .into();
            }
            if ui
                .button(self.text("初期プリセットに戻す", "Restore Default Presets"))
                .on_hover_text(self.text(
                    "すべてのブラシを初期状態に置き換えます",
                    "Replaces every brush with the defaults",
                ))
                .clicked()
            {
                self.replace_brushes(crate::default_presets());
            }
        });
        ui.separator();
        if ui
            .button(self.text("ブラシを書き出し", "Export Brush"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Efude Brush", &["efudebrush"])
                .save_file()
            && let Err(e) = efude_brush::save_bundle(&path, &self.brushes[self.selected_brush])
        {
            self.status = e.to_string();
        }
        if ui
            .button(self.text("ブラシを読み込み", "Import Brush"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Efude Brush", &["efudebrush"])
                .pick_file()
        {
            match efude_brush::load_bundle(&path) {
                Ok(b) => {
                    self.brushes.push(b);
                    self.selected_brush = self.brushes.len() - 1;
                }
                Err(e) => self.status = e.to_string(),
            }
        }
        if ui
            .button(self.text("入力ログ保存", "Save Input Log"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Stroke Log", &["json"])
                .save_file()
            && let Err(e) = self.stroke_log.save(&path)
        {
            self.status = e.to_string();
        }
        if ui
            .button(self.text("入力ログ読込", "Load Input Log"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Stroke Log", &["json"])
                .pick_file()
        {
            match efude_input::StrokeLog::load(&path) {
                Ok(log) => self.stroke_log = log,
                Err(e) => self.status = e.to_string(),
            }
        }
        if ui
            .button(self.text("最後の線を再生", "Replay Last Stroke"))
            .clicked()
        {
            self.replay_last_stroke();
        }
        ui.horizontal(|ui| {
            if ui
                .button(self.text("ブラシ比較を開始", "Start Brush Comparison"))
                .clicked()
            {
                self.comparison_brush = Some(self.brushes[self.selected_brush].clone());
                self.stroke_log.strokes.clear();
                self.recording_stroke = true;
                self.status = self
                    .text(
                        "元ブラシの比較用ストロークを描いてください",
                        "Draw a comparison stroke with the original brush",
                    )
                    .into();
            }
            if ui
                .button(self.text("ブラシ比較を終了", "Finish Brush Comparison"))
                .clicked()
            {
                self.recording_stroke = false;
                self.status = self
                    .text(
                        "比較用ストロークを記録しました",
                        "Comparison stroke recorded",
                    )
                    .into();
            }
        });
        if self.comparison_brush.is_some() {
            ui.label(if self.recording_stroke {
                "● 比較記録中"
            } else {
                "比較記録済み"
            });
            if ui
                .button(self.text("別レイヤーで比較再生", "Replay Comparison on New Layers"))
                .clicked()
                && {
                    self.finish_pending_canvas_gesture();
                    self.can_add_layers(2)
                }
                && let Some(points) = self.stroke_log.replay(0)
            {
                let original_brush = self.comparison_brush.clone().unwrap();
                let selected_brush = self.brushes[self.selected_brush].clone();
                let old_brush_index = self.selected_brush;
                let old_layer_index = self.selected_layer;
                let parent = self.doc.layers[old_layer_index].parent_id;
                let ids = self.next_layer_ids(2);
                self.history.begin();
                for ((label, brush), id) in [("元", original_brush), ("現在", selected_brush)]
                    .into_iter()
                    .zip(ids)
                {
                    let mut layer = efude_canvas::Layer::new(
                        id,
                        format!("比較 {label}: {}", brush.name),
                        self.doc.width,
                        self.doc.height,
                    );
                    layer.parent_id = parent;
                    let index = old_layer_index + 1;
                    self.history
                        .insert_layer(&mut self.doc.layers, index, layer);
                    self.selected_layer = index;
                    self.size = brush.size;
                    self.brushes.push(brush);
                    self.selected_brush = self.brushes.len() - 1;
                    self.render_replay(&points);
                    self.brushes.pop();
                }
                self.history.commit();
                self.selected_layer = old_layer_index;
                self.selected_brush = old_brush_index;
                self.size = self.brushes[old_brush_index].size;
            }
            if ui
                .button(self.text("比較を解除", "Clear Comparison"))
                .clicked()
            {
                self.comparison_brush = None;
                self.recording_stroke = false;
            }
        }
    }

    /// Canvas size and resolution.
    #[allow(unused_variables)]
    pub(crate) fn canvas_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        ui.heading(self.text("キャンバス", "Canvas"));
        ui.label(format!("{} × {} px", self.doc.width, self.doc.height));
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut self.canvas_width_input)
                    .range(1..=efude_canvas::MAX_DOCUMENT_DIMENSION)
                    .prefix(if english { "Width " } else { "幅 " }),
            );
            ui.add(
                egui::DragValue::new(&mut self.canvas_height_input)
                    .range(1..=efude_canvas::MAX_DOCUMENT_DIMENSION)
                    .prefix(if english { "Height " } else { "高さ " }),
            );
        });
        ui.add(egui::Slider::new(&mut self.canvas_dpi_input, 10.0..=2400.0).text("DPI"));
        if ui
            .button(self.text("キャンバスサイズを適用", "Apply Canvas Size"))
            .clicked()
        {
            self.apply_canvas_size();
        }
    }

    pub(crate) fn apply_canvas_size(&mut self) {
        self.finish_pending_canvas_gesture();
        self.history.begin();
        let before_selection_active = self.selection.active;
        let before_selection = self.selection.mask.clone();
        let resize_result = self.history.resize_document(
            &mut self.doc,
            self.canvas_width_input,
            self.canvas_height_input,
            self.canvas_dpi_input,
        );
        if matches!(resize_result, Ok(true)) {
            self.selection.clear();
            self.history.record_selection_change(
                before_selection_active,
                before_selection,
                self.selection.active,
                self.selection.mask.clone(),
            );
        }
        self.history.commit();
        match resize_result {
            Ok(true) => {
                self.editing_mask = false;
                self.symmetry_center = Vec2::new(
                    (self.doc.width - 1) as f32 / 2.0,
                    (self.doc.height - 1) as f32 / 2.0,
                );
                self.navigator_center =
                    Vec2::new(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0);
                self.canvas_width_input = self.doc.width;
                self.canvas_height_input = self.doc.height;
                self.canvas_dpi_input = self.doc.dpi;
                self.canvas_texture_dirty = true;
                self.navigator_texture_dirty = true;
                self.status = self
                    .text(
                        "キャンバスサイズを変更しました（Undoで復元できます）",
                        "Canvas size changed (Undo to restore)",
                    )
                    .into();
            }
            Ok(false) => {
                self.status = self
                    .text("サイズは変更されていません", "Canvas size is unchanged")
                    .into();
            }
            Err(_) => {
                self.status = self
                                .text(
                                    "キャンバスは幅・高さ30,000px以下、総画素数1億以下にしてください",
                                    "Canvas size must be at most 30,000 px per side and 100 million pixels total",
                                )
                                .into();
            }
        }
    }

    pub(super) fn apply_layer_mask_action(&mut self, index: usize, action: u8) {
        self.finish_pending_canvas_gesture();
        let width = self.doc.width;
        let height = self.doc.height;
        self.history.begin();
        self.history
            .record_all_mask_tiles(&self.doc.layers[index], width, height);
        let mask = self.doc.layers[index].mask.as_mut().unwrap();
        if action == 0 {
            for y in 0..height {
                for x in 0..width {
                    // Missing mask tiles are white; allocate them before
                    // the first write so later pixels keep that default.
                    if !mask.has_tile(x, y) {
                        mask.ensure_tile_filled(x, y, [255; 4]);
                    }
                    let old = mask.pixel_or_tile_default(x, y, [255; 4])[0];
                    mask.set_pixel(x, y, [255 - old; 4]);
                }
            }
        } else {
            mask.clear_tiles();
            let selection = &self.selection.mask;
            for tile_y in 0..height.div_ceil(efude_canvas::TILE_SIZE) {
                for tile_x in 0..width.div_ceil(efude_canvas::TILE_SIZE) {
                    let origin_x = tile_x * efude_canvas::TILE_SIZE;
                    let origin_y = tile_y * efude_canvas::TILE_SIZE;
                    let tile_width = efude_canvas::TILE_SIZE.min(width - origin_x);
                    let tile_height = efude_canvas::TILE_SIZE.min(height - origin_y);
                    let has_cutout = (0..tile_height).any(|local_y| {
                        (0..tile_width).any(|local_x| {
                            selection
                                .get(((origin_y + local_y) * width + origin_x + local_x) as usize)
                                .copied()
                                .unwrap_or(0)
                                < 255
                        })
                    });
                    if has_cutout {
                        mask.ensure_tile_filled(origin_x, origin_y, [255; 4]);
                        for local_y in 0..tile_height {
                            for local_x in 0..tile_width {
                                let x = origin_x + local_x;
                                let y = origin_y + local_y;
                                let value = selection
                                    .get((y * width + x) as usize)
                                    .copied()
                                    .unwrap_or(0);
                                mask.set_pixel(x, y, [value; 4]);
                            }
                        }
                    }
                }
            }
        }
        self.history.commit();
    }

    /// Layer list and layer operations.
    #[allow(unused_variables)]
    /// Replaces the document with a blank canvas of the size in the canvas
    /// inputs (the caller has confirmed discarding unsaved changes). Returns
    /// false if the size is invalid.
    pub(crate) fn new_document(&mut self) -> bool {
        self.pending_merge = None;
        self.confirm_layer_clear = false;
        self.cancel_studio_session();
        self.thumbnails.clear();
        self.studio.last_brush_point = None;
        if !efude_canvas::valid_document_dimensions(
            self.canvas_width_input,
            self.canvas_height_input,
        ) || !self.canvas_dpi_input.is_finite()
            || !(10.0..=2400.0).contains(&self.canvas_dpi_input)
        {
            self.status = self
                .text(
                    "新規キャンバスの幅・高さ・DPIを確認してください",
                    "Check the new canvas width, height, and DPI",
                )
                .into();
            return false;
        }
        self.open_document_tab();
        self.doc = Document::new(self.canvas_width_input, self.canvas_height_input);
        self.doc.dpi = self.canvas_dpi_input;
        self.fill_reference_layer = 0;
        self.selection_reference_layer = 0;
        self.symmetry_center = Vec2::new(
            self.doc.width.saturating_sub(1) as f32 / 2.0,
            self.doc.height.saturating_sub(1) as f32 / 2.0,
        );
        self.navigator_center = Vec2::new(self.doc.width as f32 / 2., self.doc.height as f32 / 2.);
        self.zoom = 1.0;
        self.doc_path = None;
        self.selected_layer = 0;
        self.history = History::default();
        self.selection.clear();
        self.editing_mask = false;
        self.view_rotation = 0.0;
        self.flip_x = false;
        self.flip_y = false;
        self.canvas_texture_dirty = true;
        self.navigator_texture_dirty = true;
        self.status = self
            .text("新しいキャンバスを作成しました", "New canvas created")
            .into();
        true
    }

    /// The "New Canvas" dialog: size presets, width, height and resolution.
    pub(crate) fn new_document_window(&mut self, ctx: &egui::Context) {
        let english = self.language_english;
        let mut open = self.show_new_document;
        let mut create = false;
        egui::Window::new(if english {
            "New Canvas"
        } else {
            "新規キャンバス"
        })
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, w, h, dpi) in [
                    ("1920×1080", 1920, 1080, 72.0),
                    ("A4 350dpi", 2894, 4093, 350.0),
                    ("B5 350dpi", 2508, 3541, 350.0),
                    ("2048×2048", 2048, 2048, 72.0),
                    ("4096×4096", 4096, 4096, 72.0),
                ] {
                    if ui.button(label).clicked() {
                        self.canvas_width_input = w;
                        self.canvas_height_input = h;
                        self.canvas_dpi_input = dpi;
                    }
                }
            });
            ui.add_space(6.0);
            egui::Grid::new("new-canvas").num_columns(2).show(ui, |ui| {
                ui.label(if english { "Width (px)" } else { "幅 (px)" });
                ui.add(
                    egui::DragValue::new(&mut self.canvas_width_input)
                        .range(1..=efude_canvas::MAX_DOCUMENT_DIMENSION),
                );
                ui.end_row();
                ui.label(if english {
                    "Height (px)"
                } else {
                    "高さ (px)"
                });
                ui.add(
                    egui::DragValue::new(&mut self.canvas_height_input)
                        .range(1..=efude_canvas::MAX_DOCUMENT_DIMENSION),
                );
                ui.end_row();
                ui.label(if english {
                    "Resolution (dpi)"
                } else {
                    "解像度 (dpi)"
                });
                ui.add(egui::DragValue::new(&mut self.canvas_dpi_input).range(10.0..=2400.0));
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if english { "Create" } else { "作成" };
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(label).color(Color32::WHITE))
                            .fill(crate::layout::ACCENT),
                    )
                    .clicked()
                {
                    create = true;
                }
                if ui
                    .button(if english { "Cancel" } else { "キャンセル" })
                    .clicked()
                {
                    self.show_new_document = false;
                }
            });
        });
        if create && self.new_document() {
            open = false;
        }
        self.show_new_document = open && self.show_new_document;
    }
}

/// Evaluates the pressure curve through (0,0), the two control points and
/// (1,1) at `x`, then applies `gamma` (as the brushes do).
fn pressure_curve_value(points: [f32; 4], gamma: f32, x: f32) -> f32 {
    let [x1, y1, x2, y2] = points.map(|v| v.clamp(0.0, 1.0));
    let x = x.clamp(0.0, 1.0);
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let t = (low + high) * 0.5;
        let u = 1.0 - t;
        let bx = 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t;
        if bx < x {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) * 0.5;
    let u = 1.0 - t;
    let y = 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t;
    y.clamp(0.0, 1.0).powf(gamma.clamp(0.2, 3.0))
}

/// A square graph of a pressure curve (input across, output up) whose two
/// control points are dragged with the pointer. `marker` draws a pressure
/// on the curve. Returns whether a point moved.
pub(crate) fn pressure_curve_editor(
    ui: &mut egui::Ui,
    id: &str,
    points: &mut [f32; 4],
    gamma: f32,
    marker: Option<f32>,
) -> bool {
    let side = ui.available_width().clamp(120.0, 200.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::hover());
    let graph = rect.shrink(8.0);
    let to_screen = |x: f32, y: f32| {
        Pos2::new(
            graph.left() + x * graph.width(),
            graph.bottom() - y * graph.height(),
        )
    };
    let to_value = |pos: Pos2| {
        (
            ((pos.x - graph.left()) / graph.width()).clamp(0.0, 1.0),
            ((graph.bottom() - pos.y) / graph.height()).clamp(0.0, 1.0),
        )
    };
    let mut changed = false;
    let handles = [
        (0usize, to_screen(points[0], points[1])),
        (2, to_screen(points[2], points[3])),
    ];
    for (index, center) in handles {
        let handle = Rect::from_center_size(center, Vec2::splat(18.0));
        let response = ui.interact(
            handle,
            ui.make_persistent_id((id, index)),
            egui::Sense::drag(),
        );
        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let (x, y) = to_value(pos);
            points[index] = x;
            points[index + 1] = y;
            changed = true;
        }
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(graph, 4.0, Color32::from_rgb(28, 29, 36));
    let grid = Stroke::new(1.0, Color32::from_gray(52));
    for step in 1..4 {
        let f = step as f32 / 4.0;
        painter.line_segment([to_screen(f, 0.0), to_screen(f, 1.0)], grid);
        painter.line_segment([to_screen(0.0, f), to_screen(1.0, f)], grid);
    }
    painter.line_segment(
        [to_screen(0.0, 0.0), to_screen(1.0, 1.0)],
        Stroke::new(1.0, Color32::from_gray(80)),
    );
    let guide = Stroke::new(1.0, Color32::from_rgb(120, 130, 170));
    painter.line_segment(
        [to_screen(0.0, 0.0), to_screen(points[0], points[1])],
        guide,
    );
    painter.line_segment(
        [to_screen(1.0, 1.0), to_screen(points[2], points[3])],
        guide,
    );
    let curve: Vec<Pos2> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0;
            to_screen(x, pressure_curve_value(*points, gamma, x))
        })
        .collect();
    painter.add(egui::Shape::line(
        curve,
        Stroke::new(2.0, crate::layout::ACCENT),
    ));
    if let Some(pressure) = marker {
        let y = pressure_curve_value(*points, gamma, pressure);
        painter.circle_filled(
            to_screen(pressure, y),
            3.5,
            Color32::from_rgb(255, 205, 110),
        );
    }
    for (index, _) in handles {
        let center = to_screen(points[index], points[index + 1]);
        painter.circle_filled(center, 6.0, Color32::WHITE);
        painter.circle_stroke(center, 6.0, Stroke::new(1.5, crate::layout::ACCENT));
    }
    painter.text(
        graph.left_top() + Vec2::new(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        "out",
        egui::FontId::proportional(10.0),
        Color32::from_gray(120),
    );
    painter.text(
        graph.right_bottom() - Vec2::new(4.0, 2.0),
        egui::Align2::RIGHT_BOTTOM,
        "in",
        egui::FontId::proportional(10.0),
        Color32::from_gray(120),
    );
    changed
}

/// Zoom levels offered next to the canvas (percent).
const ZOOM_LEVELS: [f32; 10] = [
    10.0, 12.5, 25.0, 50.0, 75.0, 100.0, 200.0, 300.0, 400.0, 500.0,
];
