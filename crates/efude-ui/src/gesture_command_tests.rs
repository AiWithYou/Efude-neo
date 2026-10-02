// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Command boundaries exercised with real canvas pointer and keyboard events.

use super::*;

#[test]
fn oversized_brush_mask_paint_is_bounded_by_the_document_and_undoable() {
    for (size, dynamic_size, aspect) in
        [(3.0e38, 1.0, 1.0), (2000.0, 4.0, 10.0), (3.0e38, 4.0, 10.0)]
    {
        let mut app = EfudeApp::default();
        app.doc = Document::new(4, 4);
        app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(4, 4));
        app.doc.layers[0].pixels.fill_shared([120, 40, 200, 255]);
        app.size = size;
        app.dynamic_size = dynamic_size;
        app.brushes[app.selected_brush].tip_aspect = aspect;
        app.color = Color32::BLACK;
        let pixels = app.doc.layers[0].pixels.to_dense();
        app.history.begin();
        app.dab_mask(InkPoint::new(1.0, 1.0, 1.0, 0));
        app.history.commit();
        let coverage = |app: &EfudeApp| {
            (0..4)
                .flat_map(|y| {
                    (0..4).map(move |x| {
                        app.doc.layers[0]
                            .mask
                            .as_ref()
                            .unwrap()
                            .pixel_or_tile_default(x, y, [255; 4])[0]
                    })
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(coverage(&app), vec![0; 16]);
        assert_eq!(app.doc.layers[0].pixels.to_dense(), pixels);
        for _ in 0..3 {
            app.undo();
            assert_eq!(coverage(&app), vec![255; 16]);
            app.redo();
            assert_eq!(coverage(&app), vec![0; 16]);
            assert_eq!(app.doc.layers[0].pixels.to_dense(), pixels);
        }
    }
}

#[test]
fn oversized_brush_selection_paint_is_bounded_by_the_document_and_undoable() {
    for (size, dynamic_size, aspect) in
        [(3.0e38, 1.0, 1.0), (2000.0, 4.0, 10.0), (3.0e38, 4.0, 10.0)]
    {
        let mut app = EfudeApp::default();
        app.doc = Document::new(4, 4);
        app.tool = Tool::SelectionBrush;
        app.size = size;
        app.dynamic_size = dynamic_size;
        app.brushes[app.selected_brush].tip_aspect = aspect;
        app.begin_selection_operation(egui::Modifiers::NONE);
        app.selection_dab(InkPoint::new(1.0, 1.0, 1.0, 0));
        app.finish_selection_operation();
        assert!(app.selection.active);
        assert_eq!(app.selection.mask, vec![255; 16]);
        for _ in 0..3 {
            app.undo();
            assert!(!app.selection.active);
            assert!(app.selection.mask.is_empty());
            app.redo();
            assert!(app.selection.active);
            assert_eq!(app.selection.mask, vec![255; 16]);
        }
    }
}

struct Harness {
    app: EfudeApp,
    ctx: egui::Context,
    time: f64,
    pointer: Pos2,
    navigator_upload: Vec<Color32>,
}

impl Harness {
    fn new(tool: Tool) -> Self {
        let mut app = EfudeApp::default();
        app.doc = Document::new(120, 80);
        app.navigator_center = Vec2::new(60.0, 40.0);
        app.use_windows_ink = false;
        app.use_wintab = false;
        app.selected_layer = 0;
        app.color = Color32::BLACK;
        app.tool = tool;
        app.size = 6.0;
        let ctx = egui::Context::default();
        layout::apply_theme(&ctx);
        let mut h = Self {
            app,
            ctx,
            time: 0.0,
            pointer: Pos2::ZERO,
            navigator_upload: Vec::new(),
        };
        for _ in 0..3 {
            h.frame(Vec::new());
        }
        h
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.frame_focused(events, true);
    }

    fn frame_focused(&mut self, events: Vec<egui::Event>, focused: bool) {
        self.time += 1.0 / 60.0;
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
            time: Some(self.time),
            events,
            focused,
            ..Default::default()
        };
        eframe::App::raw_input_hook(&mut self.app, &self.ctx, &mut input);
        let _ = self.ctx.run(input, |ctx| self.app.update_ui(ctx));
    }

    fn move_to(&mut self, x: f32, y: f32) {
        let (rect, scale) = self.app.canvas_screen.expect("canvas drawn");
        self.pointer = rect.center() + Vec2::new(x - 60.0, y - 40.0) * scale;
        self.frame(vec![egui::Event::PointerMoved(self.pointer)]);
    }

    fn button(&mut self, pressed: bool) {
        self.frame(vec![egui::Event::PointerButton {
            pos: self.pointer,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }

    fn command(&mut self, key: egui::Key, shift: bool) {
        let modifiers = if shift {
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::COMMAND
        };
        self.frame(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }]);
        self.frame(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers,
        }]);
    }

    fn begin_gesture(&mut self) {
        self.move_to(20.0, 40.0);
        self.button(true);
        for x in [25.0, 35.0, 45.0, 55.0] {
            self.move_to(x, 40.0);
        }
        assert!(self.app.history.is_active());
    }

    fn release_farther(&mut self) {
        self.move_to(95.0, 40.0);
        self.button(false);
        self.frame(Vec::new());
    }

    fn navigator_pixels(&mut self) -> Vec<Color32> {
        // Let the navigator's test window finish its initial layout.
        for _ in 0..3 {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
                time: Some(self.time),
                ..Default::default()
            };
            let output = self.ctx.run(input, |ctx| {
                self.app.update_ui(ctx);
                egui::Window::new("Navigator test")
                    .default_pos(Pos2::new(1100.0, 600.0))
                    .default_width(160.0)
                    .show(ctx, |ui| {
                        ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
                        self.app.navigator_ui(ui, ctx);
                        ctx.memory_mut(|memory| memory.set_everything_is_visible(false));
                    });
            });
            let Some(texture) = self.app.navigator_texture.as_ref() else {
                continue;
            };
            let id = texture.id();
            for (updated, delta) in output.textures_delta.set {
                if updated == id {
                    assert!(delta.pos.is_none());
                    let egui::ImageData::Color(image) = delta.image else {
                        panic!("navigator must upload a color image");
                    };
                    self.navigator_upload = image.pixels.clone();
                }
            }
        }
        assert!(!self.navigator_upload.is_empty(), "navigator was not drawn");
        self.navigator_upload.clone()
    }

    fn expected_navigator_pixels(&self) -> Vec<Color32> {
        let pixels = efude_canvas::composite_display(&self.app.doc, self.app.display_checker());
        egui::ColorImage::from_rgba_unmultiplied([120, 80], &pixels).pixels
    }
}

