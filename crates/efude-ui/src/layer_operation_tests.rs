// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Regression coverage for layer operations and repeated history navigation.

use super::*;

struct BatchedPanelHarness {
    app: EfudeApp,
    ctx: egui::Context,
    time: f64,
}

impl BatchedPanelHarness {
    fn new(pane: layout::Pane) -> Self {
        let mut app = EfudeApp::default();
        app.language_english = true;
        app.doc = Document::new(120, 80);
        app.navigator_center = Vec2::new(60.0, 40.0);
        app.use_windows_ink = false;
        app.use_wintab = false;
        app.show_tools_panel = false;
        app.size = 8.0;
        app.color = Color32::BLACK;
        app.brushes[app.selected_brush].opacity = 0.5;
        // A supported dock arrangement that renders the command before Canvas.
        app.workspace = egui_dock::DockState::new(vec![pane]);
        app.workspace.main_surface_mut().split_right(
            egui_dock::NodeIndex::root(),
            0.36,
            vec![layout::Pane::Canvas],
        );
        let ctx = egui::Context::default();
        layout::apply_theme(&ctx);
        Self {
            app,
            ctx,
            time: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 1.0 / 60.0;
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 1600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ctx| self.app.update_ui(ctx),
        )
    }

    fn label_position(&mut self, label: &str) -> Pos2 {
        fn find(shape: &egui::Shape, label: &str) -> Option<Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
                _ => None,
            }
        }
        for _ in 0..3 {
            self.frame(Vec::new());
        }
        let output = self.frame(Vec::new());
        output
            .shapes
            .iter()
            .find_map(|s| find(&s.shape, label))
            .unwrap_or_else(|| panic!("control was not rendered: {label}"))
            .center()
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn click(&mut self, pos: Pos2) {
        self.frame(vec![
            egui::Event::PointerMoved(pos),
            Self::button(pos, true),
        ]);
        self.frame(vec![Self::button(pos, false)]);
    }

    fn canvas_pos(&self, x: f32, y: f32) -> Pos2 {
        let (rect, scale) = self.app.canvas_screen.unwrap();
        rect.center() + Vec2::new(x - 60.0, y - 40.0) * scale
    }

    fn begin_stroke(&mut self) -> Pos2 {
        self.begin_stroke_with_packets(false)
    }

    fn queue_pen_point(&mut self, x: f32, y: f32) {
        let pos = self.canvas_pos(x, y);
        self.app.pen_queue.push(efude_input::PenPacket {
            x: pos.x,
            y: pos.y,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            rotation: 0.0,
            time_ms: ((self.time + 1.0 / 60.0) * 1000.0) as u64,
            received_at: std::time::Instant::now(),
        });
    }

    fn begin_stroke_with_packets(&mut self, native: bool) -> Pos2 {
        self.app.use_windows_ink = native;
        let start = self.canvas_pos(20.0, 40.0);
        if native {
            self.queue_pen_point(20.0, 40.0);
        }
        self.frame(vec![
            egui::Event::PointerMoved(start),
            Self::button(start, true),
        ]);
        let end = self.canvas_pos(55.0, 40.0);
        for x in [25.0, 35.0, 45.0, 55.0] {
            if native {
                self.queue_pen_point(x, 40.0);
            }
            self.frame(vec![egui::Event::PointerMoved(self.canvas_pos(x, 40.0))]);
        }
        assert!(self.app.history.is_active());
        assert!(!self.app.history.can_undo());
        assert!(!self.app.active.is_empty());
        end
    }

    fn release_and_click(&mut self, end: Pos2, control: Pos2) {
        // Sequential real events can be batched by a slow frame: pen-up,
        // then a click of a panel control before another frame is rendered.
        self.frame(vec![
            Self::button(end, false),
            egui::Event::PointerMoved(control),
            Self::button(control, true),
            Self::button(control, false),
        ]);
    }

    fn assert_history_roundtrip(&mut self, initial: &Document) {
        fn mask_values(doc: &Document) -> Option<Vec<u8>> {
            doc.layers[0].mask.as_ref().map(|mask| {
                (0..doc.width * doc.height)
                    .map(|i| mask.pixel_or_tile_default(i % doc.width, i / doc.width, [255; 4])[0])
                    .collect()
            })
        }
        let final_doc = self.app.doc.clone();
        assert!(!self.app.history.is_active());
        assert!(self.app.active.is_empty());
        assert!(self.app.stroke_builder.is_none());
        // An eventual repeated release must not reapply a stale held stroke.
        let farther = self.canvas_pos(95.0, 40.0);
        self.frame(vec![
            egui::Event::PointerMoved(farther),
            Self::button(farther, false),
        ]);
        assert!(
            self.app.doc.layers[0].pixels.to_dense() == final_doc.layers[0].pixels.to_dense(),
            "late release changed paint"
        );
        assert!(
            mask_values(&self.app.doc) == mask_values(&final_doc),
            "late release changed mask"
        );
        self.app.undo();
        assert!(
            self.app.doc.layers[0].pixels.to_dense() != final_doc.layers[0].pixels.to_dense()
                || mask_values(&self.app.doc) != mask_values(&final_doc),
            "the panel command must change paint or mask"
        );
        assert!(
            self.app.history.can_undo(),
            "the original stroke must remain a separate Undo step"
        );
        self.app.undo();
        assert!(
            self.app.doc.layers[0].pixels.to_dense() == initial.layers[0].pixels.to_dense(),
            "two Undo steps left paint from the unfinished stroke"
        );
        assert!(
            mask_values(&self.app.doc) == mask_values(initial),
            "two Undo steps left mask paint from the unfinished stroke"
        );
        assert!(!self.app.history.can_undo());
        self.app.redo();
        self.app.redo();
        assert!(
            self.app.doc.layers[0].pixels.to_dense() == final_doc.layers[0].pixels.to_dense(),
            "Redo did not restore paint"
        );
        assert!(
            mask_values(&self.app.doc) == mask_values(&final_doc),
            "Redo did not restore mask"
        );
        assert!(!self.app.history.can_redo());
    }
}

