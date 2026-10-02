// SPDX-FileCopyrightText: 2026 AiWithYou
// SPDX-License-Identifier: MPL-2.0
//! Stale native packets must not become part of a later canvas gesture.

use super::*;

fn frame(
    app: &mut EfudeApp,
    ctx: &egui::Context,
    time: &mut f64,
    focused: bool,
    events: Vec<egui::Event>,
) {
    *time += 1.0 / 60.0;
    let mut raw_input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
        time: Some(*time),
        focused,
        events,
        ..Default::default()
    };
    eframe::App::raw_input_hook(app, ctx, &mut raw_input);
    let _ = ctx.run(raw_input, |ctx| app.update_ui(ctx));
}

fn assert_stale_packets_are_discarded(lose_focus: bool) {
    let mut app = EfudeApp::default();
    app.doc = Document::new(120, 80);
    app.navigator_center = Vec2::new(60.0, 40.0);
    app.use_windows_ink = true;
    app.use_wintab = false;
    app.color = Color32::BLACK;
    let ctx = egui::Context::default();
    layout::apply_theme(&ctx);
    let mut time = 0.0;
    for _ in 0..3 {
        frame(&mut app, &ctx, &mut time, true, Vec::new());
    }
    let (rect, scale) = app.canvas_screen.unwrap();
    let screen = |x: f32, y: f32| rect.center() + Vec2::new(x - 60.0, y - 40.0) * scale;
    let old = screen(20.0, 20.0);
    let new = screen(90.0, 60.0);
    let packet = |pos: Pos2, time_ms: u64| efude_input::PenPacket {
        x: pos.x,
        y: pos.y,
        pressure: 1.0,
        tilt_x: 0.0,
        tilt_y: 0.0,
        rotation: 0.0,
        time_ms,
        received_at: std::time::Instant::now(),
    };
    // The bounded queue can retain more than one frame's 8192-packet drain.
    for i in 0..9000 {
        app.pen_queue.push(packet(old, i));
    }
    if !lose_focus {
        app.use_windows_ink = false;
    }
    frame(
        &mut app,
        &ctx,
        &mut time,
        !lose_focus,
        vec![egui::Event::PointerGone],
    );
    app.use_windows_ink = true;
    app.pen_queue.push(packet(new, 10000));
    frame(
        &mut app,
        &ctx,
        &mut time,
        true,
        vec![
            egui::Event::PointerMoved(new),
            egui::Event::PointerButton {
                pos: new,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        &mut app,
        &ctx,
        &mut time,
        true,
        vec![egui::Event::PointerButton {
            pos: new,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(
        app.doc.layers[0].pixels.pixel(90, 60)[3] > 0,
        "fresh native input must paint"
    );
    assert_eq!(
        app.doc.layers[0].pixels.pixel(20, 20)[3],
        0,
        "stale native input must not paint"
    );
    let painted = app.doc.layers[0].pixels.to_dense();
    app.undo();
    assert!(app.doc.layers[0].pixels.to_dense().iter().all(|v| *v == 0));
    app.redo();
    assert_eq!(app.doc.layers[0].pixels.to_dense(), painted);
}

#[test]
fn focus_loss_discards_the_entire_native_input_backlog() {
    assert_stale_packets_are_discarded(true);
}

#[test]
fn disabling_tablet_input_discards_the_entire_native_input_backlog() {
    assert_stale_packets_are_discarded(false);
}

struct FocusHarness {
    app: EfudeApp,
    ctx: egui::Context,
    time: f64,
    pointer: Pos2,
}

impl FocusHarness {
    fn new() -> Self {
        let mut app = EfudeApp::default();
        app.doc = Document::new(120, 80);
        app.navigator_center = Vec2::new(60.0, 40.0);
        app.use_windows_ink = false;
        app.use_wintab = false;
        app.color = Color32::BLACK;
        app.size = 6.0;
        app.brushes[app.selected_brush].settle = false;
        let ctx = egui::Context::default();
        layout::apply_theme(&ctx);
        let mut harness = Self {
            app,
            ctx,
            time: 0.0,
            pointer: Pos2::ZERO,
        };
        for _ in 0..3 {
            harness.frame(Vec::new());
        }
        harness
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        frame(&mut self.app, &self.ctx, &mut self.time, true, events);
    }

    fn screen_position(&self, x: f32, y: f32) -> Pos2 {
        let (rect, scale) = self.app.canvas_screen.unwrap();
        rect.center() + Vec2::new(x - 60.0, y - 40.0) * scale
    }

    fn move_to(&mut self, x: f32, y: f32) {
        self.pointer = self.screen_position(x, y);
        self.frame(vec![egui::Event::PointerMoved(self.pointer)]);
    }

    fn button(&mut self, pressed: bool) {
        self.frame(vec![pointer_button(self.pointer, pressed)]);
    }

    fn begin_stroke(&mut self) {
        self.move_to(20.0, 20.0);
        self.button(true);
        for x in [25.0, 35.0, 45.0, 55.0] {
            self.move_to(x, 20.0);
        }
        assert!(self.app.history.is_active());
    }

    fn focus_round_trip(&mut self, new_events: Vec<egui::Event>) {
        let mut events = vec![
            egui::Event::WindowFocused(false),
            egui::Event::WindowFocused(true),
        ];
        events.extend(new_events);
        self.frame(events);
    }
}

fn key_press(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn pointer_button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn same_frame_focus_round_trip_releases_the_old_borrowed_tool() {
    let mut h = FocusHarness::new();
    h.frame(vec![key_press(egui::Key::Space)]);
    assert_eq!(h.app.tool, Tool::Pan);
    h.focus_round_trip(Vec::new());
    assert_eq!(h.app.tool, Tool::Brush);
    assert!(h.app.held_tool_key.is_none());
    assert!(!h.ctx.input(|input| input.key_down(egui::Key::Space)));
}

#[test]
fn same_frame_focus_round_trip_keeps_a_new_tool_key() {
    for new_key in [egui::Key::E, egui::Key::Space] {
        let mut h = FocusHarness::new();
        h.frame(vec![key_press(egui::Key::Space)]);
        h.focus_round_trip(vec![key_press(new_key)]);
        let expected = if new_key == egui::Key::E {
            Tool::Eraser
        } else {
            Tool::Pan
        };
        assert_eq!(h.app.tool, expected);
        assert_eq!(h.app.held_tool_key.as_ref().unwrap().key, new_key);
        assert!(h.ctx.input(|input| input.key_down(new_key)));
        assert_eq!(h.app.held_tool_key.as_ref().unwrap().previous, Tool::Brush);
    }
}

#[test]
fn same_frame_focus_round_trip_finishes_the_old_stroke_without_a_tail() {
    let mut expected = FocusHarness::new();
    let mut interrupted = FocusHarness::new();
    expected.begin_stroke();
    interrupted.begin_stroke();
    expected.button(false);
    expected.frame(Vec::new());
    interrupted.focus_round_trip(Vec::new());
    let finished = expected.app.doc.layers[0].pixels.to_dense();
    assert!(
        !interrupted.app.history.is_active(),
        "focus loss must finish the stroke"
    );
    interrupted.move_to(95.0, 20.0);
    interrupted.button(false);
    interrupted.frame(Vec::new());
    assert!(
        interrupted.app.doc.layers[0].pixels.to_dense() == finished,
        "focus return must not extend the old stroke"
    );
    interrupted.app.undo();
    assert!(!interrupted.app.doc.layers[0].pixels.has_allocated_tiles());
    interrupted.app.redo();
    assert!(interrupted.app.doc.layers[0].pixels.to_dense() == finished);
}

#[test]
fn same_frame_focus_round_trip_keeps_a_new_pointer_press() {
    let mut expected = FocusHarness::new();
    let mut interrupted = FocusHarness::new();
    expected.begin_stroke();
    interrupted.begin_stroke();
    expected.button(false);
    expected.frame(Vec::new());
    let first_stroke = expected.app.doc.layers[0].pixels.to_dense();
    expected.move_to(80.0, 60.0);
    expected.button(true);
    interrupted.pointer = interrupted.screen_position(80.0, 60.0);
    interrupted.focus_round_trip(vec![
        egui::Event::PointerMoved(interrupted.pointer),
        pointer_button(interrupted.pointer, true),
    ]);
    assert!(
        interrupted.ctx.input(|input| input.pointer.primary_down()),
        "the post-focus press must remain held"
    );
    for h in [&mut expected, &mut interrupted] {
        h.move_to(95.0, 60.0);
        h.button(false);
        h.frame(Vec::new());
    }
    let both_strokes = expected.app.doc.layers[0].pixels.to_dense();
    assert!(
        interrupted.app.doc.layers[0].pixels.to_dense() == both_strokes,
        "the new press must draw an independent stroke after focus returns"
    );
    interrupted.app.undo();
    assert!(interrupted.app.doc.layers[0].pixels.to_dense() == first_stroke);
    interrupted.app.undo();
    assert!(!interrupted.app.doc.layers[0].pixels.has_allocated_tiles());
    interrupted.app.redo();
    assert!(interrupted.app.doc.layers[0].pixels.to_dense() == first_stroke);
    interrupted.app.redo();
    assert!(interrupted.app.doc.layers[0].pixels.to_dense() == both_strokes);
}

#[test]
fn focus_loss_key_up_is_safe_with_or_without_a_focus_event() {
    for marker in [None, Some(false), Some(true)] {
        let mut h = FocusHarness::new();
        h.frame(vec![key_press(egui::Key::Space)]);
        let mut key_up = key_press(egui::Key::Space);
        if let egui::Event::Key { pressed, .. } = &mut key_up {
            *pressed = false;
        }
        let mut events = vec![key_up];
        match marker {
            Some(false) => events.insert(0, egui::Event::WindowFocused(false)),
            Some(true) => events.push(egui::Event::WindowFocused(false)),
            None => {}
        }
        frame(&mut h.app, &h.ctx, &mut h.time, false, events);
        assert_eq!(h.app.tool, Tool::Brush);
        assert!(h.app.held_tool_key.is_none());
        assert!(!h.ctx.input(|input| input.key_down(egui::Key::Space)));
        h.frame(vec![egui::Event::WindowFocused(true)]);
        h.frame(vec![key_press(egui::Key::E)]);
        assert_eq!(h.app.tool, Tool::Eraser);
    }
}

#[test]
fn raw_focus_cleanup_keeps_clipboard_text_wheel_and_post_gain_input_in_order() {
    let mut h = FocusHarness::new();
    let fresh_pointer = h.screen_position(80.0, 60.0);
    let preserved = vec![
        egui::Event::Copy,
        egui::Event::Paste("captured paste".into()),
        egui::Event::Text("typed text".into()),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 72.0),
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let mut events = preserved.clone();
    events.extend([
        key_press(egui::Key::E),
        egui::Event::PointerMoved(h.screen_position(20.0, 20.0)),
        pointer_button(h.screen_position(20.0, 20.0), true),
        egui::Event::WindowFocused(false),
        egui::Event::WindowFocused(true),
        key_press(egui::Key::Space),
        egui::Event::WindowFocused(false),
        egui::Event::WindowFocused(true),
        key_press(egui::Key::E),
        egui::Event::PointerMoved(fresh_pointer),
        pointer_button(fresh_pointer, true),
    ]);
    let mut raw = egui::RawInput {
        focused: true,
        events,
        ..Default::default()
    };
    eframe::App::raw_input_hook(&mut h.app, &h.ctx, &mut raw);
    let mut expected = preserved;
    expected.extend([
        egui::Event::WindowFocused(false),
        egui::Event::WindowFocused(true),
        egui::Event::WindowFocused(false),
        egui::Event::WindowFocused(true),
        key_press(egui::Key::E),
        egui::Event::PointerMoved(fresh_pointer),
        pointer_button(fresh_pointer, true),
    ]);
    assert_eq!(raw.events, expected);
    // Drive egui without the application clipboard handlers: these events
    // should remain available without changing the OS clipboard in the test.
    let _ = h.ctx.run(raw, |ctx| {
        assert!(ctx.input(|input| input.key_down(egui::Key::E)));
        assert!(!ctx.input(|input| input.key_down(egui::Key::Space)));
        assert!(ctx.input(|input| input.pointer.primary_down()));
        assert_eq!(
            ctx.input(|input| input.pointer.interact_pos()),
            Some(fresh_pointer)
        );
        assert_eq!(
            ctx.input(|input| input.raw_scroll_delta),
            Vec2::new(0.0, 72.0)
        );
        assert_eq!(ctx.input(|input| input.events[..4].to_vec()), expected[..4]);
    });
}

struct ViewInputHarness {
    app: EfudeApp,
    ctx: egui::Context,
    time: f64,
    pointer: Pos2,
    pixels_per_point: f32,
}

impl ViewInputHarness {
    fn new(rotation: f32, flips: (bool, bool), pixels_per_point: f32, zoom: f32) -> Self {
        let mut app = EfudeApp::default();
        app.doc = Document::new(100, 200);
        app.navigator_center = Vec2::new(50.0, 100.0);
        app.view_rotation = rotation;
        (app.flip_x, app.flip_y) = flips;
        app.zoom = zoom;
        app.use_windows_ink = false;
        app.use_wintab = false;
        app.color = Color32::BLACK;
        app.size = 6.0;
        app.brushes[app.selected_brush].settle = false;
        let ctx = egui::Context::default();
        layout::apply_theme(&ctx);
        let mut h = Self {
            app,
            ctx,
            time: 0.0,
            pointer: Pos2::ZERO,
            pixels_per_point,
        };
        for _ in 0..3 {
            h.frame(Vec::new());
        }
        assert_eq!(h.ctx.pixels_per_point(), pixels_per_point);
        h
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(self.pixels_per_point);
        eframe::App::raw_input_hook(&mut self.app, &self.ctx, &mut raw);
        let _ = self.ctx.run(raw, |ctx| self.app.update_ui(ctx));
    }

    fn screen_position(&self, point: Vec2) -> Pos2 {
        let (rect, scale) = self.app.canvas_screen.unwrap();
        let mut delta = point - Vec2::new(50.0, 100.0);
        if self.app.flip_x {
            delta.x = -delta.x;
        }
        if self.app.flip_y {
            delta.y = -delta.y;
        }
        let (sin, cos) = self.app.view_rotation.to_radians().sin_cos();
        rect.center()
            + Vec2::new(delta.x * cos - delta.y * sin, delta.x * sin + delta.y * cos) * scale
    }

    fn push_packet(&mut self, point: Vec2) {
        let screen = self.screen_position(point);
        self.app.pen_queue.push(efude_input::PenPacket {
            x: screen.x,
            y: screen.y,
            pressure: 0.2,
            tilt_x: 0.0,
            tilt_y: 0.0,
            rotation: 0.0,
            time_ms: (self.time * 1000.0) as u64,
            received_at: std::time::Instant::now(),
        });
    }

    fn native_stroke(&mut self, points: &[Vec2]) {
        self.app.use_windows_ink = true;
        self.pointer = self.screen_position(points[0]);
        self.frame(vec![egui::Event::PointerMoved(self.pointer)]);
        self.push_packet(points[0]);
        self.frame(vec![pointer_button(self.pointer, true)]);
        for &point in &points[1..] {
            self.pointer = self.screen_position(point);
            self.push_packet(point);
            self.frame(vec![egui::Event::PointerMoved(self.pointer)]);
        }
        // Outside the document, but inside the unrotated screen rectangle at 90 degrees.
        self.push_packet(Vec2::new(-10.0, 100.0));
        self.frame(vec![pointer_button(self.pointer, false)]);
    }

    fn coalesced_window_stroke(&mut self) -> Vec<InkPoint> {
        let origin = self.screen_position(Vec2::new(50.25, 10.25));
        self.pointer = self.screen_position(Vec2::new(50.25, 70.25));
        self.frame(vec![
            egui::Event::PointerMoved(origin),
            pointer_button(origin, true),
            egui::Event::PointerMoved(self.pointer),
        ]);
        let samples = self.app.active.clone();
        self.frame(vec![pointer_button(self.pointer, false)]);
        samples
    }

    fn assert_image_and_history(&mut self, expected: &[u8]) {
        let painted = self.app.doc.layers[0].pixels.to_dense();
        assert!(painted.iter().any(|&value| value > 0));
        assert!(
            painted
                .iter()
                .zip(expected)
                .all(|(&a, &b)| a.abs_diff(b) <= 1),
            "view rotation {} changed the document image: maximum channel difference {}",
            self.app.view_rotation,
            painted
                .iter()
                .zip(expected)
                .map(|(&a, &b)| a.abs_diff(b))
                .max()
                .unwrap()
        );
        assert!(!self.app.history.is_active());
        self.app.undo();
        assert!(!self.app.doc.layers[0].pixels.has_allocated_tiles());
        self.app.redo();
        assert!(self.app.doc.layers[0].pixels.to_dense() == painted);
    }
}

#[test]
fn native_samples_follow_rotated_canvas_bounds_and_preserve_pressure() {
    let paths = [
        [
            Vec2::new(50.0, 10.0),
            Vec2::new(50.0, 25.0),
            Vec2::new(50.0, 40.0),
        ],
        [
            Vec2::new(50.0, 80.0),
            Vec2::new(50.0, 40.0),
            Vec2::new(50.0, 10.0),
        ],
    ];
    for (rotation, flips, pixels_per_point, zoom, grid_snap) in [
        (90.0, (false, false), 1.0, 1.0, false),
        (-90.0, (true, false), 1.5, 2.0, false),
        (35.0, (false, true), 2.0, 1.0, false),
        (90.0, (true, true), 1.25, 1.0, true),
    ] {
        for path in paths {
            let mut baseline = ViewInputHarness::new(0.0, flips, pixels_per_point, zoom);
            let mut rotated = ViewInputHarness::new(rotation, flips, pixels_per_point, zoom);
            for h in [&mut baseline, &mut rotated] {
                h.app.show_grid = grid_snap;
                h.app.grid_snap = grid_snap;
                h.app.grid_size = 64;
                h.native_stroke(&path);
            }
            let expected = baseline.app.doc.layers[0].pixels.to_dense();
            for h in [&mut baseline, &mut rotated] {
                let diagnostic = h.app.input_diagnostics;
                assert_eq!(diagnostic.tablet_samples, 3, "rotation {rotation}");
                assert_eq!(diagnostic.outside_canvas, 1, "rotation {rotation}");
                assert_eq!(diagnostic.window_fallbacks, 0, "rotation {rotation}");
                assert_eq!(diagnostic.min_pressure, Some(0.2));
                assert_eq!(diagnostic.max_pressure, Some(0.2));
                h.assert_image_and_history(&expected);
            }
        }
    }
}

#[test]
fn coalesced_window_press_origin_follows_rotated_canvas_bounds() {
    for (rotation, flips, pixels_per_point, zoom) in [
        (90.0, (false, false), 1.0, 1.0),
        (-90.0, (true, false), 1.5, 2.0),
        (35.0, (false, true), 2.0, 1.0),
    ] {
        let mut baseline = ViewInputHarness::new(0.0, flips, pixels_per_point, zoom);
        let mut rotated = ViewInputHarness::new(rotation, flips, pixels_per_point, zoom);
        let baseline_samples = baseline.coalesced_window_stroke();
        let rotated_samples = rotated.coalesced_window_stroke();
        assert_eq!(baseline_samples.len(), 2);
        assert_eq!(rotated_samples.len(), 2);
        for (expected, actual) in baseline_samples.iter().zip(&rotated_samples) {
            assert!(expected.position.distance(actual.position) < 0.001);
            assert_eq!(expected.pressure, actual.pressure);
            assert_eq!(expected.time_ms, actual.time_ms);
        }
        let expected = baseline.app.doc.layers[0].pixels.to_dense();
        assert!(baseline.app.doc.layers[0].pixels.pixel(50, 10)[3] > 0);
        assert!(
            rotated.app.doc.layers[0].pixels.pixel(50, 10)[3] > 0,
            "the coalesced frame discarded the press origin on the rotated canvas"
        );
        baseline.assert_image_and_history(&expected);
        rotated.assert_image_and_history(&expected);
    }
}

#[test]
fn coalesced_integer_window_contacts_keep_the_same_finished_image_at_35_degrees() {
    let stroke = |h: &mut ViewInputHarness| {
        let origin = h.screen_position(Vec2::new(50.0, 10.0));
        h.pointer = h.screen_position(Vec2::new(50.0, 70.0));
        h.frame(vec![
            egui::Event::PointerMoved(origin),
            pointer_button(origin, true),
            egui::Event::PointerMoved(h.pointer),
        ]);
        let samples = h.app.active.clone();
        h.frame(vec![pointer_button(h.pointer, false)]);
        samples
    };
    let mut baseline = ViewInputHarness::new(0.0, (false, false), 1.0, 1.0);
    let mut rotated = ViewInputHarness::new(35.0, (false, false), 1.0, 1.0);
    let a = stroke(&mut baseline);
    let b = stroke(&mut rotated);
    assert_eq!(a.len(), 2);
    assert_eq!(b.len(), 2);
    for (a, b) in a.iter().zip(&b) {
        assert!(a.position.distance(b.position) < 0.001);
        assert_eq!(a.pressure, b.pressure);
        assert_eq!(a.time_ms, b.time_ms);
    }
    let expected = baseline.app.doc.layers[0].pixels.to_dense();
    baseline.assert_image_and_history(&expected);
    rotated.assert_image_and_history(&expected);
}

#[test]
fn finished_live_contacts_match_replay_for_wet_and_build_up_brushes() {
    for kind in [BrushKind::Pen, BrushKind::Watercolor, BrushKind::Airbrush] {
        for settle in [false, true] {
            let mut brush = efude_brush::defaults()
                .into_iter()
                .find(|b| b.kind == kind)
                .unwrap();
            brush.settle = settle;
            brush.taper_in_pixels = true;
            brush.taper_start = 1.0;
            brush.taper_end = 2.0;
            // Isolate dab construction and preview restoration from the rim effect.
            brush.wet_edge_on = false;
            let mut live = ViewInputHarness::new(35.0, (false, false), 1.0, 1.0);
            live.app.brushes[live.app.selected_brush] = brush.clone();
            live.pointer = live.screen_position(Vec2::new(50.0, 10.0));
            live.frame(vec![
                egui::Event::PointerMoved(live.pointer),
                pointer_button(live.pointer, true),
            ]);
            for y in [30.0, 50.0, 70.0] {
                live.pointer = live.screen_position(Vec2::new(50.0, y));
                live.frame(vec![egui::Event::PointerMoved(live.pointer)]);
            }
            let points = live.app.active.clone();
            assert_eq!(points.len(), 4);
            assert!(live.app.doc.layers[0].pixels.has_allocated_tiles());
            live.frame(vec![pointer_button(live.pointer, false)]);
            let painted = live.app.doc.layers[0].pixels.to_dense();
            let mut replay = ViewInputHarness::new(35.0, (false, false), 1.0, 1.0);
            replay.app.brushes[replay.app.selected_brush] = brush;
            replay.app.history.begin();
            replay.app.render_replay(&points);
            replay.app.history.commit();
            assert!(
                painted == replay.app.doc.layers[0].pixels.to_dense(),
                "{kind:?}, settle={settle}"
            );
            live.assert_image_and_history(&painted);
        }
    }
}

#[cfg(target_os = "windows")]
mod native_contact_tests {
    use super::*;

    // Native POINTER_INFO flags: independent of the packet's pressure value.
    const CONTACT: u32 = 0x0004;
    const DOWN: u32 = 0x0001_0000;
    const UPDATE: u32 = 0x0002_0000;
    const UP: u32 = 0x0004_0000;

    struct ContactHarness {
        app: EfudeApp,
        ctx: egui::Context,
        time: f64,
    }

    impl ContactHarness {
        fn new() -> Self {
            let mut app = EfudeApp::default();
            app.doc = Document::new(64, 32);
            app.navigator_center = Vec2::new(32.0, 16.0);
            app.zoom = 6.0;
            app.use_windows_ink = false;
            app.use_wintab = false;
            app.color = Color32::BLACK;
            app.size = 4.0;
            app.workspace = egui_dock::DockState::new(vec![layout::Pane::Canvas]);
            let brush = &mut app.brushes[app.selected_brush];
            brush.size_source = DynamicSource::None;
            brush.opacity_source = DynamicSource::None;
            brush.settle = false;
            let ctx = egui::Context::default();
            layout::apply_theme(&ctx);
            let mut h = Self {
                app,
                ctx,
                time: 0.0,
            };
            for _ in 0..3 {
                h.frame(Vec::new());
            }
            h.app.use_windows_ink = true;
            h
        }

        fn position(&self, x: f32) -> Pos2 {
            let (rect, scale) = self.app.canvas_screen.unwrap();
            rect.center() + Vec2::new(x - 32.0, 0.0) * scale
        }

        fn moved(&self, x: f32) -> egui::Event {
            egui::Event::PointerMoved(self.position(x))
        }

        fn button(&self, x: f32, pressed: bool) -> egui::Event {
            pointer_button(self.position(x), pressed)
        }

        fn packet(&mut self, x: f32, pressure: f32, time_ms: u64, flags: u32) {
            let screen = self.position(x);
            self.app.pen_queue.push_windows_ink(
                efude_input::PenPacket {
                    x: screen.x,
                    y: screen.y,
                    pressure,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    rotation: 0.0,
                    time_ms,
                    received_at: std::time::Instant::now(),
                },
                flags,
            );
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
                time: Some(self.time),
                focused: true,
                events,
                ..Default::default()
            };
            eframe::App::raw_input_hook(&mut self.app, &self.ctx, &mut raw);
            let _ = self.ctx.run(raw, |ctx| self.app.update_ui(ctx));
        }

        fn assert_one_undo_and_redo(&mut self) {
            let painted = self.app.doc.layers[0].pixels.to_dense();
            assert!(!self.app.history.is_active());
            self.app.undo();
            assert!(!self.app.doc.layers[0].pixels.has_allocated_tiles());
            self.app.redo();
            assert!(self.app.doc.layers[0].pixels.to_dense() == painted);
        }
    }

    #[test]
    fn hover_before_down_does_not_join_zero_pressure_contact() {
        let mut h = ContactHarness::new();
        // A pure hover frame must remain blank.
        h.packet(8.0, 0.0, 100, UPDATE);
        h.frame(vec![h.moved(8.0)]);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        // A slow frame contains hover, pen-down and the first contact move.
        h.packet(8.0, 0.0, 101, UPDATE);
        h.packet(32.0, 0.0, 102, CONTACT | DOWN);
        h.packet(36.0, 0.0, 103, CONTACT | UPDATE);
        h.frame(vec![h.moved(32.0), h.button(32.0, true), h.moved(36.0)]);
        h.packet(36.0, 0.0, 104, UP);
        h.frame(vec![h.button(36.0, false)]);
        assert_eq!(h.app.doc.layers[0].pixels.pixel(20, 16)[3], 0);
        assert!(h.app.doc.layers[0].pixels.pixel(34, 16)[3] > 0);
        assert_eq!(h.app.input_diagnostics.tablet_samples, 2);
        h.assert_one_undo_and_redo();
    }

    #[test]
    fn hover_after_up_does_not_extend_the_finished_contact() {
        let mut h = ContactHarness::new();
        h.frame(vec![h.moved(16.0)]);
        h.packet(16.0, 0.5, 100, CONTACT | DOWN);
        h.frame(vec![h.button(16.0, true)]);
        h.packet(24.0, 0.5, 101, CONTACT | UPDATE);
        h.frame(vec![h.moved(24.0)]);
        // The last contact point, pen-up and following hover arrive together.
        h.packet(24.0, 0.5, 102, CONTACT | UPDATE);
        h.packet(26.0, 0.0, 103, UP);
        h.packet(50.0, 0.0, 104, UPDATE);
        h.frame(vec![h.button(26.0, false), h.moved(50.0)]);
        assert_eq!(h.app.doc.layers[0].pixels.pixel(40, 16)[3], 0);
        assert!(h.app.doc.layers[0].pixels.pixel(20, 16)[3] > 0);
        assert_eq!(h.app.input_diagnostics.tablet_samples, 3);
        h.assert_one_undo_and_redo();
    }

    #[test]
    fn native_release_drains_the_bounded_queue_through_its_last_contact() {
        let mut h = ContactHarness::new();
        h.frame(vec![h.moved(8.0)]);
        h.packet(8.0, 0.5, 100, CONTACT | DOWN);
        h.frame(vec![h.button(8.0, true)]);
        // More than the ordinary 8192-packet frame budget, still below the
        // fixed queue capacity. The last contact carries the real endpoint.
        for serial in 0..8192 {
            h.packet(24.0, 0.5, 101 + serial, CONTACT | UPDATE);
        }
        h.packet(48.0, 0.5, 8293, CONTACT | UPDATE);
        h.frame(vec![h.button(48.0, false)]);
        assert!(h.app.doc.layers[0].pixels.pixel(40, 16)[3] > 0);
        assert!(h.app.doc.layers[0].pixels.pixel(48, 16)[3] > 0);
        assert!(h.app.pen_queue.shared().is_empty());
        assert_eq!(h.app.input_diagnostics.tablet_samples, 8194);
        let finished = h.app.doc.layers[0].pixels.to_dense();
        h.frame(Vec::new());
        assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
        assert!(h.app.pen_queue.shared().is_empty());
        h.assert_one_undo_and_redo();
    }
}

#[cfg(target_os = "windows")]
mod coalesced_contact_tests {
    use super::*;

    const CONTACT: u32 = 0x0004;
    const DOWN: u32 = 0x0001_0000;
    const UPDATE: u32 = 0x0002_0000;
    const UP: u32 = 0x0004_0000;

    fn packet(h: &mut FocusHarness, position: (f32, f32), time_ms: u64, flags: u32) {
        let screen = h.screen_position(position.0, position.1);
        h.app.pen_queue.push_windows_ink(
            efude_input::PenPacket {
                x: screen.x,
                y: screen.y,
                pressure: 0.5,
                tilt_x: 0.0,
                tilt_y: 0.0,
                rotation: 0.0,
                time_ms,
                received_at: std::time::Instant::now(),
            },
            flags,
        );
    }

    fn moved(h: &FocusHarness, position: (f32, f32)) -> egui::Event {
        egui::Event::PointerMoved(h.screen_position(position.0, position.1))
    }

    fn button(h: &FocusHarness, position: (f32, f32), pressed: bool) -> egui::Event {
        pointer_button(h.screen_position(position.0, position.1), pressed)
    }

    fn two_contacts(native: bool, coalesced: bool) -> (FocusHarness, Option<Vec<u8>>) {
        let mut h = FocusHarness::new();
        let mut brush = efude_brush::defaults()
            .into_iter()
            .find(|brush| brush.kind == BrushKind::Pen)
            .unwrap();
        brush.settle = false;
        brush.stabilization = 0;
        brush.pull_distance = 0.0;
        brush.size_source = DynamicSource::None;
        brush.opacity_source = DynamicSource::None;
        h.app.brushes[h.app.selected_brush] = brush;
        h.app.use_windows_ink = native;
        h.app.use_wintab = false;
        h.frame(vec![moved(&h, (20.0, 20.0))]);
        if native {
            packet(&mut h, (20.0, 20.0), 100, CONTACT | DOWN);
        }
        h.frame(vec![button(&h, (20.0, 20.0), true)]);
        if native {
            packet(&mut h, (35.0, 20.0), 101, CONTACT | UPDATE);
        }
        h.frame(vec![moved(&h, (35.0, 20.0))]);
        assert!(
            h.app.stroke_builder.is_some() && h.app.history.is_active(),
            "the preceding frame must capture the first stroke"
        );

        if native {
            packet(&mut h, (45.0, 20.0), 102, CONTACT | UPDATE);
            packet(&mut h, (45.0, 20.0), 103, UP);
        }
        let old_release = button(&h, (45.0, 20.0), false);
        let first = if coalesced {
            None
        } else {
            h.frame(vec![old_release.clone()]);
            let first = h.app.doc.layers[0].pixels.to_dense();
            if native {
                packet(&mut h, (90.0, 60.0), 104, CONTACT | DOWN);
            }
            h.frame(vec![
                moved(&h, (90.0, 60.0)),
                button(&h, (90.0, 60.0), true),
            ]);
            assert!(h.app.stroke_builder.is_some() && h.app.history.is_active());
            Some(first)
        };

        if native {
            if coalesced {
                packet(&mut h, (90.0, 60.0), 104, CONTACT | DOWN);
            }
            packet(&mut h, (100.0, 60.0), 105, CONTACT | UPDATE);
            packet(&mut h, (100.0, 60.0), 106, UP);
        }
        let mut events = Vec::new();
        if coalesced {
            // Both contacts are completed inside one real raw-input frame.
            // Native End markers must retain the same boundary as these events.
            events.extend([
                old_release,
                moved(&h, (90.0, 60.0)),
                button(&h, (90.0, 60.0), true),
            ]);
        }
        events.extend([moved(&h, (100.0, 60.0)), button(&h, (100.0, 60.0), false)]);
        h.frame(events);
        let finished = h.app.doc.layers[0].pixels.to_dense();
        h.frame(Vec::new());
        assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
        (h, first)
    }

    fn assert_two_strokes_and_history(h: &mut FocusHarness, first: &[u8], both: &[u8]) {
        for y in 30..50 {
            for x in 60..80 {
                assert_eq!(
                    h.app.doc.layers[0].pixels.pixel(x, y)[3],
                    0,
                    "the contact boundary painted a bridge at ({x}, {y})"
                );
            }
        }
        assert!(h.app.doc.layers[0].pixels.pixel(30, 20)[3] > 0);
        assert!(
            h.app.doc.layers[0].pixels.pixel(95, 60)[3] > 0,
            "the second contact was discarded"
        );
        assert!(h.app.doc.layers[0].pixels.to_dense() == both);
        assert!(!h.app.history.is_active());
        assert!(h.app.stroke_builder.is_none());
        assert!(h.app.active.is_empty());
        assert!(h.app.pen_queue.shared_events().is_empty());
        h.app.undo();
        assert!(h.app.doc.layers[0].pixels.to_dense() == first);
        assert!(
            h.app.history.can_undo(),
            "the contacts need separate Undo actions"
        );
        h.app.undo();
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.app.redo();
        assert!(h.app.doc.layers[0].pixels.to_dense() == first);
        h.app.redo();
        assert!(h.app.doc.layers[0].pixels.to_dense() == both);
        assert!(!h.app.history.can_redo());
    }

    fn assert_coalesced_matches_separate_frames(native: bool) {
        let (mut control, first) = two_contacts(native, false);
        let first = first.unwrap();
        let both = control.app.doc.layers[0].pixels.to_dense();
        assert_two_strokes_and_history(&mut control, &first, &both);
        let (mut coalesced, _) = two_contacts(native, true);
        assert_two_strokes_and_history(&mut coalesced, &first, &both);
    }

    #[test]
    fn same_frame_native_contacts_keep_two_strokes_and_two_undos() {
        assert_coalesced_matches_separate_frames(true);
    }

    #[test]
    fn same_frame_window_contacts_keep_two_strokes_and_two_undos() {
        assert_coalesced_matches_separate_frames(false);
    }

    fn native_contacts_with_second_release_later(
        coalesced: bool,
        second_native: bool,
    ) -> (FocusHarness, Vec<u8>) {
        let mut h = FocusHarness::new();
        let mut brush = efude_brush::defaults()
            .into_iter()
            .find(|brush| brush.kind == BrushKind::Pen)
            .unwrap();
        brush.settle = false;
        brush.stabilization = 0;
        brush.pull_distance = 0.0;
        brush.size_source = DynamicSource::None;
        brush.opacity_source = DynamicSource::None;
        h.app.brushes[h.app.selected_brush] = brush;
        h.app.use_windows_ink = true;
        h.app.use_wintab = false;
        h.frame(vec![moved(&h, (20.0, 20.0))]);
        packet(&mut h, (20.0, 20.0), 100, CONTACT | DOWN);
        h.frame(vec![button(&h, (20.0, 20.0), true)]);
        packet(&mut h, (35.0, 20.0), 101, CONTACT | UPDATE);
        h.frame(vec![moved(&h, (35.0, 20.0))]);
        assert!(h.app.stroke_builder.is_some() && h.app.history.is_active());

        packet(&mut h, (45.0, 20.0), 102, CONTACT | UPDATE);
        packet(&mut h, (45.0, 20.0), 103, UP);
        let old_release = button(&h, (45.0, 20.0), false);
        let first = if coalesced {
            None
        } else {
            h.frame(vec![old_release.clone()]);
            Some(h.app.doc.layers[0].pixels.to_dense())
        };
        if second_native {
            packet(&mut h, (90.0, 60.0), 104, CONTACT | DOWN);
            packet(&mut h, (100.0, 60.0), 105, CONTACT | UPDATE);
        }
        let mut events = Vec::new();
        if coalesced {
            events.push(old_release);
        }
        events.extend([
            moved(&h, (90.0, 60.0)),
            button(&h, (90.0, 60.0), true),
            moved(&h, (100.0, 60.0)),
        ]);
        h.frame(events);
        assert!(
            h.app.stroke_builder.is_some() && h.app.history.is_active(),
            "the old release must leave the new contact unfinished"
        );
        assert_eq!(h.app.native_contact_active, second_native);

        if second_native {
            packet(&mut h, (110.0, 60.0), 106, CONTACT | UPDATE);
            packet(&mut h, (110.0, 60.0), 107, UP);
        }
        h.frame(vec![
            moved(&h, (110.0, 60.0)),
            button(&h, (110.0, 60.0), false),
        ]);
        let finished = h.app.doc.layers[0].pixels.to_dense();
        assert!(h.app.doc.layers[0].pixels.pixel(108, 60)[3] > 0);
        h.frame(Vec::new());
        assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
        (h, first.unwrap_or_default())
    }

    #[test]
    fn old_native_release_keeps_a_new_contact_active_until_its_later_release() {
        let (mut control, first) = native_contacts_with_second_release_later(false, true);
        let both = control.app.doc.layers[0].pixels.to_dense();
        assert_two_strokes_and_history(&mut control, &first, &both);
        let (mut coalesced, _) = native_contacts_with_second_release_later(true, true);
        assert_two_strokes_and_history(&mut coalesced, &first, &both);
    }

    #[test]
    fn sample_only_native_input_finishes_on_the_window_release() {
        let mut h = FocusHarness::new();
        let mut brush = efude_brush::defaults()
            .into_iter()
            .find(|brush| brush.kind == BrushKind::Pen)
            .unwrap();
        brush.settle = false;
        brush.stabilization = 0;
        brush.pull_distance = 0.0;
        brush.size_source = DynamicSource::None;
        brush.opacity_source = DynamicSource::None;
        h.app.brushes[h.app.selected_brush] = brush;
        h.app.use_windows_ink = false;
        h.app.use_wintab = true;
        h.frame(vec![moved(&h, (20.0, 20.0))]);
        h.frame(vec![button(&h, (20.0, 20.0), true)]);
        assert!(h.app.stroke_builder.is_some() && h.app.history.is_active());

        // Unknown WinTab tip mappings produce Sample events without inferring
        // Start/End. The original window capture/release remains responsible.
        for (position, time_ms) in [((35.0, 20.0), 101), ((45.0, 20.0), 102)] {
            let screen = h.screen_position(position.0, position.1);
            h.app
                .pen_queue
                .shared_events()
                .push(efude_input::PenInputEvent::Sample(efude_input::PenPacket {
                    x: screen.x,
                    y: screen.y,
                    pressure: 0.5,
                    tilt_x: 0.0,
                    tilt_y: 0.0,
                    rotation: 0.0,
                    time_ms,
                    received_at: std::time::Instant::now(),
                }))
                .unwrap();
        }
        h.frame(vec![
            moved(&h, (45.0, 20.0)),
            button(&h, (45.0, 20.0), false),
        ]);
        assert_eq!(h.app.input_diagnostics.tablet_samples, 2);
        assert!(!h.app.native_contact_active);
        assert!(!h.app.history.is_active());
        assert!(h.app.stroke_builder.is_none());
        assert!(h.app.active.is_empty());
        assert!(h.app.pen_queue.shared_events().is_empty());
        assert!(h.app.doc.layers[0].pixels.pixel(43, 20)[3] > 0);
        let finished = h.app.doc.layers[0].pixels.to_dense();
        h.frame(Vec::new());
        assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
        h.app.undo();
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.app.redo();
        assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
        assert!(!h.app.history.can_redo());
    }

    #[test]
    fn native_release_keeps_a_same_frame_new_window_contact() {
        let (mut control, first) = native_contacts_with_second_release_later(false, false);
        let both = control.app.doc.layers[0].pixels.to_dense();
        assert_two_strokes_and_history(&mut control, &first, &both);
        let (mut coalesced, _) = native_contacts_with_second_release_later(true, false);
        assert_two_strokes_and_history(&mut coalesced, &first, &both);
    }

    fn assert_first_frame_window_contacts(release_same_frame: bool) {
        let (mut control, first) = two_contacts(false, false);
        let first = first.unwrap();
        let both = control.app.doc.layers[0].pixels.to_dense();
        assert_two_strokes_and_history(&mut control, &first, &both);

        let mut h = FocusHarness::new();
        let mut brush = efude_brush::defaults()
            .into_iter()
            .find(|brush| brush.kind == BrushKind::Pen)
            .unwrap();
        brush.settle = false;
        brush.stabilization = 0;
        brush.pull_distance = 0.0;
        brush.size_source = DynamicSource::None;
        brush.opacity_source = DynamicSource::None;
        h.app.brushes[h.app.selected_brush] = brush;
        assert!(!h.app.use_windows_ink && !h.app.use_wintab);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(h.app.canvas_screen.is_some());
        assert!(h.app.selection_start.is_none());
        assert!(h.app.stroke_builder.is_none());
        // No preceding frame captures either stroke. Both presses arrive
        // together after the blank frames established the canvas cache.
        let mut events = vec![
            moved(&h, (20.0, 20.0)),
            button(&h, (20.0, 20.0), true),
            moved(&h, (35.0, 20.0)),
            button(&h, (45.0, 20.0), false),
            moved(&h, (90.0, 60.0)),
            button(&h, (90.0, 60.0), true),
            moved(&h, (100.0, 60.0)),
            button(&h, (100.0, 60.0), false),
        ];
        if !release_same_frame {
            events.pop();
        }
        h.frame(events);
        if !release_same_frame {
            assert!(
                h.app.stroke_builder.is_some() && h.app.history.is_active(),
                "the first release must not finish the still-held second contact"
            );
            assert!(h.ctx.input(|input| input.pointer.primary_down()));
            h.frame(vec![button(&h, (100.0, 60.0), false)]);
        }
        let finished = h.app.doc.layers[0].pixels.to_dense();
        h.frame(Vec::new());
        assert!(h.app.doc.layers[0].pixels.to_dense() == finished);
        eprintln!(
            "first-frame contacts: first_mid_alpha={}, second_mid_alpha={}",
            h.app.doc.layers[0].pixels.pixel(30, 20)[3],
            h.app.doc.layers[0].pixels.pixel(95, 60)[3]
        );
        assert_two_strokes_and_history(&mut h, &first, &both);
    }

    #[test]
    fn first_frame_window_contacts_keep_two_strokes_and_two_undos() {
        assert_first_frame_window_contacts(true);
    }

    #[test]
    fn first_frame_window_contacts_keep_the_second_stroke_until_its_release() {
        assert_first_frame_window_contacts(false);
    }

    fn assert_single_frame_window_contact(drag: bool) {
        let harness = || {
            let mut h = FocusHarness::new();
            let mut brush = efude_brush::defaults()
                .into_iter()
                .find(|brush| brush.kind == BrushKind::Pen)
                .unwrap();
            brush.settle = false;
            brush.stabilization = 0;
            brush.pull_distance = 0.0;
            brush.size_source = DynamicSource::None;
            brush.opacity_source = DynamicSource::None;
            h.app.brushes[h.app.selected_brush] = brush;
            h
        };
        let mut control = harness();
        control.move_to(20.0, 20.0);
        control.button(true);
        if drag {
            control.move_to(35.0, 20.0);
            control.move_to(45.0, 20.0);
        }
        control.button(false);
        let expected = control.app.doc.layers[0].pixels.to_dense();

        let mut h = harness();
        assert!(!h.app.use_windows_ink && !h.app.use_wintab);
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        let mut events = vec![moved(&h, (20.0, 20.0)), button(&h, (20.0, 20.0), true)];
        if drag {
            events.push(moved(&h, (35.0, 20.0)));
        }
        events.push(button(&h, (if drag { 45.0 } else { 20.0 }, 20.0), false));
        h.frame(events);
        assert!(h.app.doc.layers[0].pixels.pixel(20, 20)[3] > 0);
        if drag {
            assert!(h.app.doc.layers[0].pixels.pixel(30, 20)[3] > 0);
            assert!(h.app.doc.layers[0].pixels.pixel(43, 20)[3] > 0);
        }
        assert!(h.app.doc.layers[0].pixels.to_dense() == expected);
        assert!(!h.app.history.is_active());
        assert!(h.app.stroke_builder.is_none());
        assert!(h.app.active.is_empty());
        h.frame(Vec::new());
        assert!(h.app.doc.layers[0].pixels.to_dense() == expected);
        h.app.undo();
        assert!(!h.app.doc.layers[0].pixels.has_allocated_tiles());
        assert!(!h.app.history.can_undo());
        h.app.redo();
        assert!(h.app.doc.layers[0].pixels.to_dense() == expected);
        assert!(!h.app.history.can_redo());
    }

    #[test]
    fn single_frame_window_tap_matches_separate_frames_and_one_undo() {
        assert_single_frame_window_contact(false);
    }

    #[test]
    fn single_frame_window_drag_matches_separate_frames_and_one_undo() {
        assert_single_frame_window_contact(true);
    }
}
