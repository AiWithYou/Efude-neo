// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Several open documents, one per canvas tab. The active document lives in
//! the app's own fields (so every tool works on it unchanged); the others
//! are parked here and swapped in when their tab is chosen.

use super::*;

/// Everything that belongs to one open document.
pub(crate) struct DocumentTab {
    pub(crate) doc: Document,
    pub(crate) history: History,
    selection: Selection,
    selected_layer: usize,
    editing_mask: bool,
    zoom: f32,
    navigator_center: Vec2,
    view_rotation: f32,
    flip_x: bool,
    flip_y: bool,
    pub(crate) doc_path: Option<std::path::PathBuf>,
    symmetry_center: Vec2,
    perspective_points: Vec<(i32, i32)>,
    perspective_selected: usize,
    fill_reference_layer: u64,
    selection_reference_layer: u64,
    effect_layer: Option<u64>,
    paste_preview: Option<(u32, u32, Vec<u8>, Vec2)>,
    paste_texture: Option<egui::TextureHandle>,
}

/// A tab: its number for untitled documents, and the parked document
/// (`None` for the active tab, whose document is in the app).
pub(crate) struct TabSlot {
    pub number: u32,
    pub parked: Option<DocumentTab>,
    pub recovered_title: Option<String>,
}

/// State of the tab bar.
#[derive(Default)]
pub(crate) struct Tabs {
    pub slots: Vec<TabSlot>,
    pub active: usize,
    pub next_number: u32,
    /// Tab waiting for "close without saving?" confirmation.
    pub confirm_close: Option<usize>,
}

impl Tabs {
    pub fn with_one() -> Self {
        Self {
            slots: vec![TabSlot {
                number: 1,
                parked: None,
                recovered_title: None,
            }],
            active: 0,
            next_number: 2,
            confirm_close: None,
        }
    }
}