fn assert_panel_final_contact(canvas_first: bool, native: bool, batched: bool) {
    let mut h = BatchedPanelHarness::new(layout::Pane::Brushes);
    // A one-preset brush set keeps the tested control visible in either dock;
    // the selected brush and its stroke settings are unchanged.
    let brush = h.app.brushes[h.app.selected_brush].clone();
    h.app.brushes = vec![brush];
    h.app.selected_brush = 0;
    if canvas_first {
        // Keep the shipped dock topology, with Canvas drawn before Brushes.
        h.app.workspace = layout::default_workspace();
        let brush_tab = h.app.workspace.find_tab(&layout::Pane::Brushes).unwrap();
        h.app.workspace.set_active_tab(brush_tab);
    }
    let header = h.label_position("Import, Export, Input Logs and Comparison");
    h.click(header);
    let control = h.label_position("Replay Last Stroke");
    let initial = h.app.doc.clone();
    h.begin_stroke_with_packets(native);
    let end = h.canvas_pos(75.0, 40.0);
    if native {
        h.queue_pen_point(75.0, 40.0);
    }
    let mut release = vec![
        egui::Event::PointerMoved(end),
        BatchedPanelHarness::button(end, false),
    ];
    if batched {
        release.extend([
            egui::Event::PointerMoved(control),
            BatchedPanelHarness::button(control, true),
            BatchedPanelHarness::button(control, false),
        ]);
    }
    h.frame(release);
    if !batched {
        // Same release and command, separated by a rendered frame.
        h.click(control);
    }
    let last = h
        .app
        .stroke_log
        .strokes
        .last()
        .and_then(|stroke| stroke.last())
        .expect("the original stroke must be recorded");
    let final_contact = (last.x, last.y, last.pressure);
    let final_pixels = h.app.doc.layers[0].pixels.to_dense();
    h.app.undo();
    let original_endpoint_alpha = h.app.doc.layers[0].pixels.pixel(70, 40)[3];
    assert!(
        h.app.history.can_undo(),
        "Replay must be a separate Undo step"
    );
    h.app.undo();
    assert!(
        h.app.doc.layers[0].pixels.to_dense() == initial.layers[0].pixels.to_dense(),
        "two Undo steps must restore the initial image"
    );
    assert!(!h.app.history.can_undo());
    h.app.redo();
    h.app.redo();
    assert!(
        h.app.doc.layers[0].pixels.to_dense() == final_pixels,
        "two Redo steps must restore the final image"
    );
    assert!(!h.app.history.can_redo());
    assert!(
        (final_contact.0 - 75.0).abs() < 0.01
            && (final_contact.1 - 40.0).abs() < 0.01
            && original_endpoint_alpha > 0,
        "final contact or original stroke tail was lost: canvas_first={canvas_first}, native={native}, batched={batched}, logged={final_contact:?}, original_alpha_at_70={original_endpoint_alpha}"
    );
    assert!((final_contact.2 - 1.0).abs() < 0.01);
}