#[test]
fn focus_loss_releases_a_borrowed_tool_without_a_key_up_event() {
    let mut h = Harness::new(Tool::Brush);
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(h.app.tool, Tool::Pan);
    h.frame_focused(vec![egui::Event::WindowFocused(false)], false);
    h.frame(vec![egui::Event::WindowFocused(true)]);
    assert_eq!(h.app.tool, Tool::Brush);
    assert!(h.app.held_tool_key.is_none());
    assert!(!h.ctx.input(|input| input.key_down(egui::Key::Space)));
}

#[test]
fn focus_loss_finishes_a_stroke_and_ignores_the_old_held_pointer() {
    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    h.frame_focused(vec![egui::Event::WindowFocused(false)], false);
    assert!(!h.app.history.is_active());
    assert!(h.app.active.is_empty());
    let finished = h.app.doc.layers[0].pixels.to_dense();
    h.frame(vec![egui::Event::WindowFocused(true)]);
    h.release_farther();
    assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
    h.command(egui::Key::Z, false);
    assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
    h.command(egui::Key::Z, true);
    assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
    h.begin_gesture();
    h.release_farther();
    assert!(h.app.doc.layers[0].pixels.pixel(80, 40)[3] > 0);
}

fn check_zoom_shortcut_stroke_boundary(key: egui::Key) {
    let mut h = Harness::new(Tool::Brush);
    let mut released = Harness::new(Tool::Brush);
    for harness in [&mut h, &mut released] {
        harness.app.brushes[harness.app.selected_brush].settle = false;
        harness.app.zoom = 2.0;
        harness.app.navigator_center = Vec2::new(48.0, 32.0);
        harness.frame(Vec::new());
        harness.move_to(20.0, 20.0);
        harness.button(true);
        for x in [25.0, 30.0, 35.0] {
            harness.move_to(x, 20.0);
        }
        assert!(harness.app.history.is_active());
    }
    released.button(false);
    let finished = released.app.doc.layers[0].pixels.to_dense();
    released.command(key, false);
    let live_navigator = h.navigator_pixels();
    assert!(!h.app.navigator_texture_dirty);
    let previous_point = h.app.active.last().map(|p| p.position);
    h.command(key, false);
    assert!(
        h.app.active.is_empty(),
        "{key:?} kept the stroke active: {previous_point:?} -> {:?}",
        h.app.active.last().map(|p| p.position)
    );
    assert!(!h.app.history.is_active());
    assert_eq!(h.app.zoom, released.app.zoom);
    assert_eq!(h.app.navigator_center, released.app.navigator_center);
    h.release_farther();
    assert!(
        h.app.doc.layers[0].pixels.to_dense() == finished,
        "{key:?} must produce the same stroke as releasing before zooming"
    );
    let final_navigator = h.expected_navigator_pixels();
    assert!(
        live_navigator != final_navigator,
        "finishing the held stroke must add the pending tail"
    );
    assert!(
        h.navigator_pixels() == final_navigator,
        "{key:?} must update the navigator with the finished stroke"
    );
    h.command(egui::Key::Z, false);
    assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
    h.command(egui::Key::Z, true);
    assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
    h.begin_gesture();
    h.release_farther();
    assert!(h.app.doc.layers[0].pixels.pixel(80, 40)[3] > 0);
    h.command(egui::Key::Z, false);
    assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
}

#[test]
fn zoom_shortcuts_plus_finishes_a_stroke_before_zooming() {
    check_zoom_shortcut_stroke_boundary(egui::Key::Plus);
}

#[test]
fn zoom_shortcuts_minus_finishes_a_stroke_before_zooming() {
    check_zoom_shortcut_stroke_boundary(egui::Key::Minus);
}

#[test]
fn zoom_shortcuts_fit_finishes_a_stroke_before_zooming() {
    check_zoom_shortcut_stroke_boundary(egui::Key::Num0);
}

#[test]
fn focus_loss_updates_the_navigator_with_the_finished_stroke() {
    let mut h = Harness::new(Tool::Brush);
    h.app.brushes[h.app.selected_brush].settle = false;
    h.begin_gesture();
    let live_navigator = h.navigator_pixels();
    assert!(!h.app.navigator_texture_dirty);
    h.frame_focused(vec![egui::Event::WindowFocused(false)], false);
    assert!(h.app.active.is_empty());
    assert!(!h.app.history.is_active());
    let final_navigator = h.expected_navigator_pixels();
    assert!(live_navigator != final_navigator);
    assert!(
        h.navigator_pixels() == final_navigator,
        "focus loss must update the navigator with the finished stroke"
    );
}

#[test]
fn rasterizing_during_a_vector_stroke_keeps_two_independent_undo_steps() {
    let mut h = Harness::new(Tool::Brush);
    h.app.add_vector_layer();
    h.begin_gesture();
    let index = h.app.selected_layer;
    h.app.rasterize_layer(index);
    assert!(h.app.active.is_empty());
    assert!(!h.app.doc.layers[index].is_vector());
    let rasterized = h.app.doc.layers[index].pixels.to_dense();
    h.release_farther();
    assert_eq!(h.app.doc.layers[index].pixels.to_dense(), rasterized);
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert!(h.app.doc.layers[index].is_vector());
        assert!(!h.app.doc.layers[index].vector.as_ref().unwrap().is_empty());
        assert_eq!(h.app.doc.layers[index].pixels.to_dense(), rasterized);
        h.command(egui::Key::Z, false);
        assert!(h.app.doc.layers[index].vector.as_ref().unwrap().is_empty());
        assert!(!h.app.doc.layers[index].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers[index].pixels.to_dense(), rasterized);
        h.command(egui::Key::Z, true);
        assert!(!h.app.doc.layers[index].is_vector());
        assert_eq!(h.app.doc.layers[index].pixels.to_dense(), rasterized);
    }
}

