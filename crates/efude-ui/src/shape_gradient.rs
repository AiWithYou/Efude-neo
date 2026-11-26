// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use super::*;

use efude_canvas::{GradientKind, GradientStop, ShapeKind, ShapeStyle};

pub(super) const HANDLES: [(f32, f32); 8] = [
    (0., 0.),
    (0.5, 0.),
    (1., 0.),
    (1., 0.5),
    (1., 1.),
    (0.5, 1.),
    (0., 1.),
    (0., 0.5),
];
pub(super) fn handle(rect: Rect, (x, y): (f32, f32)) -> Pos2 {
    rect.min + rect.size() * Vec2::new(x, y)
}
pub(super) fn snap_angle(angle: f32) -> f32 {
    (angle.to_degrees() / 15.).round().to_radians() * 15.
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PreviewRegion {
    pub(super) rect: Rect,
    pub(super) size: [usize; 2],
}

impl PreviewRegion {
    pub(super) fn new(rect: Rect, density: f32, limit: usize) -> Self {
        let limit = limit as f32;
        let density = density.min(1.).min(limit / rect.size().max_elem());
        let dim = |v: f32| (v * density).ceil().clamp(1., limit) as usize;
        Self {
            rect,
            size: [dim(rect.width()), dim(rect.height())],
        }
    }
    pub(super) fn image(self, mut sample: impl FnMut(u32, u32) -> [u8; 4]) -> egui::ColorImage {
        let [w, h] = self.size;
        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = self.rect.min
                    + self.rect.size()
                        * Vec2::new((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let [r, g, b, _] = sample(p.x as u32, p.y as u32);
                pixels.push(Color32::from_rgb(r, g, b));
            }
        }
        egui::ColorImage {
            size: self.size,
            pixels,
        }
    }
}

#[derive(Default)]
pub(super) struct DrawingTools {
    gradient: usize,
    colors: usize,
    reverse: bool,
    shape: usize,
    style: usize,
    width: f32,
    radius: f32,
    sides: u32,
    ratio: usize,
    pub(super) drag: Option<(Vec2, Vec2)>,
    pub(super) crop: Option<Rect>,
    crop_grip: Option<(Rect, usize)>,
    pub(super) preview: Option<(String, egui::TextureHandle, PreviewRegion)>,
    pub(super) last_brush_point: Option<(u64, Vec2)>,
    pub(super) size_drag: Option<(Pos2, f32)>,
    pub(super) rotate_drag: Option<(f32, f32)>,
    pub(super) sampled_from: Option<[u8; 4]>,
}
fn choices(ui: &mut egui::Ui, selected: &mut usize, english: bool, options: &[(&str, &str)]) {
    ui.horizontal_wrapped(|ui| {
        for (i, &(ja, en)) in options.iter().enumerate() {
            ui.selectable_value(selected, i, if english { en } else { ja });
        }
    });
}
impl DrawingTools {
    fn prepare(
        &self,
        tool: Tool,
        size: [u32; 2],
        colors: [[u8; 4]; 2],
    ) -> Option<efude_canvas::GeometryPaint> {
        let (a, b) = self.drag?;
        let (a, b) = ([a.x, a.y], [b.x, b.y]);
        let [fg, bg] = colors;
        if tool == Tool::Gradient {
            let colors = match self.colors {
                1 => vec![fg, [fg[0], fg[1], fg[2], 0]],
                2 => vec![[0, 0, 0, 255], [255; 4]],
                3 => vec![[48, 35, 92, 255], [230, 92, 101, 255], [255, 200, 110, 255]],
                4 => vec![[52, 101, 185, 255], [180, 221, 248, 255]],
                5 => vec![
                    [255, 0, 0, 255],
                    [255, 255, 0, 255],
                    [0, 255, 0, 255],
                    [0, 255, 255, 255],
                    [0, 0, 255, 255],
                    [170, 0, 255, 255],
                ],
                _ => vec![fg, bg],
            };
            let len = colors.len();
            let stops: Vec<_> = (0..len)
                .map(|i| GradientStop {
                    position: i as f32 / (len - 1) as f32,
                    color: colors[if self.reverse { len - 1 - i } else { i }],
                })
                .collect();
            efude_canvas::prepare_gradient(
                size,
                a,
                b,
                [
                    GradientKind::Linear,
                    GradientKind::Radial,
                    GradientKind::Reflected,
                ][self.gradient],
                &stops,
            )
        } else {
            let kind = [
                ShapeKind::Rectangle,
                ShapeKind::RoundedRect {
                    radius: self.radius,
                },
                ShapeKind::Ellipse,
                ShapeKind::Polygon {
                    sides: self.sides.clamp(3, 12),
                },
                ShapeKind::Line,
            ][self.shape];
            let style = ShapeStyle {
                fill: (self.style != 1).then_some(fg),
                stroke: (self.style != 0 || self.shape == 4)
                    .then_some((if self.style == 2 { bg } else { fg }, self.width.max(1.))),
            };
            efude_canvas::prepare_shape(size, a, b, kind, style)
        }
    }
}
impl EfudeApp {
    pub(super) fn cancel_studio_session(&mut self) {
        if self.transform_session.take().is_some() {
            self.transform_scale_x = 1.;
            self.transform_scale_y = 1.;
            self.transform_angle = 0.;
            self.mesh_offsets = [[0.; 2]; 16];
        }
        self.studio.drag = None;
        self.studio.crop = None;
        self.studio.crop_grip = None;
        self.studio.preview = None;
        self.studio.size_drag = None;
        self.studio.rotate_drag = None;
        self.studio.sampled_from = None;
    }
    pub(super) fn session_keys(&mut self, ctx: &egui::Context) {
        if self.transform_session.is_none()
            && self.studio.crop.is_none()
            && self.studio.drag.is_none()
        {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.cancel_studio_session();
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            self.apply_studio_session();
        }
    }
    pub(super) fn selection_rectangle(&self) -> Option<[u32; 4]> {
        self.selection.bounds(self.doc.width, self.doc.height)
    }
    pub(super) fn apply_crop(&mut self, bounds: [u32; 4]) {
        self.finish_pending_canvas_gesture();
        if self.comic_doc().is_some() {
            self.status = self
                .text(
                    "ページ設定付き漫画原稿は切り抜けません",
                    "Cropping is unavailable for a comic with page settings",
                )
                .into();
            return;
        }
        self.history.begin();
        if efude_canvas::crop_document(&mut self.doc, bounds, &mut self.history) == Ok(true) {
            let mut balloons = self.balloons();
            if !balloons.is_empty() {
                let offset = glam::Vec2::new(bounds[0] as f32, bounds[1] as f32);
                for b in &mut balloons {
                    b.center -= offset;
                    for tail in &mut b.tails {
                        tail.tip -= offset;
                    }
                }
                self.store_balloons(&balloons);
            }
            self.history.record_selection_change(
                self.selection.active,
                std::mem::take(&mut self.selection.mask),
                false,
                Vec::new(),
            );
            self.selection.clear();
            self.canvas_width_input = self.doc.width;
            self.canvas_height_input = self.doc.height;
            self.navigator_center = Vec2::new(self.doc.width as f32, self.doc.height as f32) * 0.5;
            self.zoom = 1.;
        }
        self.history.commit_labeled(self.text("切り抜き", "Crop"));
        self.cancel_studio_session();
    }
    fn crop_bounds(&self) -> Option<[u32; 4]> {
        let r = self.studio.crop?;
        let x = (r.min.x.round() as u32).min(self.doc.width);
        let y = (r.min.y.round() as u32).min(self.doc.height);
        let w = (r.max.x.round() as u32)
            .min(self.doc.width)
            .saturating_sub(x);
        let h = (r.max.y.round() as u32)
            .min(self.doc.height)
            .saturating_sub(y);
        (w > 0 && h > 0).then_some([x, y, w, h])
    }
    fn apply_studio_session(&mut self) {
        if self.transform_session.is_some() {
            self.commit_transform();
        } else if let Some(bounds) = self.crop_bounds() {
            self.apply_crop(bounds);
        }
    }
    pub(super) fn geometry_menu(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                !self.editing_mask && !self.doc.layers[self.selected_layer].alpha_lock,
                egui::Button::new(self.text("自由変形 (Ctrl+T)", "Free transform (Ctrl+T)")),
            )
            .clicked()
        {
            self.start_transform(false);
        }
        if ui
            .add_enabled(
                !self.editing_mask && !self.doc.layers[self.selected_layer].alpha_lock,
                egui::Button::new(self.text("メッシュ変形", "Mesh transform")),
            )
            .clicked()
        {
            self.start_transform(true);
        }
        let can_crop = self.comic_doc().is_none();
        let crop_hint = self.text(
            "ページ設定付き漫画原稿は切り抜けません",
            "Cropping is unavailable for a comic with page settings",
        );
        if ui
            .add_enabled(
                can_crop && self.selection.active,
                egui::Button::new(self.text("選択範囲でトリミング", "Crop to selection")),
            )
            .on_disabled_hover_text(if can_crop {
                self.text("先に範囲を選択してください", "Select an area first")
            } else {
                crop_hint
            })
            .clicked()
            && let Some(r) = self.selection_rectangle()
        {
            self.apply_crop(r);
        }
        if ui
            .add_enabled(
                can_crop,
                egui::Button::new(self.text("透明余白をトリミング", "Trim transparency")),
            )
            .on_disabled_hover_text(crop_hint)
            .clicked()
            && let Some(r) = efude_canvas::document_content_bounds(&self.doc)
        {
            self.apply_crop(r);
        }
    }
    pub(super) fn studio_options(&mut self, ui: &mut egui::Ui) -> bool {
        let en = self.language_english;
        let t = |ja, en_text| if en { en_text } else { ja };
        if self.transform_session.is_some() {
            self.transform_controls(ui);
        } else {
            let s = &mut self.studio;
            match self.tool {
                Tool::Gradient => {
                    choices(
                        ui,
                        &mut s.gradient,
                        en,
                        &[
                            ("線形", "Linear"),
                            ("放射", "Radial"),
                            ("反射", "Reflected"),
                        ],
                    );
                    choices(
                        ui,
                        &mut s.colors,
                        en,
                        &[
                            ("描画色→背景色", "Foreground → background"),
                            ("描画色→透明", "Foreground → transparent"),
                            ("白黒", "Monochrome"),
                            ("夕焼け", "Sunset"),
                            ("空", "Sky"),
                            ("虹", "Rainbow"),
                        ],
                    );
                    ui.checkbox(&mut s.reverse, t("反転", "Reverse"));
                    return true;
                }
                Tool::Shape => {
                    choices(
                        ui,
                        &mut s.shape,
                        en,
                        &[
                            ("矩形", "Rectangle"),
                            ("角丸矩形", "Rounded rectangle"),
                            ("楕円", "Ellipse"),
                            ("正多角形", "Polygon"),
                            ("直線", "Line"),
                        ],
                    );
                    choices(
                        ui,
                        &mut s.style,
                        en,
                        &[
                            ("塗り", "Fill"),
                            ("線", "Stroke"),
                            ("塗り＋線", "Fill and stroke"),
                        ],
                    );
                    ui.add(
                        egui::Slider::new(&mut s.width, 1.0..=128.).text(t("線幅", "Stroke width")),
                    );
                    if s.shape == 1 {
                        ui.add(
                            egui::Slider::new(&mut s.radius, 0.0..=256.)
                                .text(t("角丸半径", "Radius")),
                        );
                    }
                    if s.shape == 3 {
                        ui.add(egui::Slider::new(&mut s.sides, 3..=12).text(t("頂点数", "Sides")));
                    }
                    return true;
                }
                Tool::Crop => {
                    choices(
                        ui,
                        &mut s.ratio,
                        en,
                        &[
                            ("自由", "Free"),
                            ("1:1", "1:1"),
                            ("16:9", "16:9"),
                            ("4:3", "4:3"),
                            ("3:2", "3:2"),
                            ("B5", "B5"),
                            ("A4", "A4"),
                        ],
                    );
                    if let Some([x, y, w, h]) = self.crop_bounds() {
                        ui.label(format!("X {x}   Y {y}   {w} × {h}"));
                    }
                }
                _ => return false,
            }
        }
        if ui.button(t("適用 (Enter)", "Apply (Enter)")).clicked() {
            self.apply_studio_session();
        }
        if ui.button(t("キャンセル (Esc)", "Cancel (Esc)")).clicked() {
            self.cancel_studio_session();
        }
        true
    }
    pub(super) fn geometry_canvas(
        &mut self,
        response: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
        scale: f32,
        to_screen: &impl Fn(Vec2) -> Pos2,
    ) -> bool {
        if self.transform_session.is_some() {
            self.transform_canvas(response, painter, rect, scale, to_screen);
            return true;
        }
        if !matches!(self.tool, Tool::Shape | Tool::Gradient | Tool::Crop) {
            return false;
        }
        let ctx = &response.ctx;
        let mods = ctx.input(|i| i.modifiers);
        let primary = egui::PointerButton::Primary;
        let cursor = response
            .interact_pointer_pos()
            .map(|p| self.document_position_unsnapped(p, rect, scale));
        let doc_size = Vec2::new(self.doc.width as f32, self.doc.height as f32);
        if response.drag_started_by(primary)
            && let Some(pos) = ctx.input(|i| i.pointer.press_origin())
        {
            let p = self.document_position_unsnapped(pos, rect, scale);
            self.studio.drag = Some((p, p));
            self.studio.crop_grip = None;
            if self.tool == Tool::Crop
                && let Some(r) = self.studio.crop
            {
                let grip = HANDLES
                    .iter()
                    .position(|&uv| to_screen(handle(r, uv).to_vec2()).distance(pos) < 8.)
                    .or_else(|| r.contains(p.to_pos2()).then_some(8));
                self.studio.crop_grip = grip.map(|g| (r, g));
            }
        }
        if let (Some((origin, _)), Some(p)) = (self.studio.drag, cursor) {
            let mut delta = p - origin;
            if self.tool == Tool::Crop {
                let (anchor, mut end) = if let Some((r, grip)) = self.studio.crop_grip {
                    if grip == 8 {
                        let min = (r.min.to_vec2() + delta).clamp(Vec2::ZERO, doc_size - r.size());
                        (min, min + r.size())
                    } else {
                        let (x, y) = HANDLES[grip];
                        let opposite = handle(r, (1. - x, 1. - y));
                        (
                            Vec2::new(
                                if x == 0.5 { r.left() } else { opposite.x },
                                if y == 0.5 { r.top() } else { opposite.y },
                            ),
                            Vec2::new(
                                if x == 0.5 { r.right() } else { p.x },
                                if y == 0.5 { r.bottom() } else { p.y },
                            ),
                        )
                    }
                } else {
                    (origin, p)
                };
                let anchor = anchor.clamp(Vec2::ZERO, doc_size);
                let ratio =
                    [0., 1., 16. / 9., 4. / 3., 1.5, 182. / 257., 210. / 297.][self.studio.ratio];
                if ratio > 0. && self.studio.crop_grip.is_none_or(|(_, g)| g != 8) {
                    let d = end - anchor;
                    let sign = Vec2::new(
                        if d.x < 0. { -1. } else { 1. },
                        if d.y < 0. { -1. } else { 1. },
                    );
                    let space = Vec2::new(
                        if sign.x < 0. {
                            anchor.x
                        } else {
                            doc_size.x - anchor.x
                        },
                        if sign.y < 0. {
                            anchor.y
                        } else {
                            doc_size.y - anchor.y
                        },
                    );
                    let w =
                        d.x.abs()
                            .max(d.y.abs() * ratio)
                            .min(space.x)
                            .min(space.y * ratio);
                    end = anchor + Vec2::new(w, w / ratio) * sign;
                }
                self.studio.crop = Some(Rect::from_two_pos(
                    anchor.to_pos2(),
                    end.clamp(Vec2::ZERO, doc_size).to_pos2(),
                ));
            } else {
                if mods.shift {
                    if self.tool == Tool::Gradient || self.studio.shape == 4 {
                        delta = Vec2::angled(snap_angle(delta.angle())) * delta.length();
                    } else {
                        let size = delta.x.abs().max(delta.y.abs());
                        delta = Vec2::new(delta.x.signum() * size, delta.y.signum() * size);
                    }
                }
                let a = if mods.alt && self.tool == Tool::Shape {
                    origin - delta
                } else {
                    origin
                };
                self.studio.drag = Some((a, origin + delta));
                let valid = !self.editing_mask
                    && !self.doc.layers[self.selected_layer].locked
                    && !self.is_reference_layer(self.selected_layer)
                    && !self.doc.layers[self.selected_layer].is_vector()
                    && self.doc.layers[self.selected_layer].kind == LayerKind::Raster;
                if valid {
                    let key = format!(
                        "{:?} {:?} {:?} {:?} {:?} {} {} {} {} {} {} {} {}",
                        self.tool,
                        self.studio.drag,
                        self.color,
                        self.secondary_color,
                        self.history.state_token(),
                        self.studio.gradient,
                        self.studio.colors,
                        self.studio.reverse,
                        self.studio.shape,
                        self.studio.style,
                        self.studio.width,
                        self.studio.radius,
                        self.studio.sides
                    );
                    if let Some(region) = self.preview_region(
                        rect,
                        painter.clip_rect(),
                        scale,
                        ctx.pixels_per_point(),
                        self.max_texture_side(ctx).min(2048),
                    ) {
                        self.preview_drawing(ctx, region, key);
                        self.show_geometry_preview(painter, to_screen);
                    }
                    if response.drag_stopped_by(primary) {
                        self.history.begin();
                        let size = [self.doc.width, self.doc.height];
                        if let Some(paint) =
                            self.studio
                                .prepare(self.tool, size, [self.color, self.secondary_color])
                        {
                            paint.draw(
                                &mut self.doc.layers[self.selected_layer],
                                size,
                                self.selection.active.then_some(&self.selection.mask),
                                &mut self.history,
                            );
                        }
                        if self.tool == Tool::Shape {
                            self.commit_paint_history("図形", "Shape");
                        } else {
                            self.commit_paint_history("グラデーション", "Gradient");
                        }
                    }
                }
                let guide = Stroke::new(1., Color32::WHITE);
                if self.tool == Tool::Gradient || self.studio.shape == 4 {
                    painter.line_segment([to_screen(a), to_screen(origin + delta)], guide);
                } else {
                    let r = Rect::from_two_pos(a.to_pos2(), (origin + delta).to_pos2());
                    painter.add(egui::Shape::closed_line(
                        [
                            r.left_top(),
                            r.right_top(),
                            r.right_bottom(),
                            r.left_bottom(),
                        ]
                        .map(|p| to_screen(p.to_vec2()))
                        .to_vec(),
                        guide,
                    ));
                }
                self.studio.drag = Some((origin, p));
            }
        }
        if self.tool == Tool::Crop
            && let Some(r) = self.studio.crop
        {
            let page = Rect::from_min_size(Pos2::ZERO, doc_size);
            for strip in [
                Rect::from_min_max(page.min, Pos2::new(page.right(), r.top())),
                Rect::from_min_max(Pos2::new(0., r.bottom()), page.max),
                Rect::from_min_max(Pos2::new(0., r.top()), r.left_bottom()),
                Rect::from_min_max(r.right_top(), Pos2::new(page.right(), r.bottom())),
            ] {
                if strip.is_positive() {
                    painter.add(egui::Shape::convex_polygon(
                        [
                            strip.left_top(),
                            strip.right_top(),
                            strip.right_bottom(),
                            strip.left_bottom(),
                        ]
                        .map(|p| to_screen(p.to_vec2()))
                        .to_vec(),
                        Color32::from_black_alpha(135),
                        Stroke::NONE,
                    ));
                }
            }
            painter.add(egui::Shape::closed_line(
                [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ]
                .map(|p| to_screen(p.to_vec2()))
                .to_vec(),
                Stroke::new(1., Color32::WHITE),
            ));
            for uv in HANDLES {
                painter.circle_filled(to_screen(handle(r, uv).to_vec2()), 3., Color32::WHITE);
            }
        }
        if response.drag_stopped_by(primary) {
            self.studio.drag = None;
            self.studio.preview = None;
            self.studio.crop_grip = None;
        }
        true
    }
    pub(super) fn max_texture_side(&self, ctx: &egui::Context) -> usize {
        let limit = ctx.input(|i| i.max_texture_side);
        self.gpu_render_state
            .as_ref()
            .map_or(limit, |s| {
                limit.min(s.device.limits().max_texture_dimension_2d as usize)
            })
            .max(1)
    }
    pub(super) fn needs_viewport_texture(&self, ctx: &egui::Context) -> bool {
        self.doc.width.max(self.doc.height) as usize > self.max_texture_side(ctx)
    }
    pub(super) fn display_region(&self, region: PreviewRegion) -> egui::ColorImage {
        let sampler = efude_canvas::CompositeSampler::new(&self.doc);
        let checker = self.display_checker();
        region.image(|x, y| sampler.display(&self.doc, x, y, checker, None))
    }
    pub(super) fn preview_region(
        &self,
        rect: Rect,
        clip: Rect,
        scale: f32,
        pixel_ratio: f32,
        max_side: usize,
    ) -> Option<PreviewRegion> {
        let visible = clip;
        if !visible.is_positive() || scale <= 0. {
            return None;
        }
        let mut bounds = Rect::NOTHING;
        for p in [
            visible.left_top(),
            visible.right_top(),
            visible.left_bottom(),
            visible.right_bottom(),
        ] {
            bounds.extend_with(self.document_position_unsnapped(p, rect, scale).to_pos2());
        }
        let page = Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(self.doc.width as f32, self.doc.height as f32),
        );
        let bounds = Rect::from_min_max(bounds.min.floor(), bounds.max.ceil()).intersect(page);
        if !bounds.is_positive() {
            return None;
        }
        let screen_side = (visible.size().max_elem() * pixel_ratio).ceil().max(1.) as usize;
        Some(PreviewRegion::new(
            bounds,
            scale * pixel_ratio,
            max_side.min(screen_side),
        ))
    }

    pub(super) fn preview_drawing(
        &mut self,
        ctx: &egui::Context,
        region: PreviewRegion,
        key: String,
    ) {
        if self.preview_cached(&key, region) {
            return;
        }
        let paint = self.studio.prepare(
            self.tool,
            [self.doc.width, self.doc.height],
            [self.color, self.secondary_color],
        );
        let width = self.doc.width;
        self.cache_geometry_preview(ctx, key, region, |layer, selection, x, y| {
            let base = layer.pixels.pixel(x, y);
            let coverage = if selection.active {
                selection
                    .mask
                    .get((y * width + x) as usize)
                    .copied()
                    .unwrap_or(0) as f32
                    / 255.
            } else {
                1.
            };
            let pixel = paint
                .as_ref()
                .map_or(base, |p| p.pixel(x, y, base, coverage, layer.alpha_lock));
            (
                pixel,
                layer
                    .mask
                    .as_ref()
                    .map_or(255, |m| m.pixel_or_tile_default(x, y, [255; 4])[0]),
            )
        });
    }
    pub(super) fn preview_cached(&self, key: &str, region: PreviewRegion) -> bool {
        self.studio
            .preview
            .as_ref()
            .is_some_and(|(k, _, r)| k == key && *r == region)
    }
    pub(super) fn cache_geometry_preview(
        &mut self,
        ctx: &egui::Context,
        key: String,
        region: PreviewRegion,
        sample: impl Fn(&efude_canvas::Layer, &Selection, u32, u32) -> ([u8; 4], u8),
    ) {
        let layer = &self.doc.layers[self.selected_layer];
        let sampler = efude_canvas::CompositeSampler::new(&self.doc);
        let checker = self.display_checker();
        let image = region.image(|x, y| {
            let (raw, mask) = sample(layer, &self.selection, x, y);
            sampler.preview(&self.doc, x, y, checker, (layer.id, raw, mask))
        });
        self.studio.preview = Some((
            key,
            ctx.load_texture("edit-preview", image, display::CANVAS_TEXTURE),
            region,
        ));
    }
    pub(super) fn show_geometry_preview(
        &self,
        painter: &egui::Painter,
        to_screen: &impl Fn(Vec2) -> Pos2,
    ) {
        if let Some((_, tex, region)) = &self.studio.preview {
            let r = region.rect;
            painter.add(super::canvas_view::canvas_quad(
                tex.id(),
                [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ]
                .map(|p| to_screen(p.to_vec2())),
                Color32::WHITE,
            ));
        }
    }
}