macro_rules! panel_final_contact_test {
    ($name:ident, $canvas_first:expr, $native:expr, $batched:expr) => {
        #[test]
        fn $name() {
            assert_panel_final_contact($canvas_first, $native, $batched);
        }
    };
}

panel_final_contact_test!(
    panel_final_contact_brush_first_window_isolated,
    false,
    false,
    false
);
panel_final_contact_test!(
    panel_final_contact_brush_first_window_batch,
    false,
    false,
    true
);
panel_final_contact_test!(
    panel_final_contact_brush_first_native_isolated,
    false,
    true,
    false
);
panel_final_contact_test!(
    panel_final_contact_brush_first_native_batch,
    false,
    true,
    true
);
panel_final_contact_test!(
    panel_final_contact_canvas_first_window_isolated,
    true,
    false,
    false
);
panel_final_contact_test!(
    panel_final_contact_canvas_first_window_batch,
    true,
    false,
    true
);
panel_final_contact_test!(
    panel_final_contact_canvas_first_native_isolated,
    true,
    true,
    false
);
panel_final_contact_test!(
    panel_final_contact_canvas_first_native_batch,
    true,
    true,
    true
);

#[test]
fn batched_panel_replay_preserves_the_finished_strokes_undo() {
    let mut h = BatchedPanelHarness::new(layout::Pane::Brushes);
    let header = h.label_position("Import, Export, Input Logs and Comparison");
    h.click(header);
    let control = h.label_position("Replay Last Stroke");
    h.app.stroke_log.push(&[
        InkPoint::new(20.0, 15.0, 1.0, 0),
        InkPoint::new(100.0, 15.0, 1.0, 1),
    ]);
    let initial = h.app.doc.clone();
    let end = h.begin_stroke();
    h.release_and_click(end, control);
    assert_eq!(
        h.app.doc.layers[0].pixels.pixel(35, 15)[3],
        0,
        "replay must use the just-finished stroke, not the older input log"
    );
    h.assert_history_roundtrip(&initial);
}

fn assert_batched_panel_mask_action(label: &str) {
    let mut h = BatchedPanelHarness::new(layout::Pane::Layers);
    h.app.doc.layers[0].pixels.fill_shared([120, 40, 200, 255]);
    h.app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(120, 80));
    h.app.editing_mask = true;
    h.app.selection.active = true;
    h.app.selection.mask = vec![200; 120 * 80];
    let control = h.label_position(label);
    let initial = h.app.doc.clone();
    let end = h.begin_stroke();
    h.release_and_click(end, control);
    h.assert_history_roundtrip(&initial);
}

#[test]
fn batched_panel_mask_invert_preserves_the_finished_strokes_undo() {
    assert_batched_panel_mask_action("Invert Mask");
}

#[test]
fn batched_panel_mask_from_selection_preserves_the_finished_strokes_undo() {
    assert_batched_panel_mask_action("From Selection");
}

#[test]
fn panel_commands_preserve_the_result_when_a_held_stroke_is_released_later() {
    for action in [None, Some(0), Some(1)] {
        let mut h = BatchedPanelHarness::new(layout::Pane::Layers);
        if action.is_some() {
            h.app.doc.layers[0].pixels.fill_shared([120, 40, 200, 255]);
            h.app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(120, 80));
            h.app.editing_mask = true;
            h.app.selection.active = true;
            h.app.selection.mask = vec![200; 120 * 80];
        }
        h.frame(Vec::new());
        let initial = h.app.doc.clone();
        h.begin_stroke();
        match action {
            None => h.app.replay_last_stroke(),
            Some(action) => h.app.apply_layer_mask_action(0, action),
        }
        h.assert_history_roundtrip(&initial);
    }
}