#[test]
fn guide_import_completion_during_a_stroke_keeps_independent_undo_steps() {
    let mut h = Harness::new(Tool::Brush);
    let mut released = Harness::new(Tool::Brush);
    let original = efude_canvas::GuideImage::fit_to_canvas(
        2,
        1,
        vec![20, 40, 60, 255, 80, 100, 120, 255],
        &h.app.doc,
    )
    .unwrap();
    for harness in [&mut h, &mut released] {
        harness.app.doc.guide = Some(original.clone());
        harness.begin_gesture();
    }
    released.button(false);
    let stroke = released.app.doc.layers[0].pixels.to_dense();

    let (sender, receiver) = std::sync::mpsc::channel();
    h.app.io_receiver = receiver;
    sender
        .send(IoCompletion::GuideLoaded {
            document_id: h.app.history.document_id(),
            path: "new-guide.png".into(),
            width: 2,
            height: 1,
            rgba: vec![200, 160, 120, 255, 80, 40, 0, 255],
        })
        .unwrap();
    h.frame(Vec::new());
    assert!(!h.app.history.is_active());
    assert!(h.app.active.is_empty());
    let imported = h.app.doc.guide.clone().unwrap();
    assert!(imported != original);
    assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
    h.release_farther();
    assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);

    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert!(h.app.doc.guide.as_ref() == Some(&original));
        assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.command(egui::Key::Z, true);
        assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
        assert!(h.app.doc.guide.as_ref() == Some(&original));
        h.command(egui::Key::Z, true);
        assert!(h.app.doc.guide.as_ref() == Some(&imported));
        assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
        assert!(!h.app.history.can_redo());
    }
}

fn assert_layer_replacement_finishes_stroke(command: fn(&mut EfudeApp)) {
    let mut baseline = Harness::new(Tool::Brush);
    baseline.begin_gesture();
    baseline.button(false);
    let finished = baseline.app.doc.layers[0].pixels.to_dense();

    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    command(&mut h.app);
    assert!(h.app.active.is_empty());
    assert!(!h.app.history.is_active());
    let after = efude_canvas::composite_transparent(&h.app.doc);
    let after_ids = h
        .app
        .doc
        .layers
        .iter()
        .map(|layer| layer.id)
        .collect::<Vec<_>>();
    h.release_farther();
    assert_eq!(efude_canvas::composite_transparent(&h.app.doc), after);
    for _ in 0..3 {
        h.app.undo();
        assert_eq!(h.app.doc.layers.len(), 1);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
        h.app.undo();
        assert_eq!(h.app.doc.layers.len(), 1);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.app.redo();
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
        h.app.redo();
        assert_eq!(efude_canvas::composite_transparent(&h.app.doc), after);
        assert_eq!(
            h.app
                .doc
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            after_ids
        );
        assert!(!h.app.history.can_redo());
    }
}

#[test]
fn merging_visible_layers_during_a_stroke_keeps_independent_undo_steps() {
    assert_layer_replacement_finishes_stroke(EfudeApp::merge_visible_layers);
}

#[test]
fn clearing_all_layers_during_a_stroke_keeps_independent_undo_steps() {
    assert_layer_replacement_finishes_stroke(EfudeApp::clear_all_layers);
}

#[test]
fn transforming_during_a_stroke_keeps_independent_undo_steps() {
    assert_layer_replacement_finishes_stroke(|app| app.apply_transform(1.2, 0.8, 0.2));
}

#[test]
fn mesh_warp_during_a_stroke_keeps_independent_undo_steps() {
    assert_layer_replacement_finishes_stroke(|app| {
        app.mesh_offsets.fill([8.0, 0.0]);
        app.apply_mesh_warp();
    });
}

#[test]
fn selection_brushes_after_tablet_input_accept_window_input() {
    for tool in [Tool::SelectionBrush, Tool::QuickMask] {
        let mut h = Harness::new(tool);
        h.app.input_diagnostics.tablet_samples = 1;
        h.app.tablet_seen = true;
        h.move_to(20.0, 40.0);
        h.button(true);
        for x in [25.0, 35.0, 45.0, 55.0] {
            h.move_to(x, 40.0);
        }
        h.button(false);
        assert!(h.app.selection.active, "{tool:?}");
        assert!(h.app.selection.mask.iter().any(|value| *value > 0));
        h.app.undo();
        assert!(!h.app.selection.active);
        h.app.redo();
        assert!(h.app.selection.mask.iter().any(|value| *value > 0));
    }
}

#[test]
fn a_short_window_stroke_after_tablet_input_is_not_discarded() {
    let mut h = Harness::new(Tool::Brush);
    h.app.use_windows_ink = true;
    h.app.tablet_seen = true;
    h.move_to(20.0, 40.0);
    h.button(true);
    h.move_to(55.0, 40.0);
    h.button(false);
    assert!(h.app.doc.layers[0].pixels.has_allocated_tiles());
    let painted = h.app.doc.layers[0].pixels.to_dense();
    h.app.undo();
    assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
    h.app.redo();
    assert_eq!(h.app.doc.layers[0].pixels.to_dense(), painted);
}

#[test]
fn short_window_selection_strokes_after_tablet_input_keep_press_origin() {
    for tool in [Tool::SelectionBrush, Tool::QuickMask] {
        let mut h = Harness::new(tool);
        h.app.use_windows_ink = true;
        h.app.tablet_seen = true;
        h.move_to(20.0, 40.0);
        h.button(true);
        h.move_to(55.0, 40.0);
        h.button(false);
        assert!(h.app.selection.active, "{tool:?}");
        for x in [20usize, 55] {
            assert!(h.app.selection.mask[40 * 120 + x] > 0, "{tool:?}, x={x}");
        }
        let selected = h.app.selection.mask.clone();
        h.app.undo();
        assert!(!h.app.selection.active);
        h.app.redo();
        assert_eq!(h.app.selection.mask, selected);
    }
}

#[test]
fn selection_brush_erase_removes_coverage_and_is_undoable() {
    for tool in [Tool::SelectionBrush, Tool::QuickMask] {
        let mut h = Harness::new(tool);
        h.app.selection.rectangle(120, 80, (0, 0), (119, 79));
        h.app.selection.mask.fill(200);
        h.app.selection_erase = true;
        let before = h.app.selection.mask.clone();
        h.move_to(20.0, 40.0);
        h.button(true);
        h.button(false);
        assert!(h.app.selection.active, "{tool:?}");
        let erased = h.app.selection.mask.clone();
        assert!(erased[40 * 120 + 20] < 200, "{tool:?}");
        assert_eq!(erased[40 * 120 + 100], 200);
        for _ in 0..3 {
            h.app.undo();
            assert_eq!(h.app.selection.mask, before);
            assert!(!h.app.history.can_undo());
            h.app.redo();
            assert_eq!(h.app.selection.mask, erased);
            assert!(!h.app.history.can_redo());
        }
    }
}

