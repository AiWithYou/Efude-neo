// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Command boundaries exercised with real canvas pointer and keyboard events.

use super::*;

struct Harness {
    app: EfudeApp,
    ctx: egui::Context,
    time: f64,
    pointer: Pos2,
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
        };
        for _ in 0..3 {
            h.frame(Vec::new());
        }
        h
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
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
        assert_eq!(h.app.doc.layers.len(), 2);
        assert!(h.app.balloons().is_empty());
        assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, false);
        assert_eq!(h.app.doc.layers.len(), 1);
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers.len(), 2);
        assert!(!h.app.doc.layers[1].pixels.has_allocated_tiles());
        h.command(egui::Key::Z, true);
        assert_eq!(h.app.doc.layers[1].pixels.to_dense(), balloon_pixels);
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