fn click_panel_button(
    app: &mut EfudeApp,
    panel: fn(&mut EfudeApp, &mut egui::Ui, &egui::Context),
    label: &str,
) {
    fn text_rect(shape: &egui::Shape, label: &str) -> Option<Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.galley.rect.translate(text.pos.to_vec2()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, label)),
            _ => None,
        }
    }
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let mut frame = |app: &mut EfudeApp, events| {
        time += 1.0 / 60.0;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 1600.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel(app, ui, ctx));
            },
        )
    };
    for _ in 0..2 {
        let _ = frame(app, Vec::new());
    }
    let output = frame(app, Vec::new());
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| text_rect(&shape.shape, label))
        .unwrap_or_else(|| panic!("button was not rendered: {label}"))
        .center();
    let _ = frame(
        app,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let _ = frame(
        app,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
}

#[test]
fn layer_panel_buttons_at_the_native_limit_preserve_the_document() {
    for label in ["+ Child Layer", "+ Folder"] {
        let mut app = EfudeApp::default();
        app.language_english = true;
        app.doc = Document::new(1, 1);
        app.doc.layers[0].pixels.set_pixel(0, 0, [200, 40, 80, 255]);
        for id in 2..=2000 {
            app.doc
                .layers
                .push(efude_canvas::Layer::new(id, "layer", 1, 1));
        }
        let state = app.history.state_token();
        click_panel_button(&mut app, EfudeApp::layers_ui, label);
        assert_eq!(app.doc.layers.len(), 2000, "{label}");
        assert_eq!(app.selected_layer, 0);
        assert_eq!(app.history.state_token(), state);
        assert!(!app.history.can_undo());
        assert!(!app.history.is_active());
        let directory = tempfile::tempdir().unwrap();
        efude_io::save(&directory.path().join("panel-limit.efude"), &app.doc).unwrap();
    }
}

#[test]
fn brush_comparison_near_the_native_limit_does_not_create_partial_layers() {
    let mut app = EfudeApp::default();
    app.language_english = true;
    app.doc = Document::new(1, 1);
    for id in 2..=1999 {
        app.doc
            .layers
            .push(efude_canvas::Layer::new(id, "layer", 1, 1));
    }
    app.comparison_brush = Some(app.brushes[app.selected_brush].clone());
    app.stroke_log.push(&[InkPoint::new(0.0, 0.0, 1.0, 0)]);
    let state = app.history.state_token();
    let brush_count = app.brushes.len();
    let brush = app.selected_brush;
    let size = app.size;
    click_panel_button(
        &mut app,
        EfudeApp::brush_io_ui,
        "Replay Comparison on New Layers",
    );
    assert_eq!(app.doc.layers.len(), 1999);
    assert_eq!(app.selected_layer, 0);
    assert_eq!(app.history.state_token(), state);
    assert!(!app.history.can_undo());
    assert!(!app.history.is_active());
    assert_eq!(app.brushes.len(), brush_count);
    assert_eq!(app.selected_brush, brush);
    assert_eq!(app.size, size);
    let directory = tempfile::tempdir().unwrap();
    efude_io::save(&directory.path().join("comparison-limit.efude"), &app.doc).unwrap();
}

#[test]
fn creating_layers_at_the_native_limit_preserves_the_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("creation-limit.efude");
    let mut doc = Document::new(1, 1);
    doc.layers[0].pixels.set_pixel(0, 0, [200, 40, 80, 255]);
    for id in 2..=2000 {
        doc.layers.push(efude_canvas::Layer::new(id, "layer", 1, 1));
    }
    efude_io::save(&path, &doc).unwrap();
    for create in [
        EfudeApp::add_raster_layer as fn(&mut EfudeApp),
        EfudeApp::add_vector_layer,
        EfudeApp::duplicate_layer_subtree,
    ] {
        let mut app = EfudeApp::default();
        app.doc = efude_io::load(&path).unwrap();
        let state = app.history.state_token();
        let before = efude_canvas::composite_transparent(&app.doc);
        create(&mut app);
        assert_eq!(app.doc.layers.len(), 2000);
        assert_eq!(app.selected_layer, 0);
        assert_eq!(app.history.state_token(), state);
        assert!(!app.history.is_active());
        assert!(!app.history.can_undo());
        assert!(!app.history.is_dirty());
        assert_eq!(efude_canvas::composite_transparent(&app.doc), before);
        efude_io::save(&path, &app.doc).unwrap();
    }
}