#[test]
fn panel_split_gesture_keeps_the_layer_capacity_error() {
    let mut app = EfudeApp::default();
    app.doc = Document::new(128, 96);
    let comic = comic::ComicDoc {
        page: efude_comic::PageSpec::presets()[0].2.clone(),
        layout: efude_comic::PanelLayout::for_dpi(72.0),
    };
    app.doc
        .metadata
        .insert("comic".into(), serde_json::to_string(&comic).unwrap());
    app.create_panel(Some([
        glam::Vec2::new(16.0, 16.0),
        glam::Vec2::new(112.0, 80.0),
    ]));
    while app.doc.layers.len() < 1998 {
        let id = app.doc.layers.len() as u64 + 100;
        app.doc
            .layers
            .push(efude_canvas::Layer::new(id, "capacity", 128, 96));
    }
    app.tool = Tool::PanelSplit;
    app.comic_ui.split_start = Some(Vec2::new(64.0, 8.0));
    app.comic_ui.split_end = Some(Vec2::new(64.0, 88.0));
    app.finish_canvas_gesture(false);
    assert_eq!(app.doc.layers.len(), 1998);
    assert!(app.status.contains("2000"), "{}", app.status);
}

#[test]
fn resizing_during_a_stroke_keeps_the_stroke_and_resize_undoable() {
    let mut baseline = Harness::new(Tool::Brush);
    baseline.begin_gesture();
    baseline.button(false);
    let finished = baseline.app.doc.layers[0].pixels.to_dense();
    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    h.app.canvas_width_input = 90;
    h.app.canvas_height_input = 60;
    h.app.canvas_dpi_input = 600.0;
    h.app.apply_canvas_size();
    assert!(h.app.active.is_empty());
    assert!(!h.app.history.is_active());
    assert_eq!(
        (h.app.doc.width, h.app.doc.height, h.app.doc.dpi),
        (90, 60, 600.0)
    );
    let resized = h.app.doc.layers[0].pixels.to_dense();
    h.release_farther();
    assert_eq!(h.app.doc.layers[0].pixels.to_dense(), resized);
    for _ in 0..3 {
        h.app.undo();
        assert_eq!(
            (h.app.doc.width, h.app.doc.height, h.app.doc.dpi),
            (120, 80, 300.0)
        );
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
        h.app.undo();
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.app.redo();
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
        h.app.redo();
        assert_eq!(
            (h.app.doc.width, h.app.doc.height, h.app.doc.dpi),
            (90, 60, 600.0)
        );
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), resized);
    }
}

#[test]
fn unchanged_or_invalid_canvas_size_during_a_stroke_keeps_its_undo() {
    for width in [120, efude_canvas::MAX_DOCUMENT_DIMENSION + 1] {
        let mut baseline = Harness::new(Tool::Brush);
        baseline.begin_gesture();
        baseline.button(false);
        let finished = baseline.app.doc.layers[0].pixels.to_dense();
        let mut h = Harness::new(Tool::Brush);
        h.begin_gesture();
        h.app.canvas_width_input = width;
        h.app.canvas_height_input = 80;
        h.app.apply_canvas_size();
        assert!(h.app.active.is_empty());
        assert!(!h.app.history.is_active());
        assert_eq!((h.app.doc.width, h.app.doc.height), (120, 80));
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
        h.release_farther();
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
        h.app.undo();
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.app.redo();
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
    }
}

#[test]
fn starting_a_paste_during_a_stroke_keeps_the_stroke_and_paste_undoable() {
    let mut h = Harness::new(Tool::Brush);
    let mut released = Harness::new(Tool::Brush);
    h.begin_gesture();
    released.begin_gesture();
    released.button(false);
    let stroke = released.app.doc.layers[0].pixels.to_dense();
    assert!(!h.app.selection.active);

    // Start with clipboard pixels already acquired: this avoids modifying
    // the system clipboard and exercises the same preview setup as Ctrl+V.
    let ctx = h.ctx.clone();
    h.app
        .start_paste_preview(&ctx, 4, 4, [80, 120, 200, 128].repeat(16));
    assert!(h.app.paste_preview.is_some());
    assert!(!h.app.history.is_active());
    assert!(h.app.active.is_empty());
    assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
    h.release_farther();
    assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
    assert!(h.app.paste_preview.is_some());

    h.move_to(70.0, 60.0);
    h.button(true);
    h.move_to(90.0, 60.0);
    h.button(false);
    assert!(h.app.paste_preview.is_some());
    h.button(true);
    h.button(false);
    assert!(h.app.paste_preview.is_none());
    assert_eq!(
        h.app.doc.layers[0].pixels.pixel(90, 60),
        [80, 120, 200, 128]
    );
    let pasted = h.app.doc.layers[0].pixels.to_dense();
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.command(egui::Key::Z, true);
        assert!(h.app.doc.layers[0].pixels.to_dense() == stroke);
        h.command(egui::Key::Z, true);
        assert!(h.app.doc.layers[0].pixels.to_dense() == pasted);
        assert!(!h.app.history.can_redo());
    }
}

#[test]
fn placing_a_paste_does_not_also_run_the_selected_click_tool() {
    for tool in [
        Tool::MagicWand,
        Tool::ColorRange,
        Tool::PolygonSelect,
        Tool::BezierRuler,
        Tool::PerspectiveRuler,
    ] {
        let mut h = Harness::new(tool);
        if tool == Tool::PerspectiveRuler {
            h.app.setting_vanishing_point = true;
        }
        let ctx = h.ctx.clone();
        h.app
            .start_paste_preview(&ctx, 4, 4, [80, 120, 200, 128].repeat(16));
        h.move_to(60.0, 40.0);
        h.button(true);
        h.button(false);
        assert!(h.app.paste_preview.is_none(), "{tool:?}");
        assert!(!h.app.selection.active, "{tool:?} changed the selection");
        assert!(
            h.app.selection_points.is_empty(),
            "{tool:?} started a polygon"
        );
        assert!(h.app.bezier_points.is_empty(), "{tool:?} started a curve");
        assert!(
            h.app.perspective_points.is_empty(),
            "{tool:?} set a vanishing point"
        );
        assert!(
            !h.app.history.is_active(),
            "{tool:?} started another action"
        );
        let pasted = h.app.doc.layers[0].pixels.to_dense();
        assert!(h.app.doc.layers[0].pixels.pixel(60, 40)[3] > 0);
        h.app.undo();
        assert!(
            !h.app.doc.layers[0].pixels.has_allocated_tiles(),
            "{tool:?}"
        );
        assert!(!h.app.history.can_undo(), "{tool:?} created a second Undo");
        h.app.redo();
        assert!(h.app.doc.layers[0].pixels.to_dense() == pasted);
    }
}