impl EfudeApp {
    pub(crate) fn install_image_in_parked_tab(
        &mut self,
        document_id: u64,
        path: &std::path::Path,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<bool, ()> {
        let Some(tab) = self
            .tabs
            .slots
            .iter_mut()
            .filter_map(|slot| slot.parked.as_mut())
            .find(|tab| tab.history.document_id() == document_id)
        else {
            return Ok(false);
        };
        tab.selected_layer =
            Self::insert_image_layer(&mut tab.doc, &mut tab.history, path, width, height, rgba)?;
        tab.editing_mask = false;
        Ok(true)
    }

    pub(crate) fn install_guide_in_parked_tab(
        &mut self,
        document_id: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> bool {
        let Some(tab) = self
            .tabs
            .slots
            .iter_mut()
            .filter_map(|slot| slot.parked.as_mut())
            .find(|tab| tab.history.document_id() == document_id)
        else {
            return false;
        };
        let Some(guide) = efude_canvas::GuideImage::fit_to_canvas(width, height, rgba, &tab.doc)
        else {
            return false;
        };
        tab.history.set_guide(&mut tab.doc, Some(guide));
        true
    }

    pub(crate) fn install_filter_in_parked_tab(
        &mut self,
        document_id: u64,
        state_token: u64,
        layer_id: u64,
        pixels: efude_canvas::TilePixels,
    ) -> bool {
        let Some(tab) = self
            .tabs
            .slots
            .iter_mut()
            .filter_map(|slot| slot.parked.as_mut())
            .find(|tab| tab.history.document_id() == document_id)
        else {
            return false;
        };
        if tab.history.state_token() != state_token {
            return false;
        }
        let Some(index) = tab.doc.layers.iter().position(|layer| layer.id == layer_id) else {
            return false;
        };
        if tab.doc.layers[index].locked || tab.doc.layers[index].kind != LayerKind::Raster {
            return false;
        }
        // Match the active document's reference restriction, including
        // reference folders, without activating or redrawing this tab.
        let mut ancestor = Some(layer_id);
        for _ in 0..tab.doc.layers.len() {
            let Some(layer) =
                ancestor.and_then(|id| tab.doc.layers.iter().find(|candidate| candidate.id == id))
            else {
                break;
            };
            if layer.reference {
                return false;
            }
            ancestor = layer.parent_id;
        }
        tab.history.begin();
        tab.history
            .record_all_layer_tiles(&tab.doc.layers[index], tab.doc.width, tab.doc.height);
        tab.doc.layers[index].pixels = pixels;
        tab.history.commit();
        true
    }

    pub(crate) fn finish_pending_canvas_gesture(&mut self) {
        if self.selection_start.is_some()
            || self.stroke_builder.is_some()
            || !self.active.is_empty()
        {
            self.finish_canvas_gesture(false);
            self.canvas_gesture_interrupted = true;
        }
    }

    fn park_active(&mut self) -> DocumentTab {
        self.finish_pending_canvas_gesture();
        self.commit_pending_guide_edit();
        let size = (self.doc.width, self.doc.height);
        DocumentTab {
            doc: std::mem::replace(&mut self.doc, Document::new(size.0, size.1)),
            history: std::mem::take(&mut self.history),
            selection: std::mem::take(&mut self.selection),
            selected_layer: self.selected_layer,
            editing_mask: self.editing_mask,
            zoom: self.zoom,
            navigator_center: self.navigator_center,
            view_rotation: self.view_rotation,
            flip_x: self.flip_x,
            flip_y: self.flip_y,
            doc_path: self.doc_path.take(),
            symmetry_center: self.symmetry_center,
            perspective_points: std::mem::take(&mut self.perspective_points),
            perspective_selected: self.perspective_selected,
            fill_reference_layer: self.fill_reference_layer,
            selection_reference_layer: self.selection_reference_layer,
            effect_layer: self.comic_ui.effect_layer.take(),
            paste_preview: self.paste_preview.take(),
            paste_texture: self.paste_texture.take(),
        }
    }

    fn unpark(&mut self, tab: DocumentTab) {
        self.doc = tab.doc;
        self.history = tab.history;
        self.selection = tab.selection;
        self.selected_layer = tab.selected_layer;
        self.editing_mask = tab.editing_mask;
        self.zoom = tab.zoom;
        self.navigator_center = tab.navigator_center;
        self.view_rotation = tab.view_rotation;
        self.flip_x = tab.flip_x;
        self.flip_y = tab.flip_y;
        self.doc_path = tab.doc_path;
        self.symmetry_center = tab.symmetry_center;
        self.perspective_points = tab.perspective_points;
        self.perspective_selected = tab.perspective_selected;
        self.fill_reference_layer = tab.fill_reference_layer;
        self.selection_reference_layer = tab.selection_reference_layer;
        self.comic_ui.effect_layer = tab.effect_layer;
        self.paste_preview = tab.paste_preview;
        self.paste_texture = tab.paste_texture;
        self.canvas_width_input = self.doc.width;
        self.canvas_height_input = self.doc.height;
        self.canvas_dpi_input = self.doc.dpi;
    }

    /// Clears gestures finished on the old document and redraws everything
    /// whenever the active document changes.
    fn reset_transient_state(&mut self) {
        self.active.clear();
        self.stroke_builder = None;
        self.provisional = None;
        self.pending_provisional.clear();
        self.vector_live = None;
        self.vector_edit = Default::default();
        self.move_origin = None;
        self.balloon_ui.clear_document_gesture();
        self.layer_drag = None;
        self.paste_preview = None;
        self.paste_texture = None;
        self.selection_start = None;
        self.selection_before_gesture = None;
        self.selection_points.clear();
        self.bezier_points.clear();
        self.gesture_end = None;
        self.canvas_gesture_interrupted = true;
        self.pan_start = None;
        self.setting_vanishing_point = false;
        self.comic_ui.split_start = None;
        self.comic_ui.split_end = None;
        self.reset_stroke_buffers();
        self.dirty_canvas_tiles.clear();
        self.canvas_texture_dirty = true;
        self.navigator_texture_dirty = true;
    }

    /// The active tab holds an untouched, never-saved canvas that a new or
    /// opened document may simply replace.
    fn active_tab_is_pristine(&self) -> bool {
        self.doc_path.is_none()
            && self.paste_preview.is_none()
            && !self.history.is_dirty()
            && !self.history.can_undo()
            && self
                .doc
                .layers
                .iter()
                .all(|layer| !layer.pixels.has_allocated_tiles())
    }

    /// Makes room for a document that is about to be installed in the app
    /// fields: a new tab, unless the active one is pristine.
    pub(crate) fn open_document_tab(&mut self) {
        if self.tabs.slots.is_empty() {
            self.tabs = tabs::Tabs::with_one();
        }
        if self.active_tab_is_pristine() {
            return;
        }
        let parked = self.park_active();
        self.tabs.slots[self.tabs.active].parked = Some(parked);
        let number = self.tabs.next_number;
        self.tabs.next_number += 1;
        self.tabs.slots.push(TabSlot {
            number,
            parked: None,
            recovered_title: None,
        });
        self.tabs.active = self.tabs.slots.len() - 1;
        self.reset_transient_state();
    }

    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index == self.tabs.active || index >= self.tabs.slots.len() {
            return;
        }
        let Some(target) = self.tabs.slots[index].parked.take() else {
            return;
        };
        let parked = self.park_active();
        self.tabs.slots[self.tabs.active].parked = Some(parked);
        self.reset_transient_state();
        self.unpark(target);
        self.tabs.active = index;
    }

    fn tab_is_dirty(&self, index: usize) -> bool {
        match &self.tabs.slots[index].parked {
            Some(tab) => tab.history.is_dirty(),
            None => self.history.is_dirty(),
        }
    }

    /// Indices of tabs with unsaved changes.
    pub(crate) fn dirty_tabs(&self) -> Vec<usize> {
        (0..self.tabs.slots.len())
            .filter(|&index| self.tab_is_dirty(index))
            .collect()
    }

    fn tab_path(&self, index: usize) -> Option<&std::path::PathBuf> {
        match &self.tabs.slots[index].parked {
            Some(tab) => tab.doc_path.as_ref(),
            None => self.doc_path.as_ref(),
        }
    }

    /// Files of the tabs with unsaved changes.
    pub(crate) fn dirty_tab_paths(&self) -> Vec<std::path::PathBuf> {
        self.dirty_tabs()
            .into_iter()
            .filter_map(|index| self.tab_path(index).cloned())
            .collect()
    }

    /// Queues a separate backup snapshot for every changed, named canvas.
    /// Parked tabs must be included: their documents can remain open without
    /// becoming active again for the whole editing session.
    pub(crate) fn queue_dirty_tab_backups(&mut self, ctx: &egui::Context) -> Result<usize, String> {
        self.commit_pending_guide_edit();
        let mut queued = 0;
        for slot in &self.tabs.slots {
            if let Some(tab) = &slot.parked {
                if tab.history.is_dirty()
                    && let Some(path) = &tab.doc_path
                {
                    self.io_task_sender
                        .send(IoTask::Save {
                            path: path.clone(),
                            document: tab.doc.clone(),
                            backup: true,
                            backup_generations: self.backup_generations,
                            state_token: tab.history.state_token(),
                            document_id: tab.history.document_id(),
                            repaint: ctx.clone(),
                        })
                        .map_err(|_| "保存ワーカーへタスクを送信できませんでした".to_string())?;
                    queued += 1;
                }
            } else if self.history.is_dirty()
                && let Some(path) = &self.doc_path
            {
                self.queue_document_save(path.clone(), true, ctx)?;
                queued += 1;
            }
        }
        Ok(queued)
    }

    /// The tab showing the file at `path`.
    pub(crate) fn tab_with_path(&self, path: &std::path::Path) -> Option<usize> {
        let key = book::path_key(path);
        (0..self.tabs.slots.len()).find(|&index| {
            self.tab_path(index)
                .is_some_and(|p| book::path_key(p) == key)
        })
    }

    pub(crate) fn tab_title(&self, index: usize) -> String {
        let path = match &self.tabs.slots[index].parked {
            Some(tab) => tab.doc_path.as_ref(),
            None => self.doc_path.as_ref(),
        };
        path.and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .or_else(|| self.tabs.slots[index].recovered_title.clone())
            .unwrap_or_else(|| {
                let number = self.tabs.slots[index].number;
                if self.language_english {
                    format!("Untitled {number}")
                } else {
                    format!("無題 {number}")
                }
            })
    }

    /// Closes a tab without asking. The last tab is replaced by a blank
    /// canvas of the same size.
    pub(crate) fn close_tab_now(&mut self, index: usize) {
        if index == self.tabs.active {
            self.finish_pending_canvas_gesture();
            self.commit_pending_guide_edit();
        }
        if let Some(slot) = self.tabs.slots.get(index) {
            let document_id = slot
                .parked
                .as_ref()
                .map_or(self.history.document_id(), |tab| tab.history.document_id());
            if let Some(directory) = self.recovery.current_dir() {
                let _ = self.io_task_sender.send(IoTask::ClearRecovery {
                    directory: directory.to_path_buf(),
                    document_id,
                    saved_token: None,
                    repaint: egui::Context::default(),
                });
            }
            self.recovery.queued_tokens.remove(&document_id);
            self.finish_timelapse_on_tab_close(document_id);
            if self
                .macro_recording
                .as_ref()
                .is_some_and(|recording| recording.document_id == document_id)
            {
                self.macro_recording = None;
                self.macro_editor = None;
            }
        }
        if index >= self.tabs.slots.len() {
            return;
        }
        if let Some(target) = self.tabs.confirm_close {
            if target == index {
                self.tabs.confirm_close = None;
            } else if target > index {
                self.tabs.confirm_close = Some(target - 1);
            }
        }
        if self.tabs.slots.len() == 1 {
            let (width, height, dpi) = (self.doc.width, self.doc.height, self.doc.dpi);
            self.replace_document(Document::new(width, height), None);
            self.doc.dpi = dpi;
            self.tabs.slots[0].number = self.tabs.next_number;
            self.tabs.slots[0].recovered_title = None;
            self.tabs.next_number += 1;
            self.reset_transient_state();
            return;
        }
        if index == self.tabs.active {
            // Show a neighbour first, then drop the closed one.
            let neighbour = if index + 1 < self.tabs.slots.len() {
                index + 1
            } else {
                index - 1
            };
            self.switch_tab(neighbour);
        }
        self.tabs.slots.remove(index);
        if self.tabs.active > index {
            self.tabs.active -= 1;
        }
    }

    /// Marks the tab whose document `document_id` belongs to as saved.
    pub(crate) fn mark_tab_saved(
        &mut self,
        document_id: u64,
        state_token: u64,
        path: &std::path::Path,
    ) -> bool {
        for slot in &mut self.tabs.slots {
            if let Some(tab) = &mut slot.parked
                && tab.history.document_id() == document_id
            {
                tab.history.mark_saved(state_token);
                tab.doc_path = Some(path.to_path_buf());
                return true;
            }
        }
        false
    }

    /// The row of document tabs above the canvas.
    pub(crate) fn document_tabs_ui(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let mut switch_to = None;
        let mut close = None;
        egui::Frame::NONE
            .fill(Color32::from_rgb(24, 25, 31))
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("document_tabs")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for index in 0..self.tabs.slots.len() {
                                let active = index == self.tabs.active;
                                let dirty = self.tab_is_dirty(index);
                                let title = format!(
                                    "{}{}",
                                    self.tab_title(index),
                                    if dirty { " ●" } else { "" }
                                );
                                let fill = if active {
                                    Color32::from_rgb(52, 50, 74)
                                } else {
                                    Color32::from_rgb(34, 35, 42)
                                };
                                egui::Frame::NONE
                                    .fill(fill)
                                    .corner_radius(6.0)
                                    .inner_margin(egui::Margin::symmetric(8, 3))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let text =
                                                egui::RichText::new(title).color(if active {
                                                    Color32::WHITE
                                                } else {
                                                    layout::MUTED_TEXT
                                                });
                                            if ui
                                                .add(
                                                    egui::Label::new(text)
                                                        .sense(egui::Sense::click()),
                                                )
                                                .clicked()
                                            {
                                                switch_to = Some(index);
                                            }
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("×")
                                                            .color(layout::MUTED_TEXT),
                                                    )
                                                    .frame(false)
                                                    .small(),
                                                )
                                                .on_hover_text(if english {
                                                    "Close"
                                                } else {
                                                    "閉じる"
                                                })
                                                .clicked()
                                            {
                                                close = Some(index);
                                            }
                                        });
                                    });
                            }
                            if ui
                                .add(egui::Button::new("＋").frame(false))
                                .on_hover_text(if english {
                                    "New Canvas…"
                                } else {
                                    "新規キャンバス…"
                                })
                                .clicked()
                            {
                                self.show_new_document = true;
                            }
                        });
                    });
            });
        if let Some(index) = switch_to {
            self.switch_tab(index);
        }
        if let Some(index) = close {
            self.request_close_tab(index);
        }
    }

    /// Closes a tab, asking first when it has unsaved changes.
    pub(crate) fn request_close_tab(&mut self, index: usize) {
        if index == self.tabs.active {
            self.finish_pending_canvas_gesture();
        }
        let Some(slot) = self.tabs.slots.get(index) else {
            return;
        };
        let document_id = slot
            .parked
            .as_ref()
            .map_or(self.history.document_id(), |tab| tab.history.document_id());
        let has_timelapse = self
            .timelapse_sessions
            .get(&document_id)
            .is_some_and(|session| session.recording);
        let has_macro = self
            .macro_recording
            .as_ref()
            .is_some_and(|recording| recording.document_id == document_id);
        if has_timelapse || has_macro {
            let message = match (has_timelapse, has_macro) {
                (true, true) => {
                    "このタブのタイムラプスは終了し、保存前のマクロ記録は破棄されます。タイムラプスの画像は後で動画に書き出せます。閉じますか？"
                }
                (true, false) => {
                    "タイムラプス記録を終了してタブを閉じますか？ 記録画像は後で動画に書き出せます。"
                }
                (false, true) => "保存前のマクロ記録を破棄してタブを閉じますか？",
                (false, false) => unreachable!(),
            };
            if rfd::MessageDialog::new()
                .set_title("記録中のタブ")
                .set_description(message)
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show()
                != rfd::MessageDialogResult::Ok
            {
                return;
            }
        }
        if self.tab_is_dirty(index) {
            self.tabs.confirm_close = Some(index);
        } else {
            self.close_tab_now(index);
        }
    }

    /// Confirmation for closing a tab with unsaved changes.
    pub(crate) fn close_tab_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.tabs.confirm_close else {
            return;
        };
        if index >= self.tabs.slots.len() {
            self.tabs.confirm_close = None;
            return;
        }
        let english = self.language_english;
        let title = self.tab_title(index);
        let mut decision = None;
        egui::Window::new(if english {
            "Close Tab"
        } else {
            "タブを閉じる"
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(if english {
                format!("\"{title}\" has unsaved changes. Close it without saving?")
            } else {
                format!("「{title}」には保存していない変更があります。保存せずに閉じますか？")
            });
            ui.horizontal(|ui| {
                if ui
                    .button(if english {
                        "Close Without Saving"
                    } else {
                        "保存せずに閉じる"
                    })
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui
                    .button(if english { "Cancel" } else { "キャンセル" })
                    .clicked()
                {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                self.tabs.confirm_close = None;
                self.close_tab_now(index);
            }
            Some(false) => self.tabs.confirm_close = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish_io(app: &mut EfudeApp, ctx: &egui::Context) {
        for _ in 0..200 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.update_ui(ctx));
            if app.io_task_sender.busy.get() == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("I/O did not finish: {}", app.status);
    }

    #[test]
    fn pending_image_import_stays_with_its_original_tab() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("import.png");
        let color = [10, 20, 30, 255];
        image::RgbaImage::from_pixel(2, 2, image::Rgba(color))
            .save(&path)
            .unwrap();
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.doc_path = Some(directory.path().join("original.efude"));
        let origin_id = app.history.document_id();
        let ctx = egui::Context::default();
        app.queue_image_layer(path, &ctx).unwrap();

        // Completion is only polled during a UI frame, so switch first even
        // if the worker has already finished decoding this small fixture.
        app.open_document_tab();
        app.replace_document(Document::new(16, 12), None);
        let active_id = app.history.document_id();
        finish_io(&mut app, &ctx);
        assert_eq!(app.history.document_id(), active_id);
        assert_eq!(app.doc.layers.len(), 1, "import changed the active tab");
        assert!(!app.history.is_dirty());
        assert_eq!(app.selected_layer, 0);

        app.switch_tab(0);
        assert_eq!(app.history.document_id(), origin_id);
        assert_eq!(app.doc.layers.len(), 2);
        assert_eq!(app.selected_layer, 1);
        assert_eq!(app.doc.layers[1].pixels.pixel(3, 3), color);
        assert!(app.history.is_dirty());
        app.undo();
        assert_eq!(app.doc.layers.len(), 1, "import must be one undo step");
        app.redo();
        assert_eq!(app.doc.layers[1].pixels.pixel(3, 3), color);
    }

    #[test]
    fn pending_image_import_is_discarded_when_its_tab_closes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("import.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let mut app = EfudeApp::default();
        app.language_english = true;
        app.doc = Document::new(8, 8);
        let ctx = egui::Context::default();
        app.queue_image_layer(path, &ctx).unwrap();
        app.close_tab_now(0);
        let replacement_id = app.history.document_id();
        finish_io(&mut app, &ctx);
        assert_eq!(app.history.document_id(), replacement_id);
        assert_eq!(app.doc.layers.len(), 1, "import changed a replacement tab");
        assert!(!app.history.is_dirty());
        assert!(app.status.contains("closed"), "{}", app.status);
    }

    #[test]
    fn pending_image_import_is_discarded_when_a_pristine_tab_is_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("import.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        for open_file in [false, true] {
            let mut app = EfudeApp::default();
            app.doc = Document::new(8, 8);
            let ctx = egui::Context::default();
            app.queue_image_layer(path.clone(), &ctx).unwrap();
            if open_file {
                app.install_document(Document::new(16, 12), directory.path().join("other.efude"));
            } else {
                app.canvas_width_input = 16;
                app.canvas_height_input = 12;
                assert!(app.new_document());
            }
            let replacement_id = app.history.document_id();
            finish_io(&mut app, &ctx);
            assert_eq!(app.tabs.slots.len(), 1);
            assert_eq!(app.history.document_id(), replacement_id);
            assert_eq!(app.doc.layers.len(), 1, "import changed a replacement tab");
            assert!(!app.history.is_dirty());
        }
    }

    #[test]
    fn pending_image_imports_preserve_queue_order_and_independent_undo() {
        let directory = tempfile::tempdir().unwrap();
        let colors = [[10, 20, 30, 255], [40, 50, 60, 255]];
        for parked in [false, true] {
            let mut app = EfudeApp::default();
            app.doc = Document::new(8, 8);
            app.doc_path = Some(directory.path().join("original.efude"));
            let ctx = egui::Context::default();
            for (index, color) in colors.iter().enumerate() {
                let path = directory.path().join(format!("import-{index}.png"));
                image::RgbaImage::from_pixel(2, 2, image::Rgba(*color))
                    .save(&path)
                    .unwrap();
                app.queue_image_layer(path, &ctx).unwrap();
            }
            if parked {
                app.open_document_tab();
                app.replace_document(Document::new(16, 12), None);
            }
            finish_io(&mut app, &ctx);
            if parked {
                assert_eq!(app.doc.layers.len(), 1);
                assert!(!app.history.is_dirty());
                app.switch_tab(0);
            }
            assert_eq!(app.doc.layers.len(), 3);
            assert_eq!(app.selected_layer, 2);
            for (index, color) in colors.iter().enumerate() {
                assert_eq!(app.doc.layers[index + 1].pixels.pixel(3, 3), *color);
            }
            app.undo();
            assert_eq!(app.doc.layers.len(), 2);
            assert_eq!(app.doc.layers[1].pixels.pixel(3, 3), colors[0]);
            app.undo();
            assert_eq!(app.doc.layers.len(), 1);
            assert!(!app.history.is_dirty());
            app.redo();
            app.redo();
            assert_eq!(app.doc.layers.len(), 3);
            assert_eq!(app.doc.layers[2].pixels.pixel(3, 3), colors[1]);
        }
    }

    #[test]
    fn pending_paste_and_texture_survive_a_tab_round_trip() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.doc.layers[0].pixels.set_pixel(0, 0, [0, 0, 0, 255]);
        app.open_document_tab();
        let pixels = vec![255; 16];
        let texture = egui::Context::default().load_texture(
            "pending-paste-test",
            egui::ColorImage::from_rgba_unmultiplied([2, 2], &pixels),
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        app.paste_preview = Some((2, 2, pixels.clone(), Vec2::new(-1.0, 3.0)));
        app.paste_texture = Some(texture);
        app.switch_tab(0);
        assert!(app.paste_preview.is_none());
        app.switch_tab(1);
        let preview = app.paste_preview.as_ref().unwrap();
        assert_eq!(preview.2, pixels);
        assert_eq!(preview.3, Vec2::new(-1.0, 3.0));
        assert_eq!(app.paste_texture.as_ref().unwrap().id(), id);
    }

    #[test]
    fn opening_a_document_preserves_a_paste_on_an_otherwise_blank_tab() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        app.paste_preview = Some((2, 2, vec![255; 16], Vec2::ZERO));
        app.open_document_tab();
        assert_eq!(app.tabs.slots.len(), 2);
        assert!(app.paste_preview.is_none());
        app.switch_tab(0);
        assert!(app.paste_preview.is_some());
    }

    #[test]
    fn book_page_paths_find_existing_active_and_parked_tabs() {
        let directory = tempfile::tempdir().unwrap();
        let page = directory.path().join("001.efude");
        efude_io::save(&page, &Document::new(8, 8)).unwrap();
        let mut app = EfudeApp::default();
        app.install_document(Document::new(8, 8), page.canonicalize().unwrap());
        let book_page = directory.path().join(".").join("001.efude");
        assert_eq!(app.tab_with_path(&book_page), Some(0));

        app.install_document(Document::new(8, 8), directory.path().join("002.efude"));
        assert_eq!(app.tab_with_path(&book_page), Some(0));
        assert_eq!(app.tab_with_path(&directory.path().join("003.efude")), None);
        #[cfg(windows)]
        assert_eq!(
            app.tab_with_path(std::path::Path::new(
                &book_page.to_string_lossy().to_uppercase()
            )),
            Some(0)
        );
    }

    #[test]
    fn closing_recorded_tabs_queues_the_last_edit_before_discarding_the_session() {
        for parked in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut app = EfudeApp::default();
            app.doc = Document::new(8, 8);
            app.doc.guide = efude_canvas::GuideImage::fit_to_canvas(1, 1, vec![255; 4], &app.doc);
            let document_id = app.history.document_id();
            let session =
                efude_io::timelapse::create_session(directory.path(), &app.doc, false, 720)
                    .unwrap();
            let session_folder = session.folder.clone();
            app.timelapse_sessions.insert(
                document_id,
                timelapse::Recording {
                    session,
                    next_index: 2,
                    frames_written: 1,
                    last_content_revision: app.history.content_revision(),
                    last_capture: std::time::Instant::now(),
                    recording: true,
                    paused: false,
                },
            );
            app.history.begin();
            app.history.record_pixel(&app.doc.layers[0], 0);
            app.doc.layers[0].pixels.set_pixel(0, 0, [10, 20, 30, 255]);
            app.history.commit();
            if parked {
                app.install_document(Document::new(8, 8), directory.path().join("other.efude"));
            }
            let (sender, receiver) = std::sync::mpsc::sync_channel(8);
            app.timelapse_worker.sender = sender;
            app.close_tab_now(0);
            let task = receiver.try_recv().expect("closing dropped the final edit");
            match task {
                timelapse::Task::Frame {
                    document_id: id,
                    document,
                    session,
                    index,
                    ..
                } => {
                    assert_eq!(id, document_id);
                    assert_eq!(session.folder, session_folder);
                    assert_eq!(index, 2);
                    assert_eq!(document.layers[0].pixels.pixel(0, 0), [10, 20, 30, 255]);
                    assert!(
                        document.guide.is_none(),
                        "private guide leaked into final frame"
                    );
                }
                _ => panic!("closing queued an unexpected task"),
            }
            assert!(!app.timelapse_sessions.contains_key(&document_id));
        }
    }

    #[test]
    fn close_confirmation_keeps_its_target_when_an_earlier_tab_is_closed_in_the_ui() {
        let directory = tempfile::tempdir().unwrap();
        let mut h = crate::tool_tests::Harness::new(8, 8);
        h.app.language_english = true;
        h.app
            .install_document(Document::new(8, 8), directory.path().join("clean.efude"));
        h.app
            .install_document(Document::new(8, 8), directory.path().join("target.efude"));
        h.app.add_raster_layer();
        let target_id = h.app.history.document_id();
        h.app
            .install_document(Document::new(8, 8), directory.path().join("other.efude"));
        h.app.add_raster_layer();
        let other_id = h.app.history.document_id();
        h.app.request_close_tab(1);
        h.frames(3);

        // The confirmation is an ordinary window. The tab bar remains usable,
        // so clicking its first close button removes the earlier clean tab.
        h.click_label("×");
        assert_eq!(h.app.tabs.slots.len(), 2);
        h.click_label("Close Without Saving");
        assert_eq!(h.app.tabs.slots.len(), 1);
        assert_eq!(h.app.history.document_id(), other_id);
        assert_ne!(h.app.history.document_id(), target_id);
        assert!(h.app.history.is_dirty());
    }

    fn queued_filter(
        app: &mut EfudeApp,
        ctx: &egui::Context,
    ) -> (IoTask, std::sync::mpsc::Sender<IoCompletion>) {
        let (task_sender, task_receiver) = std::sync::mpsc::channel();
        let (completion_sender, completion_receiver) = std::sync::mpsc::channel();
        app.io_task_sender.sender = task_sender;
        app.io_receiver = completion_receiver;
        app.queue_filter(
            FilterOperation::Image(efude_canvas::filters::Filter::Mosaic { size: 2 }),
            ctx,
        );
        assert!(app.filter_pending);
        let task = task_receiver.try_recv().expect("filter was queued");
        (task, completion_sender)
    }

    fn complete_filter(task: IoTask, sender: &std::sync::mpsc::Sender<IoCompletion>) -> Vec<u8> {
        let IoTask::ApplyFilter {
            mut layer,
            before,
            selection,
            width,
            height,
            operation,
            document_id,
            state_token,
            ..
        } = task
        else {
            panic!("expected the real queued filter task");
        };
        run_filter(&mut layer, width, height, operation, (0, 0));
        if let (Some(mask), Some(before)) = (selection, before) {
            for y in 0..height {
                for x in 0..width {
                    let coverage = mask[(y * width + x) as usize] as f32 / 255.0;
                    let pixel = blend_selection_pixel(
                        before.pixel(x, y),
                        layer.pixels.pixel(x, y),
                        coverage,
                    );
                    layer.pixels.set_pixel(x, y, pixel);
                }
            }
            layer.pixels.prune_empty_tiles();
        }
        let expected = layer.pixels.to_dense();
        assert!(
            sender
                .send(IoCompletion::FilterApplied {
                    layer_id: layer.id,
                    document_id,
                    state_token,
                    pixels: layer.pixels,
                })
                .is_ok()
        );
        expected
    }

    fn filter_fixture() -> EfudeApp {
        let mut app = EfudeApp::default();
        app.doc = Document::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                app.doc.layers[0]
                    .pixels
                    .set_pixel(x, y, [x as u8 * 20, y as u8 * 25, 120, 255]);
            }
        }
        app
    }

    #[test]
    fn pending_filter_applies_to_its_original_parked_tab() {
        let mut app = filter_fixture();
        let ctx = egui::Context::default();
        app.selection.active = true;
        app.selection.mask = (0..64).map(|i| [0, 128, 255][i % 3]).collect();
        let origin_id = app.history.document_id();
        let original = app.doc.layers[0].pixels.to_dense();
        let (task, sender) = queued_filter(&mut app, &ctx);
        app.open_document_tab();
        app.replace_document(Document::new(16, 12), None);
        let active_id = app.history.document_id();
        let active_pixels = app.doc.layers[0].pixels.to_dense();
        let expected = complete_filter(task, &sender);
        assert_ne!(
            expected, original,
            "the real mosaic must change the fixture"
        );
        finish_io(&mut app, &ctx);
        assert!(!app.filter_pending);
        assert_eq!(app.history.document_id(), active_id);
        assert_eq!(app.doc.layers[0].pixels.to_dense(), active_pixels);
        assert!(!app.history.is_dirty());
        let origin = app.tabs.slots[0].parked.as_ref().unwrap();
        assert_eq!(origin.history.document_id(), origin_id);
        assert_eq!(origin.doc.layers[0].pixels.to_dense(), expected);
        assert!(origin.history.is_dirty());
        app.switch_tab(0);
        app.undo();
        assert_eq!(app.doc.layers[0].pixels.to_dense(), original);
        assert!(!app.history.is_dirty(), "filter must be one Undo step");
        app.redo();
        assert_eq!(app.doc.layers[0].pixels.to_dense(), expected);
    }

    #[test]
    fn pending_filter_does_not_overwrite_a_changed_parked_tab() {
        let mut app = filter_fixture();
        let ctx = egui::Context::default();
        let (task, sender) = queued_filter(&mut app, &ctx);
        app.history.begin();
        app.history.record_pixel(&app.doc.layers[0], 0);
        app.doc.layers[0].pixels.set_pixel(0, 0, [9, 8, 7, 255]);
        app.history.commit();
        let edited = app.doc.layers[0].pixels.to_dense();
        let edited_token = app.history.state_token();
        app.open_document_tab();
        app.replace_document(Document::new(16, 12), None);
        complete_filter(task, &sender);
        finish_io(&mut app, &ctx);
        let origin = app.tabs.slots[0].parked.as_ref().unwrap();
        assert_eq!(origin.history.state_token(), edited_token);
        assert_eq!(origin.doc.layers[0].pixels.to_dense(), edited);
        assert!(!app.history.is_dirty());
        app.switch_tab(0);
        app.undo();
        assert_eq!(app.doc.layers[0].pixels.pixel(0, 0), [0, 0, 120, 255]);
        assert!(!app.history.is_dirty());
    }

    #[test]
    fn pending_filter_is_discarded_when_its_tab_closes() {
        let mut app = filter_fixture();
        let ctx = egui::Context::default();
        let (task, sender) = queued_filter(&mut app, &ctx);
        app.close_tab_now(0);
        let replacement_id = app.history.document_id();
        let replacement = app.doc.layers[0].pixels.to_dense();
        complete_filter(task, &sender);
        finish_io(&mut app, &ctx);
        assert_eq!(app.history.document_id(), replacement_id);
        assert_eq!(app.doc.layers[0].pixels.to_dense(), replacement);
        assert!(!app.history.is_dirty());
        assert!(!app.filter_pending);
    }

    #[test]
    fn pending_filter_obeys_parked_layer_restrictions() {
        for restriction in [
            "locked",
            "reference",
            "reference-folder",
            "folder",
            "missing",
        ] {
            let mut app = filter_fixture();
            let ctx = egui::Context::default();
            let original = app.doc.layers[0].pixels.to_dense();
            let (task, sender) = queued_filter(&mut app, &ctx);
            app.open_document_tab();
            app.replace_document(Document::new(16, 12), None);
            let origin = app.tabs.slots[0].parked.as_mut().unwrap();
            // Keep the token unchanged so the completion must also enforce
            // the destination layer's current eligibility.
            match restriction {
                "locked" => origin.doc.layers[0].locked = true,
                "reference" => origin.doc.layers[0].reference = true,
                "reference-folder" => {
                    let mut folder = efude_canvas::Layer::new(2, "reference", 8, 8);
                    folder.kind = LayerKind::Folder;
                    folder.reference = true;
                    origin.doc.layers[0].parent_id = Some(folder.id);
                    origin.doc.layers.push(folder);
                }
                "folder" => origin.doc.layers[0].kind = LayerKind::Folder,
                "missing" => origin.doc.layers.clear(),
                _ => unreachable!(),
            }
            complete_filter(task, &sender);
            finish_io(&mut app, &ctx);
            let origin = app.tabs.slots[0].parked.as_ref().unwrap();
            assert!(!origin.history.is_dirty(), "{restriction}");
            assert!(!origin.history.can_undo(), "{restriction}");
            if let Some(layer) = origin.doc.layers.first() {
                assert_eq!(layer.pixels.to_dense(), original, "{restriction}");
            }
            assert!(!app.history.is_dirty(), "{restriction}");
            assert!(!app.filter_pending, "{restriction}");
        }
    }

    #[test]
    fn pending_filter_does_not_overwrite_an_unfinished_move() {
        let mut app = filter_fixture();
        app.tool = Tool::Move;
        // One document pixel must cross egui's screen-space drag threshold.
        app.zoom = 16.0;
        app.navigator_center = Vec2::new(4.0, 4.0);
        app.use_windows_ink = false;
        app.use_wintab = false;
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut frame = |app: &mut EfudeApp, events| {
            time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.update_ui(ctx));
        };
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        let original = app.doc.layers[0].pixels.to_dense();
        let state_token = app.history.state_token();
        let (task, sender) = queued_filter(&mut app, &ctx);
        let (rect, scale) = app.canvas_screen.unwrap();
        let start = rect.center();
        assert!(app.canvas_viewport.0.contains(start));
        frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let end = start + Vec2::new(scale, 0.0);
        frame(&mut app, vec![egui::Event::PointerMoved(end)]);
        assert!(
            app.move_origin.is_some(),
            "the real Move gesture must start"
        );
        assert!(app.history.is_active());
        assert!(app.active.is_empty(), "Move does not use brush samples");
        assert_eq!(app.history.state_token(), state_token);
        let moved = app.doc.layers[0].pixels.to_dense();
        assert_ne!(moved, original);
        assert_eq!(app.doc.layers[0].pixels.pixel(0, 0)[3], 0);
        complete_filter(task, &sender);
        frame(&mut app, Vec::new());
        assert!(!app.filter_pending);
        assert_eq!(
            app.doc.layers[0].pixels.pixel(0, 0)[3],
            0,
            "the stale filter repainted the unfinished Move's cleared source"
        );
        assert_eq!(app.doc.layers[0].pixels.to_dense(), moved);
        assert!(
            app.history.is_active(),
            "Move must retain its Undo transaction"
        );
        frame(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!app.history.is_active());
        app.undo();
        assert_eq!(app.doc.layers[0].pixels.to_dense(), original);
        assert!(!app.history.is_dirty(), "Move must remain one Undo step");
        app.redo();
        assert_eq!(app.doc.layers[0].pixels.to_dense(), moved);
    }
}