#[test]
fn duplicating_a_subtree_near_the_native_limit_is_atomic() {
    let mut app = EfudeApp::default();
    app.doc = Document::new(1, 1);
    app.doc.layers[0].kind = LayerKind::Folder;
    let mut child = efude_canvas::Layer::new(2, "child", 1, 1);
    child.parent_id = Some(app.doc.layers[0].id);
    child.pixels.set_pixel(0, 0, [200, 40, 80, 255]);
    app.doc.layers.push(child);
    for id in 3..=1999 {
        app.doc
            .layers
            .push(efude_canvas::Layer::new(id, "layer", 1, 1));
    }
    let before = efude_canvas::composite_transparent(&app.doc);
    let state = app.history.state_token();
    app.duplicate_layer_subtree();
    assert_eq!(app.doc.layers.len(), 1999);
    assert_eq!(app.selected_layer, 0);
    assert_eq!(app.history.state_token(), state);
    assert!(!app.history.can_undo());
    assert_eq!(efude_canvas::composite_transparent(&app.doc), before);
    // A single layer still fits, and its creation remains one undoable step.
    app.selected_layer = 1;
    app.duplicate_layer_subtree();
    assert_eq!(app.doc.layers.len(), 2000);
    assert_eq!(app.doc.layers[2].parent_id, Some(app.doc.layers[0].id));
    app.undo();
    assert_eq!(app.doc.layers.len(), 1999);
    assert_eq!(efude_canvas::composite_transparent(&app.doc), before);
    app.redo();
    assert_eq!(app.doc.layers.len(), 2000);
    let directory = tempfile::tempdir().unwrap();
    efude_io::save(&directory.path().join("near-limit.efude"), &app.doc).unwrap();
}

#[test]
fn pending_image_import_at_the_native_limit_preserves_active_and_parked_tabs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("import-limit.efude");
    let mut doc = Document::new(1, 1);
    doc.layers[0].pixels.set_pixel(0, 0, [200, 40, 80, 255]);
    for id in 2..=2000 {
        doc.layers.push(efude_canvas::Layer::new(id, "layer", 1, 1));
    }
    efude_io::save(&path, &doc).unwrap();
    for parked in [false, true] {
        let mut app = EfudeApp::default();
        app.doc = efude_io::load(&path).unwrap();
        app.doc_path = Some(path.clone());
        app.language_english = true;
        let document_id = app.history.document_id();
        let state = app.history.state_token();
        if parked {
            app.open_document_tab();
            app.replace_document(Document::new(1, 1), None);
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        app.io_receiver = receiver;
        sender
            .send(IoCompletion::ImageLoaded {
                document_id,
                path: "image.png".into(),
                width: 1,
                height: 1,
                rgba: vec![10, 20, 30, 255],
            })
            .unwrap();
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
                ..Default::default()
            },
            |ctx| app.update_ui(ctx),
        );
        assert!(app.status.contains("2000"), "{}", app.status);
        if parked {
            assert_eq!(app.doc.layers.len(), 1);
            assert!(!app.history.is_dirty());
            app.switch_tab(0);
        }
        assert_eq!(app.history.document_id(), document_id);
        assert_eq!(app.doc.layers.len(), 2000);
        assert_eq!(app.selected_layer, 0);
        assert_eq!(app.history.state_token(), state);
        assert!(!app.history.is_active());
        assert!(!app.history.can_undo());
        assert!(!app.history.is_dirty());
        assert_eq!(app.doc.layers[0].pixels.pixel(0, 0), [200, 40, 80, 255]);
        efude_io::save(&path, &app.doc).unwrap();
    }
}

#[test]
fn merging_at_the_native_layer_limit_preserves_the_savable_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("layer-limit.efude");
    let mut doc = Document::new(1, 1);
    doc.layers[0].pixels.set_pixel(0, 0, [200, 40, 80, 255]);
    for id in 2..=2000 {
        doc.layers.push(efude_canvas::Layer::new(id, "layer", 1, 1));
    }
    efude_io::save(&path, &doc).unwrap();
    let mut app = EfudeApp::default();
    app.doc = efude_io::load(&path).unwrap();
    let before = efude_canvas::composite_transparent(&app.doc);
    let state = app.history.state_token();
    app.merge_visible_layers();
    assert_eq!(app.doc.layers.len(), 2000);
    assert!(app.doc.layers.iter().all(|layer| layer.visible));
    assert_eq!(efude_canvas::composite_transparent(&app.doc), before);
    assert_eq!(app.history.state_token(), state);
    assert!(!app.history.is_active());
    assert!(!app.history.can_undo());
    assert!(!app.history.is_dirty());
    efude_io::save(&path, &app.doc).unwrap();
    assert_eq!(efude_io::load(&path).unwrap().layers.len(), 2000);
}