#[test]
fn selecting_a_layer_during_a_stroke_keeps_the_stroke_on_its_original_layer() {
    let mut h = Harness::new(Tool::Brush);
    h.app.add_raster_layer();
    h.app.select_layer(0);
    h.begin_gesture();
    h.app.select_layer(1);
    assert!(h.app.active.is_empty());
    let finished = h.app.doc.layers[0].pixels.to_dense();
    h.release_farther();
    assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
    assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
    h.command(egui::Key::Z, false);
    assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
    h.command(egui::Key::Z, true);
    assert_eq!(h.app.doc.layers[0].pixels.to_dense(), finished);
    assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
}

#[test]
fn new_layer_shortcut_during_a_brush_stroke_does_not_paint_the_new_layer() {
    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    h.command(egui::Key::N, true);
    assert_eq!(h.app.doc.layers.len(), 2);
    assert_eq!(h.app.selected_layer, 1);
    h.release_farther();
    assert!(h.app.doc.layers[0].pixels.pixel(35, 40)[3] > 0);
    assert!(
        !h.app.doc.layers[1].pixels.has_allocated_tiles(),
        "the held pointer must not paint the new layer"
    );
    let stroke = h.app.doc.layers[0].pixels.to_dense();
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 1);
        assert_eq!(
            h.app.doc.layers[0].pixels.to_dense(),
            stroke,
            "undoing layer creation must keep the completed stroke"
        );
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), stroke);
        assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
    }
}

#[test]
fn duplicate_shortcut_during_a_brush_stroke_copies_the_finished_stroke() {
    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    h.command(egui::Key::J, false);
    assert_eq!(h.app.doc.layers.len(), 2);
    h.release_farther();
    let source = h.app.doc.layers[0].pixels.to_dense();
    assert!(h.app.doc.layers[0].pixels.pixel(35, 40)[3] > 0);
    assert_eq!(
        h.app.doc.layers[1].pixels.to_dense(),
        source,
        "duplicate must capture the settled source without continuing the old gesture"
    );
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 1);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), source);
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), source);
        assert_eq!(h.app.doc.layers[1].pixels.to_dense(), source);
    }
}

#[test]
fn new_layer_shortcut_during_a_ruler_gesture_keeps_the_line_on_its_source() {
    let mut h = Harness::new(Tool::Line);
    h.begin_gesture();
    h.command(egui::Key::N, true);
    h.release_farther();
    assert_eq!(h.app.doc.layers.len(), 2);
    assert!(h.app.doc.layers[0].pixels.pixel(35, 40)[3] > 0);
    assert_eq!(h.app.doc.layers[0].pixels.pixel(95, 40)[3], 0);
    assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
    let line = h.app.doc.layers[0].pixels.to_dense();
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 1);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), line);
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), line);
        assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
    }
}

#[test]
fn delete_during_a_brush_stroke_does_not_restore_deleted_paint_on_release() {
    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Delete,
        physical_key: Some(egui::Key::Delete),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.release_farther();
    assert!(
        h.app.doc.layers[0]
            .pixels
            .to_dense()
            .iter()
            .all(|byte| *byte == 0),
        "Delete must remain effective after the held pointer is released"
    );
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert!(
            h.app.doc.layers[0].pixels.pixel(35, 40)[3] > 0,
            "Undo Delete restores the completed stroke"
        );
        assert_eq!(h.app.doc.layers[0].pixels.pixel(95, 40)[3], 0);
        let stroke = h.app.doc.layers[0].pixels.to_dense();
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), stroke);
        h.command(egui::Key::Z, true);
        assert!(
            h.app.doc.layers[0]
                .pixels
                .to_dense()
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}

#[test]
fn cut_during_a_brush_stroke_does_not_restore_cut_paint_on_release() {
    let mut h = Harness::new(Tool::Brush);
    h.begin_gesture();
    h.command(egui::Key::X, false);
    h.release_farther();
    assert!(
        h.app.doc.layers[0]
            .pixels
            .to_dense()
            .iter()
            .all(|byte| *byte == 0),
        "Cut must remain effective after the held pointer is released"
    );
    let (width, height, copied) = h.app.clipboard.clone().expect("cut pixels");
    assert_eq!((width, height), (120, 80));
    assert!(copied[(40 * 120 + 35) * 4 + 3] > 0);
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(
            h.app.doc.layers[0].pixels.to_dense(),
            copied,
            "Undo Cut restores exactly the completed clipboard stroke"
        );
        h.command(egui::Key::Z, false);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers[0].pixels.to_dense(), copied);
        h.command(egui::Key::Z, true);
        assert!(
            h.app.doc.layers[0]
                .pixels
                .to_dense()
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}

#[test]
fn duplicate_shortcut_during_a_vector_brush_stroke_copies_committed_vectors() {
    let mut h = Harness::new(Tool::Brush);
    h.app.doc.layers[0].vector = Some(Vec::new());
    h.begin_gesture();
    h.command(egui::Key::J, false);
    h.release_farther();
    assert_eq!(h.app.doc.layers.len(), 2);
    let source = h.app.doc.layers[0].clone();
    assert_eq!(source.vector.as_ref().unwrap().len(), 1);
    assert_eq!(h.app.doc.layers[1].vector, source.vector);
    assert_eq!(
        h.app.doc.layers[1].pixels.to_dense(),
        source.pixels.to_dense()
    );
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 1);
        assert_eq!(h.app.doc.layers[0].vector, source.vector);
        h.command(egui::Key::Z, false);
        assert!(h.app.doc.layers[0].vector.as_ref().unwrap().is_empty());
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(h.app.doc.layers[0].vector, source.vector);
        assert_eq!(h.app.doc.layers[1].vector, source.vector);
        assert_eq!(
            h.app.doc.layers[0].pixels.to_dense(),
            source.pixels.to_dense()
        );
        assert_eq!(
            h.app.doc.layers[1].pixels.to_dense(),
            source.pixels.to_dense()
        );
    }
}

#[test]
fn new_layer_shortcut_during_a_mask_stroke_keeps_the_stroke_in_the_mask() {
    let mut h = Harness::new(Tool::Brush);
    h.app.doc.layers[0].mask = Some(efude_canvas::TilePixels::new(120, 80));
    h.app.editing_mask = true;
    h.begin_gesture();
    h.command(egui::Key::N, true);
    h.release_farther();
    assert_eq!(h.app.doc.layers.len(), 2);
    assert!(!h.app.editing_mask);
    let mask = h.app.doc.layers[0].mask.clone().unwrap();
    assert!(mask.pixel(35, 40)[0] < 255);
    assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
    assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 1);
        assert_eq!(
            h.app.doc.layers[0].mask.as_ref().unwrap().to_dense(),
            mask.to_dense()
        );
        h.command(egui::Key::Z, false);
        assert!(
            !h.app.doc.layers[0]
                .mask
                .as_ref()
                .unwrap()
                .has_allocated_tiles()
        );
        h.command(egui::Key::Z, true);
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(
            h.app.doc.layers[0].mask.as_ref().unwrap().to_dense(),
            mask.to_dense()
        );
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
    }
}

#[test]
fn releasing_a_borrowed_selection_tool_key_before_pen_up_finishes_selection() {
    let mut h = Harness::new(Tool::Brush);
    let key_event = |pressed| egui::Event::Key {
        key: egui::Key::Num2,
        physical_key: Some(egui::Key::Num2),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    h.frame(vec![key_event(true)]);
    assert_eq!(h.app.tool, Tool::RectangleSelect);
    h.move_to(20.0, 20.0);
    h.button(true);
    h.move_to(55.0, 55.0);
    h.frame(vec![key_event(false)]);
    assert_eq!(
        h.app.tool,
        Tool::RectangleSelect,
        "borrowed tool stays active while dragging"
    );
    h.button(false);
    h.frame(Vec::new());
    assert_eq!(h.app.tool, Tool::Brush);
    assert!(h.app.selection.active);
    assert!(h.app.selection.mask[40 * 120 + 35] > 0);
    assert!(
        h.app.history.can_undo(),
        "the completed selection must have an Undo step"
    );
    let selected = h.app.selection.mask.clone();
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert!(!h.app.selection.active);
        h.command(egui::Key::Z, true);
        assert!(h.app.selection.active);
        assert_eq!(h.app.selection.mask, selected);
    }
    assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
}