#[test]
fn merging_a_native_document_with_the_maximum_layer_id_is_undoable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("maximum-id.efude");
    let mut doc = Document::new(8, 8);
    let mut top = efude_canvas::Layer::new(u64::MAX, "top", 8, 8);
    doc.layers[0].pixels.set_pixel(3, 4, [200, 40, 80, 255]);
    top.pixels.set_pixel(3, 4, [20, 120, 220, 128]);
    doc.layers.push(top);
    efude_io::save(&path, &doc).unwrap();
    let mut app = EfudeApp::default();
    app.doc = efude_io::load(&path).unwrap();
    let original_ids = app
        .doc
        .layers
        .iter()
        .map(|layer| layer.id)
        .collect::<Vec<_>>();
    let before = efude_canvas::composite_transparent(&app.doc);
    app.merge_visible_layers();
    let merged_id = app.doc.layers[2].id;
    assert_ne!(merged_id, 0);
    assert!(!original_ids.contains(&merged_id));
    assert!(app.doc.layers[..2].iter().all(|layer| !layer.visible));
    let merged = efude_canvas::composite_transparent(&app.doc);
    assert!(before.iter().zip(&merged).all(|(a, b)| a.abs_diff(*b) <= 1));
    for _ in 0..3 {
        app.undo();
        assert_eq!(app.doc.layers.len(), 2);
        assert_eq!(
            app.doc
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            original_ids
        );
        assert!(app.doc.layers.iter().all(|layer| layer.visible));
        assert_eq!(efude_canvas::composite_transparent(&app.doc), before);
        assert!(!app.history.can_undo());
        app.redo();
        assert_eq!(app.doc.layers.len(), 3);
        assert_eq!(app.doc.layers[2].id, merged_id);
        assert_eq!(efude_canvas::composite_transparent(&app.doc), merged);
        assert!(!app.history.can_redo());
    }
}