fn assert_balloon_then_new_layer_history(h: &mut Harness, vector: bool) {
    h.release_farther();
    assert_eq!(
        h.app.doc.layers.len(),
        3,
        "layers={:?}, balloons={:?}",
        h.app
            .doc
            .layers
            .iter()
            .map(|layer| (layer.id, &layer.name))
            .collect::<Vec<_>>(),
        h.app
            .balloons()
            .iter()
            .map(|balloon| (balloon.id, balloon.layer_id))
            .collect::<Vec<_>>()
    );
    let ids = h
        .app
        .doc
        .layers
        .iter()
        .map(|layer| layer.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids.iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "completing a balloon before creating a layer must not reuse an ID: {ids:?}"
    );
    let balloons = h.app.balloons();
    assert_eq!(balloons.len(), 1);
    let balloon_layer_id = balloons[0].layer_id;
    assert_eq!(h.app.doc.layers[1].id, balloon_layer_id);
    assert_eq!(h.app.doc.layers[2].is_vector(), vector);
    assert!(!h.app.doc.layers[2].pixels.has_allocated_tiles());
    let balloon_pixels = h.app.doc.layers[1].pixels.to_dense();
    assert!(balloon_pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    for _ in 0..3 {
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(h.app.doc.layers[1].id, balloon_layer_id);
        assert_eq!(h.app.doc.layers[1].pixels.to_dense(), balloon_pixels);
        assert_eq!(h.app.balloons().len(), 1);
        h.command(egui::Key::Z, false);
        assert!(h.app.balloons().is_empty());
        assert_eq!(h.app.doc.layers.len(), 1);
        assert!(!h.app.history.can_undo());
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert_eq!(h.app.doc.layers[1].pixels.to_dense(), balloon_pixels);
        assert_eq!(h.app.balloons()[0].layer_id, balloon_layer_id);
        h.command(egui::Key::Z, true);
        assert_eq!(
            h.app
                .doc
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            ids
        );
        assert_eq!(h.app.doc.layers[2].is_vector(), vector);
        assert!(!h.app.doc.layers[2].pixels.has_allocated_tiles());
        assert_eq!(h.app.doc.layers[1].pixels.to_dense(), balloon_pixels);
        assert_eq!(h.app.balloons()[0].layer_id, balloon_layer_id);
    }
}

#[test]
fn new_raster_layer_during_balloon_creation_allocates_id_after_finishing_balloon() {
    let mut h = Harness::new(Tool::Balloon);
    h.move_to(20.0, 20.0);
    h.button(true);
    h.move_to(70.0, 60.0);
    assert!(h.app.balloons().is_empty());
    h.command(egui::Key::N, true);
    assert_balloon_then_new_layer_history(&mut h, false);
}

#[test]
fn new_vector_layer_during_balloon_creation_allocates_id_after_finishing_balloon() {
    let mut h = Harness::new(Tool::Balloon);
    h.move_to(20.0, 20.0);
    h.button(true);
    h.move_to(70.0, 60.0);
    assert!(h.app.balloons().is_empty());
    h.app.add_vector_layer();
    assert_balloon_then_new_layer_history(&mut h, true);
}

fn assert_selection_shortcut_finishes_stroke(key: egui::Key, shift: bool, selected_before: bool) {
    for move_before_release in [false, true] {
        let mut expected = Harness::new(Tool::Brush);
        let mut interrupted = Harness::new(Tool::Brush);
        if selected_before {
            for h in [&mut expected, &mut interrupted] {
                h.app.selection.rectangle(120, 80, (0, 0), (79, 79));
            }
        }
        let before_selection = (
            interrupted.app.selection.active,
            interrupted.app.selection.mask.clone(),
        );
        let before_pixels = interrupted.app.doc.layers[0].pixels.to_dense();
        expected.begin_gesture();
        interrupted.begin_gesture();

        // The command should match lifting the pen at the latest drawn point,
        // then changing the selection as a separate action.
        expected.button(false);
        expected.frame(Vec::new());
        let stroke_pixels = expected.app.doc.layers[0].pixels.to_dense();
        assert!(stroke_pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        expected.command(key, shift);
        let after_selection = (
            expected.app.selection.active,
            expected.app.selection.mask.clone(),
        );
        assert!(
            after_selection != before_selection,
            "shortcut did not change selection"
        );

        interrupted.command(key, shift);
        let active_after_command = interrupted.app.history.is_active();
        if move_before_release {
            interrupted.release_farther();
        } else {
            interrupted.button(false);
            interrupted.frame(Vec::new());
        }
        assert!(
            interrupted.app.doc.layers[0].pixels.to_dense() == stroke_pixels,
            "changing selection must preserve the finished stroke and ignore the held pointer"
        );
        assert!(
            (
                interrupted.app.selection.active,
                interrupted.app.selection.mask.clone()
            ) == after_selection
        );
        assert!(!interrupted.app.history.is_active());
        assert!(interrupted.app.stroke_builder.is_none());
        assert!(interrupted.app.active.is_empty());

        for _ in 0..3 {
            interrupted.command(egui::Key::Z, false);
            assert!(
                interrupted.app.doc.layers[0].pixels.to_dense() == stroke_pixels,
                "Undo selection must keep the preceding stroke"
            );
            assert!(
                (
                    interrupted.app.selection.active,
                    interrupted.app.selection.mask.clone()
                ) == before_selection
            );
            interrupted.command(egui::Key::Z, false);
            assert!(interrupted.app.doc.layers[0].pixels.to_dense() == before_pixels);
            assert!(!interrupted.app.history.can_undo());
            interrupted.command(egui::Key::Z, true);
            assert!(interrupted.app.doc.layers[0].pixels.to_dense() == stroke_pixels);
            assert!(
                (
                    interrupted.app.selection.active,
                    interrupted.app.selection.mask.clone()
                ) == before_selection
            );
            interrupted.command(egui::Key::Z, true);
            assert!(interrupted.app.doc.layers[0].pixels.to_dense() == stroke_pixels);
            assert!(
                (
                    interrupted.app.selection.active,
                    interrupted.app.selection.mask.clone()
                ) == after_selection
            );
            assert!(!interrupted.app.history.can_redo());
        }
        assert!(
            !active_after_command,
            "selection command left the stroke transaction open"
        );
    }
}

#[test]
fn select_all_shortcut_during_a_stroke_matches_release_then_command() {
    assert_selection_shortcut_finishes_stroke(egui::Key::A, false, false);
}

#[test]
fn deselect_shortcut_during_a_stroke_matches_release_then_command() {
    assert_selection_shortcut_finishes_stroke(egui::Key::D, false, true);
}

#[test]
fn invert_selection_shortcut_during_a_stroke_matches_release_then_command() {
    assert_selection_shortcut_finishes_stroke(egui::Key::I, true, true);
}

fn assert_rapid_selection_path(tool: Tool) {
    let mut h = Harness::new(tool);
    h.app.size = 8.0;
    let brush = &mut h.app.brushes[h.app.selected_brush];
    brush.size_source = DynamicSource::None;
    brush.opacity_source = DynamicSource::None;
    brush.settle = false;
    let pixels = h.app.doc.layers[0].pixels.to_dense();
    h.move_to(20.0, 40.0);
    h.button(true);
    // One real pointer event: the test harness adds no intermediate positions.
    h.move_to(55.0, 40.0);
    h.button(false);
    assert!(h.app.selection.mask[40 * 120 + 20] > 0);
    assert!(h.app.selection.mask[40 * 120 + 55] > 0);
    assert!(h.app.selection.mask[40 * 120 + 40] > 0);
    assert_eq!(h.app.selection.mask[48 * 120 + 40], 0);
    assert!(!h.app.history.is_active());
    assert!(h.app.doc.layers[0].pixels.to_dense() == pixels);
    let selection = h.app.selection.mask.clone();
    h.app.undo();
    assert!(!h.app.selection.active);
    assert!(h.app.selection.mask.is_empty());
    h.app.redo();
    assert!(h.app.selection.active);
    assert!(h.app.selection.mask == selection);
}

#[test]
fn rapid_selection_brush_covers_between_raw_pointer_events() {
    assert_rapid_selection_path(Tool::SelectionBrush);
}

#[test]
fn rapid_quick_mask_covers_between_raw_pointer_events() {
    assert_rapid_selection_path(Tool::QuickMask);
}

#[test]
fn selection_segment_keeps_pressure_width_and_deliberate_sparse_spacing() {
    let mut h = Harness::new(Tool::SelectionBrush);
    h.app.size = 12.0;
    let brush = &mut h.app.brushes[h.app.selected_brush];
    brush.size_source = DynamicSource::Pressure;
    brush.size_min = 0.0;
    brush.opacity_source = DynamicSource::None;
    brush.hardness = 1.0;
    h.app.begin_selection_operation(egui::Modifiers::NONE);
    h.app.dab(InkPoint::new(20.0, 40.0, 0.0, 0));
    h.app.dab(InkPoint::new(55.0, 40.0, 1.0, 100));
    h.app.finish_selection_operation();
    assert!(
        h.app.selection.mask[40 * 120 + 22] > 0,
        "thin start has a gap"
    );
    assert_eq!(h.app.selection.mask[42 * 120 + 22], 0);
    assert!(
        h.app.selection.mask[42 * 120 + 52] > 0,
        "pressure did not widen the end"
    );

    let mut sparse = Harness::new(Tool::SelectionBrush);
    sparse.app.size = 8.0;
    let brush = &mut sparse.app.brushes[sparse.app.selected_brush];
    brush.size_source = DynamicSource::None;
    brush.opacity_source = DynamicSource::None;
    brush.hardness = 1.0;
    brush.spacing = 1.5;
    sparse.move_to(20.0, 40.0);
    sparse.button(true);
    sparse.move_to(55.0, 40.0);
    sparse.button(false);
    assert!(sparse.app.selection.mask[40 * 120 + 32] > 0);
    assert_eq!(
        sparse.app.selection.mask[40 * 120 + 37],
        0,
        "intentional sparse spacing was removed"
    );
}

#[test]
fn selection_segment_preserves_fractional_subtract_and_off_page_symmetry() {
    let mut h = Harness::new(Tool::SelectionBrush);
    h.app.size = 8.0;
    h.app.symmetry_x = true;
    h.app.symmetry_center = Vec2::new(70.0, 40.0);
    h.app.selection.active = true;
    h.app.selection.mask = vec![200; 120 * 80];
    h.app.selection_erase = true;
    let brush = &mut h.app.brushes[h.app.selected_brush];
    brush.size_source = DynamicSource::None;
    brush.opacity_source = DynamicSource::None;
    brush.opacity = 0.2;
    brush.hardness = 1.0;
    h.app.begin_selection_operation(egui::Modifiers::NONE);
    h.app.selection_combine_mode = SelectionCombineMode::Subtract;
    // Canvas gesture start captures the old selection, then clears the working
    // stroke coverage before applying Subtract at gesture completion.
    h.app.selection.active = false;
    h.app.selection.mask.clear();
    h.app.dab(InkPoint::new(20.0, 40.0, 1.0, 0));
    h.app.dab(InkPoint::new(55.0, 40.0, 1.0, 100));
    h.app.finish_selection_operation();
    for x in [40, 100] {
        let edge = h.app.selection.mask[44 * 120 + x];
        assert!(
            edge > 0 && edge < 200,
            "fractional subtract at x={x}: {edge}"
        );
        assert!(h.app.selection.mask[40 * 120 + x] < 200);
        assert_eq!(h.app.selection.mask[48 * 120 + x], 200);
    }
    let selection = h.app.selection.mask.clone();
    h.app.undo();
    assert_eq!(h.app.selection.mask, vec![200; 120 * 80]);
    h.app.redo();
    assert!(h.app.selection.mask == selection);
}

#[test]
fn selection_segment_large_finite_inputs_have_document_bounded_work() {
    for (size, pressure, start, end, max_dabs, exact_mapped_zero) in [
        (3.0e38, 0.5, 1.0, 2.0, 1, false),
        (3.0e38, 0.5, -1.0e6, 1.0e6, 1, false),
        (3.0e38, 0.0, -1.0e6, 1.0e6, 1, false),
        (3.0e38, 0.0, -1.0e6, 1.0e6, 20, true),
        (8.0, 1.0, -1.0e6, 1.0e6, 10, false),
        (8.0, 1.0, -3.0e38, 3.0e38, 36, false),
    ] {
        let mut app = EfudeApp::default();
        app.doc = Document::new(4, 4);
        app.tool = Tool::SelectionBrush;
        app.size = size;
        let brush = &mut app.brushes[app.selected_brush];
        brush.size_source = DynamicSource::Pressure;
        brush.size_min = 0.0;
        brush.opacity_source = DynamicSource::None;
        brush.spacing = 0.22;
        app.begin_selection_operation(egui::Modifiers::NONE);
        let a = InkPoint::new(start, 2.0, pressure, 0);
        let mut b = InkPoint::new(end, 2.0, pressure, 100);
        if exact_mapped_zero {
            // Exercise a genuinely zero mapped pressure independently of the
            // existing Bezier mapper's small positive endpoint approximation.
            let initial = efude_brush::engine::dynamics(
                &app.brushes[app.selected_brush],
                &a,
                None,
                app.view_scale,
            );
            app.dynamic_size = initial.size;
            app.dynamic_opacity = initial.opacity;
            app.selection_dab(a);
            app.raster.last_dab = Some(a);
        } else {
            app.dab(a);
            b.pressure =
                app.brushes[app.selected_brush].map_pressure(app.global_pressure_curve(b.pressure));
        }
        let dynamics = efude_brush::engine::dynamics(
            &app.brushes[app.selected_brush],
            &b,
            app.raster.last_dab,
            app.view_scale,
        );
        let started = std::time::Instant::now();
        let dabs = app.selection_dab_segment(b, &dynamics);
        eprintln!(
            "size={size}, pressure={pressure}, exact_zero={exact_mapped_zero}, from={start}, to={end}, intermediate={dabs}, elapsed={:?}",
            started.elapsed()
        );
        assert!(dabs <= max_dabs, "size={size}, pressure={pressure}: {dabs}");
        app.dynamic_size = dynamics.size;
        app.dynamic_opacity = dynamics.opacity;
        app.selection_dab(b);
        app.raster.last_dab = Some(b);
        app.finish_selection_operation();
        assert_eq!(app.selection.mask.len(), 16);
        assert!(app.selection.mask.iter().any(|&coverage| coverage > 0));
        assert!(!app.doc.layers[0].pixels.has_allocated_tiles());
    }
    for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut app = EfudeApp::default();
        app.doc = Document::new(4, 4);
        app.tool = Tool::SelectionBrush;
        app.size = invalid;
        app.dab(InkPoint::new(1.0, 2.0, 1.0, 0));
        app.dab(InkPoint::new(2.0, 2.0, 1.0, 100));
        assert!(app.selection.mask.iter().all(|&coverage| coverage == 0));
        assert!(app.raster.last_dab.is_none());
    }
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut app = EfudeApp::default();
        app.doc = Document::new(4, 4);
        app.tool = Tool::SelectionBrush;
        let mut point = InkPoint::new(1.0, 2.0, 1.0, 0);
        // Set the malformed field directly: InkPoint::new already clamps Inf.
        point.pressure = invalid;
        app.dab(point);
        app.dab(InkPoint::new(invalid, 2.0, 1.0, 100));
        assert!(app.selection.mask.iter().all(|&coverage| coverage == 0));
        assert!(app.raster.last_dab.is_none());
    }
}