#[test]
fn merging_clipping_runs_preserves_the_picture_through_repeated_undo_redo() {
    let mut app = EfudeApp::default();
    app.doc = Document::new(1, 1);
    app.doc.layers.clear();
    for (index, (pixel, clipping)) in [
        ([0, 0, 0, 128], false),
        ([0, 0, 0, 255], true),
        ([0, 0, 0, 128], false),
        ([255, 0, 0, 255], true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut layer = efude_canvas::Layer::new(index as u64 + 1, "layer", 1, 1);
        layer.clipping = clipping;
        layer.pixels.set_pixel(0, 0, pixel);
        app.doc.layers.push(layer);
    }
    let before = efude_canvas::composite(&app.doc);
    let saved_state = app.history.state_token();
    app.merge_visible_layers();
    let after = efude_canvas::composite(&app.doc);
    for (original, merged) in before.iter().zip(&after) {
        assert!(
            original.abs_diff(*merged) <= 1,
            "before={before:?}, after={after:?}"
        );
    }
    assert_eq!(app.doc.layers.len(), 5);
    assert!(app.doc.layers[..4].iter().all(|layer| !layer.visible));
    for _ in 0..3 {
        app.undo();
        assert_eq!(efude_canvas::composite(&app.doc), before);
        assert_eq!(app.doc.layers.len(), 4);
        assert!(app.doc.layers.iter().all(|layer| layer.visible));
        assert_eq!(app.history.state_token(), saved_state);
        app.redo();
        assert_eq!(efude_canvas::composite(&app.doc), after);
        assert_eq!(app.doc.layers.len(), 5);
        assert!(app.doc.layers[..4].iter().all(|layer| !layer.visible));
    }
}

fn moved_panel_contact(canvas_first: bool, batched: bool) -> (Vec<u8>, Vec<u8>, (i32, i32)) {
    let mut h = BatchedPanelHarness::new(layout::Pane::Layers);
    if canvas_first {
        h.app.workspace = layout::default_workspace();
        let layers = h.app.workspace.find_tab(&layout::Pane::Layers).unwrap();
        h.app.workspace.set_active_tab(layers);
    }
    h.app.tool = Tool::Move;
    h.app.last_tool = Tool::Move;
    h.app.doc.layers[0]
        .pixels
        .set_pixel(20, 40, [220, 30, 10, 255]);
    h.app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(120, 80));
    h.app.selection.active = true;
    h.app.selection.mask = vec![0; 120 * 80];
    h.app.selection.mask[40 * 120 + 20] = 255;
    let control = h.label_position("Invert Mask");
    let initial_pixels = h.app.doc.layers[0].pixels.to_dense();
    let initial_selection = h.app.selection.mask.clone();
    let start = h.canvas_pos(20.0, 40.0);
    h.frame(vec![
        egui::Event::PointerMoved(start),
        BatchedPanelHarness::button(start, true),
    ]);
    for x in [25.0, 35.0, 45.0, 55.0] {
        h.frame(vec![egui::Event::PointerMoved(h.canvas_pos(x, 40.0))]);
    }
    assert!(h.app.history.is_active());
    assert!(!h.app.history.can_undo());
    let origin = h.app.move_origin.as_ref().expect("Move must be active");
    let expected = (20 + 75 - origin.start.0, 40 + 40 - origin.start.1);
    let end = h.canvas_pos(75.0, 40.0);
    let mut events = vec![
        egui::Event::PointerMoved(end),
        BatchedPanelHarness::button(end, false),
    ];
    if batched {
        events.extend([
            egui::Event::PointerMoved(control),
            BatchedPanelHarness::button(control, true),
            BatchedPanelHarness::button(control, false),
        ]);
    }
    h.frame(events);
    if !batched {
        h.click(control);
    }
    assert!(!h.app.history.is_active());
    assert!(h.app.move_origin.is_none());
    let final_pixels = h.app.doc.layers[0].pixels.to_dense();
    let final_mask = h.app.doc.layers[0].mask.as_ref().unwrap().to_dense();
    let final_selection = h.app.selection.mask.clone();
    h.app.undo();
    assert!(
        h.app.history.can_undo(),
        "Move must remain a separate Undo step"
    );
    let moved_selection = h.app.selection.mask.clone();
    // Verify the same pixels reach the native file, rather than only a preview.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("moved.efude");
    efude_io::save(&path, &h.app.doc).unwrap();
    let moved_pixels = efude_io::load(&path).unwrap().layers[0].pixels.to_dense();
    h.app.undo();
    assert!(h.app.doc.layers[0].pixels.to_dense() == initial_pixels);
    assert!(h.app.selection.mask == initial_selection);
    assert!(!h.app.history.can_undo());
    h.app.redo();
    h.app.redo();
    assert!(h.app.doc.layers[0].pixels.to_dense() == final_pixels);
    assert!(h.app.doc.layers[0].mask.as_ref().unwrap().to_dense() == final_mask);
    assert!(h.app.selection.mask == final_selection);
    assert!(!h.app.history.can_redo());
    (moved_pixels, moved_selection, expected)
}

fn assert_move_panel_final_contact(canvas_first: bool) {
    let isolated = moved_panel_contact(canvas_first, false);
    let batched = moved_panel_contact(canvas_first, true);
    assert_eq!(isolated.2, batched.2);
    let index = isolated.2.1 as usize * 120 + isolated.2.0 as usize;
    assert_eq!(&isolated.0[index * 4..index * 4 + 4], &[220, 30, 10, 255]);
    assert_eq!(isolated.1[index], 255);
    assert!(
        isolated.0 == batched.0 && isolated.1 == batched.1,
        "batched Move release changed saved pixels or selection: canvas_first={canvas_first}, expected={:?}, batch_pixel={:?}, batch_selection={}",
        isolated.2,
        &batched.0[index * 4..index * 4 + 4],
        batched.1[index]
    );
}

#[test]
fn panel_move_final_contact_layers_first() {
    assert_move_panel_final_contact(false);
}

#[test]
fn panel_move_final_contact_canvas_first() {
    assert_move_panel_final_contact(true);
}

fn selection_panel_contact(tool: Tool, canvas_first: bool, batched: bool) -> Vec<u8> {
    let mut h = BatchedPanelHarness::new(layout::Pane::Layers);
    if canvas_first {
        h.app.workspace = layout::default_workspace();
        let layers = h.app.workspace.find_tab(&layout::Pane::Layers).unwrap();
        h.app.workspace.set_active_tab(layers);
    }
    h.app.tool = tool;
    h.app.last_tool = tool;
    h.app.doc.layers[0]
        .pixels
        .set_pixel(20, 40, [220, 30, 10, 255]);
    h.app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(120, 80));
    h.app.selection.active = false;
    h.app.selection.mask = vec![0; 120 * 80];
    let control = h.label_position("Invert Mask");
    let initial_pixels = h.app.doc.layers[0].pixels.to_dense();
    let initial_selection = h.app.selection.mask.clone();
    let start = h.canvas_pos(20.0, 20.0);
    h.frame(vec![
        egui::Event::PointerMoved(start),
        BatchedPanelHarness::button(start, true),
    ]);
    for (x, y) in [(25.0, 25.0), (35.0, 30.0), (45.0, 35.0), (55.0, 40.0)] {
        h.frame(vec![egui::Event::PointerMoved(h.canvas_pos(x, y))]);
    }
    assert!(h.app.selection_start.is_some());
    assert!(!h.app.history.can_undo());
    let end = h.canvas_pos(75.0, 40.0);
    let mut events = vec![
        egui::Event::PointerMoved(end),
        BatchedPanelHarness::button(end, false),
    ];
    if batched {
        events.extend([
            egui::Event::PointerMoved(control),
            BatchedPanelHarness::button(control, true),
            BatchedPanelHarness::button(control, false),
        ]);
    }
    h.frame(events);
    if !batched {
        h.click(control);
    }
    let mask_value = |app: &EfudeApp| {
        app.doc.layers[0]
            .mask
            .as_ref()
            .unwrap()
            .pixel_or_tile_default(0, 0, [255; 4])[0]
    };
    assert!(!h.app.history.is_active());
    assert!(h.app.selection_start.is_none());
    assert!(h.app.selection.active);
    let final_selection = h.app.selection.mask.clone();
    assert_eq!(mask_value(&h.app), 0, "Invert Mask must actually execute");
    assert!(h.app.doc.layers[0].pixels.to_dense() == initial_pixels);
    h.app.undo();
    assert_eq!(mask_value(&h.app), 255);
    assert!(h.app.selection.active);
    assert!(h.app.selection.mask == final_selection);
    assert!(
        h.app.history.can_undo(),
        "selection creation must be a separate Undo step"
    );
    h.app.undo();
    assert!(!h.app.selection.active);
    assert!(h.app.selection.mask == initial_selection);
    assert!(h.app.doc.layers[0].pixels.to_dense() == initial_pixels);
    assert!(!h.app.history.can_undo());
    h.app.redo();
    assert!(h.app.selection.active);
    assert!(h.app.selection.mask == final_selection);
    assert_eq!(mask_value(&h.app), 255);
    h.app.redo();
    assert_eq!(mask_value(&h.app), 0);
    assert!(h.app.selection.mask == final_selection);
    assert!(h.app.doc.layers[0].pixels.to_dense() == initial_pixels);
    assert!(!h.app.history.can_redo());
    final_selection
}

fn assert_selection_panel_final_contact(tool: Tool, canvas_first: bool) {
    let isolated = selection_panel_contact(tool, canvas_first, false);
    let batched = selection_panel_contact(tool, canvas_first, true);
    let index = 30 * 120 + 70;
    assert_eq!(isolated[index], 255);
    assert!(isolated.iter().any(|&value| value != 0));
    assert!(
        isolated == batched,
        "batched selection release changed the selection: tool={tool:?}, canvas_first={canvas_first}, isolated_at_70_30={}, batch_at_70_30={}, isolated_count={}, batch_count={}",
        isolated[index],
        batched[index],
        isolated.iter().filter(|&&value| value != 0).count(),
        batched.iter().filter(|&&value| value != 0).count()
    );
}

#[test]
fn panel_rectangle_final_contact_layers_first() {
    assert_selection_panel_final_contact(Tool::RectangleSelect, false);
}

#[test]
fn panel_rectangle_final_contact_canvas_first() {
    assert_selection_panel_final_contact(Tool::RectangleSelect, true);
}

#[test]
fn panel_ellipse_final_contact_layers_first() {
    assert_selection_panel_final_contact(Tool::EllipseSelect, false);
}

#[test]
fn panel_ellipse_final_contact_canvas_first() {
    assert_selection_panel_final_contact(Tool::EllipseSelect, true);
}