#[test]
fn focus_loss_closes_an_empty_stroke_waiting_for_tablet_samples() {
    let mut h = Harness::new(Tool::Brush);
    h.app.use_windows_ink = true;
    h.app.tablet_seen = true;
    h.move_to(20.0, 40.0);
    let initial_pixels = h.app.doc.layers[0].pixels.to_dense();
    let initial_token = h.app.history.state_token();
    h.button(true);
    assert!(h.app.selection_start.is_some());
    assert!(h.app.stroke_builder.is_some());
    assert!(
        h.app.active.is_empty(),
        "the native sample wait must still be active"
    );
    assert!(h.app.history.is_active());
    assert!(
        h.app.stroke_started_at.unwrap().elapsed() < std::time::Duration::from_millis(150),
        "focus-loss fixture must interrupt the native sample wait"
    );
    h.frame_focused(vec![egui::Event::WindowFocused(false)], false);
    assert!(h.app.active.is_empty());
    assert!(h.app.stroke_builder.is_none());
    assert!(h.app.selection_start.is_none());
    assert!(h.app.doc.layers[0].pixels.to_dense() == initial_pixels);
    assert_eq!(h.app.history.state_token(), initial_token);
    assert!(!h.app.history.can_undo());
    assert!(!h.app.history.can_redo());
    assert!(
        !h.app.history.is_active(),
        "focus loss left an empty tablet-wait stroke's history open"
    );
}
