// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use super::*;

use super::shape_gradient::{HANDLES, handle, snap_angle};
pub(super) struct TransformSession {
    layer_id: u64,
    token: u64,
    bounds: [u32; 4],
    mesh: bool,
    offset: Vec2,
    drag: Option<(usize, Vec2, Vec2, f32, Vec2)>,
}
#[derive(Debug, Copy, Clone)]
struct Parameters {
    scale: Vec2,
    angle: f32,
    mesh: [[f32; 2]; 16],
}
impl TransformSession {
    fn handle_rect(&self) -> Rect {
        let [x, y, w, h] = self.bounds;
        let min_span = u32::from(self.mesh);
        // WarpMap uses pixel indices. The canvas displays their centers at i + 0.5.
        Rect::from_min_size(
            Pos2::new(x as f32 + 0.5, y as f32 + 0.5),
            Vec2::new((w - 1).max(min_span) as f32, (h - 1).max(min_span) as f32),
        )
    }
    fn affine(
        &self,
        layer: &efude_canvas::Layer,
        params: Parameters,
    ) -> Option<efude_canvas::AffineTransform> {
        efude_canvas::AffineTransform::validated(
            params.scale.into(),
            params.angle,
            self.offset.into(),
            layer,
        )
    }
    fn map(
        &self,
        layer: &efude_canvas::Layer,
        selected: bool,
        params: Parameters,
    ) -> Option<efude_canvas::WarpMap> {
        if self.mesh {
            efude_canvas::WarpMap::mesh_with_bounds(layer, self.bounds, [4, 4], &params.mesh)
        } else {
            efude_canvas::WarpMap::affine_with_bounds(
                layer,
                selected,
                self.bounds,
                self.affine(layer, params)?,
            )
        }
    }
    fn point(&self, uv: (f32, f32), params: Parameters) -> Vec2 {
        let v = (handle(self.handle_rect(), uv) - self.handle_rect().center()) * params.scale;
        self.handle_rect().center().to_vec2()
            + self.offset
            + egui::emath::Rot2::from_angle(params.angle) * v
    }
}
impl EfudeApp {
    fn transform_parameters(&self) -> Parameters {
        Parameters {
            scale: Vec2::new(self.transform_scale_x, self.transform_scale_y),
            angle: self.transform_angle.to_radians(),
            mesh: self.mesh_offsets,
        }
    }
    pub(super) fn start_transform(&mut self, mesh: bool) {
        if self.refuse_mask_edit() || self.refuse_alpha_edit() {
            return;
        }
        self.finish_pending_canvas_gesture();
        self.cancel_studio_session();
        let layer = &self.doc.layers[self.selected_layer];
        if layer.locked
            || self.is_reference_layer(self.selected_layer)
            || layer.kind == LayerKind::Folder
            || mesh && layer.is_vector()
        {
            return;
        }
        let bounds = if self.selection.active {
            let Some(b) = self.selection_rectangle() else {
                return;
            };
            b
        } else {
            [0, 0, self.doc.width, self.doc.height]
        };
        self.transform_session = Some(TransformSession {
            layer_id: layer.id,
            token: self.history.state_token(),
            bounds,
            mesh,
            drag: None,
            offset: Vec2::ZERO,
        });
        self.reset_transform_values();
        self.show_tool_pane();
    }
    pub(super) fn invalid_transform_status(&mut self) {
        self.status = self
            .text(
                "倍率は0にできません。有限の変形値を指定してください。",
                "Scale cannot be 0. Enter finite transform values.",
            )
            .into();
    }
    fn reset_transform_values(&mut self) {
        self.transform_angle = 0.;
        self.transform_scale_x = 1.;
        self.transform_scale_y = 1.;
        self.mesh_offsets = [[0.; 2]; 16];
        if let Some(session) = &mut self.transform_session {
            session.offset = Vec2::ZERO;
            session.drag = None;
        }
    }
    pub(super) fn commit_transform(&mut self) {
        let Some(session) = self.transform_session.take() else {
            return;
        };
        self.studio.preview = None;
        if self.history.state_token() != session.token
            || self.doc.layers[self.selected_layer].id != session.layer_id
        {
            return;
        }
        let params = self.transform_parameters();
        if self.refuse_mask_edit() || self.refuse_alpha_edit() {
            return;
        }
        let affine = session.affine(&self.doc.layers[self.selected_layer], params);
        if !session.mesh && affine.is_none() {
            self.invalid_transform_status();
            return;
        }
        self.history.begin();
        let layer = &mut self.doc.layers[self.selected_layer];
        let before = self.selection.mask.clone();
        let active = self.selection.active;
        let (w, h) = (self.doc.width, self.doc.height);
        if session.mesh {
            efude_canvas::mesh_warp_grid(
                layer,
                &mut self.selection,
                w,
                h,
                4,
                4,
                &params.mesh,
                &mut self.history,
            );
        } else if let Some(affine) = affine {
            let (scale, angle, offset) = affine.components();
            efude_canvas::transform_selection_offset(
                layer,
                &mut self.selection,
                w,
                h,
                scale[0],
                scale[1],
                angle,
                offset,
                &mut self.history,
            );
            if layer.is_vector() && (scale != [1.; 2] || angle != 0. || offset != [0.; 2]) {
                super::vector_tools::transform_lines(
                    layer,
                    (w, h),
                    &before,
                    active,
                    (scale[0], scale[1]),
                    angle,
                    offset.into(),
                    &mut self.history,
                );
            }
        }
        self.history.record_selection_change(
            active,
            before,
            self.selection.active,
            self.selection.mask.clone(),
        );
        self.history.commit_labeled(if session.mesh {
            self.text("メッシュ変形", "Mesh transform")
        } else {
            self.text("自由変形", "Free transform")
        });
        self.canvas_texture_dirty = true;
    }
    pub(super) fn transform_controls(&mut self, ui: &mut egui::Ui) {
        let en = self.language_english;
        let t = |ja, english| if en { english } else { ja };
        let session = self.transform_session.as_mut().unwrap();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut session.mesh, false, t("自由変形", "Free transform"));
            ui.add_enabled_ui(!self.doc.layers[self.selected_layer].is_vector(), |ui| {
                ui.selectable_value(&mut session.mesh, true, t("メッシュ", "Mesh"))
            });
        });
        if session.mesh {
            let limit = self.doc.width.max(self.doc.height) as f32 * 2.;
            for (i, offset) in self.mesh_offsets.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("({}, {})", i % 4 + 1, i / 4 + 1));
                    for (v, label) in offset.iter_mut().zip(["X ", "Y "]) {
                        ui.add(egui::DragValue::new(v).range(-limit..=limit).prefix(label));
                    }
                });
            }
        } else {
            for (v, label, limit) in [
                (&mut self.transform_scale_x, t("横倍率", "Scale X"), 16.),
                (&mut self.transform_scale_y, t("縦倍率", "Scale Y"), 16.),
                (&mut self.transform_angle, t("角度", "Angle"), 3600.),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.add(egui::DragValue::new(v).range(-limit..=limit).speed(0.01));
                });
            }
            ui.horizontal(|ui| {
                if ui.button(t("左右反転", "Flip X")).clicked() {
                    self.transform_scale_x *= -1.;
                }
                if ui.button(t("上下反転", "Flip Y")).clicked() {
                    self.transform_scale_y *= -1.;
                }
            });
        }
        if ui.button(t("リセット", "Reset")).clicked() {
            self.reset_transform_values();
        }
    }
    pub(super) fn preview_transform(
        &mut self,
        ctx: &egui::Context,
        region: super::shape_gradient::PreviewRegion,
        key: String,
    ) {
        if self.preview_cached(&key, region) {
            return;
        }
        let session = self.transform_session.as_ref().unwrap();
        let params = self.transform_parameters();
        let layer = &self.doc.layers[self.selected_layer];
        let map = session.map(layer, self.selection.active, params);
        let lines = layer
            .vector
            .as_ref()
            .filter(|_| map.is_some())
            .zip(session.affine(layer, params))
            .map(|(lines, affine)| {
                let (scale, angle, offset) = affine.components();
                let [w, h] = region.size.map(|v| v as u32);
                let mut preview = efude_canvas::Layer::new(0, "", w, h);
                let mut strokes = lines.clone();
                super::vector_tools::transform_line_geometry(
                    &mut strokes,
                    (self.doc.width, self.doc.height),
                    self.selection
                        .active
                        .then_some(self.selection.mask.as_slice()),
                    session.handle_rect().center().to_vec2(),
                    scale.into(),
                    angle,
                    offset.into(),
                );
                let ratio = Vec2::new(w as f32, h as f32) / region.rect.size();
                efude_canvas::vector::transform_strokes(
                    &mut strokes,
                    |x, y| {
                        (
                            (x - region.rect.min.x) * ratio.x,
                            (y - region.rect.min.y) * ratio.y,
                        )
                    },
                    (ratio.x * ratio.y).sqrt(),
                );
                preview.vector = Some(strokes);
                efude_canvas::vector::render_all(&mut preview, w, h, None);
                preview.pixels
            });
        self.cache_geometry_preview(ctx, key, region, |layer, selection, x, y| {
            let (pixel, mask) = map.as_ref().map_or_else(
                || {
                    (
                        layer.pixels.pixel(x, y),
                        layer
                            .mask
                            .as_ref()
                            .map_or(255, |m| m.pixel_or_tile_default(x, y, [255; 4])[0]),
                    )
                },
                |m| m.preview_pixel(layer, selection, x, y),
            );
            let pixel = lines.as_ref().map_or(pixel, |lines| {
                let local = (Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - region.rect.min.to_vec2())
                    / region.rect.size();
                lines.pixel(
                    (local.x * region.size[0] as f32) as u32,
                    (local.y * region.size[1] as f32) as u32,
                )
            });
            (pixel, mask)
        });
    }
    pub(super) fn transform_canvas(
        &mut self,
        response: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
        scale: f32,
        to_screen: &impl Fn(Vec2) -> Pos2,
    ) {
        let primary = egui::PointerButton::Primary;
        let ctx = &response.ctx;
        let params = self.transform_parameters();
        let Some(session) = self.transform_session.as_ref() else {
            return;
        };
        if self.history.state_token() != session.token
            || self.doc.layers[self.selected_layer].id != session.layer_id
        {
            self.cancel_studio_session();
            return;
        }
        let key = format!("{params:?} {:?} {}", session.offset, session.mesh);
        if let Some(region) = self.preview_region(
            rect,
            painter.clip_rect(),
            scale,
            ctx.pixels_per_point(),
            self.max_texture_side(ctx).min(2048),
        ) {
            self.preview_transform(ctx, region, key);
            self.show_geometry_preview(painter, to_screen);
        }
        let p = response
            .interact_pointer_pos()
            .map(|pos| self.document_position_unsnapped(pos, rect, scale));
        let start = ctx
            .input(|i| i.pointer.press_origin())
            .map(|pos| self.document_position_unsnapped(pos, rect, scale));
        let session = self.transform_session.as_mut().unwrap();
        let mut points = Vec::new();
        let stroke = Stroke::new(1.2, Color32::WHITE);
        let center = session.handle_rect().center().to_vec2() + session.offset;
        if session.mesh {
            for i in 0..16 {
                let q = handle(
                    session.handle_rect(),
                    ((i % 4) as f32 / 3., (i / 4) as f32 / 3.),
                )
                .to_vec2()
                    + Vec2::from(self.mesh_offsets[i]);
                points.push(q);
                if i >= 4 {
                    painter.line_segment([to_screen(points[i - 4]), to_screen(q)], stroke);
                }
                if i % 4 != 0 {
                    painter.line_segment([to_screen(points[i - 1]), to_screen(q)], stroke);
                }
            }
        } else {
            points.extend(HANDLES.map(|uv| session.point(uv, params)));
            painter.add(egui::Shape::closed_line(
                points.iter().copied().map(to_screen).collect(),
                stroke,
            ));
            let pole = points[1] + (points[1] - center).normalized() * 24. / scale;
            painter.line_segment([to_screen(points[1]), to_screen(pole)], stroke);
            points.push(pole);
        }
        for &point in &points {
            painter.circle(to_screen(point), 4., crate::layout::ACCENT, stroke);
        }
        if response.drag_started_by(primary)
            && let Some(start) = start
        {
            let grip = points
                .iter()
                .enumerate()
                .map(|(i, q)| (i, (*q - start).length() * scale))
                .filter(|(_, distance)| *distance < 9.)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .or_else(|| {
                    let local = egui::emath::Rot2::from_angle(-params.angle) * (start - center);
                    (!session.mesh
                        && local.x.abs()
                            <= session.handle_rect().width() * params.scale.x.abs() / 2.
                        && local.y.abs()
                            <= session.handle_rect().height() * params.scale.y.abs() / 2.)
                        .then_some(9)
                });
            session.drag = grip.map(|i| {
                (
                    i,
                    start,
                    params.scale,
                    params.angle,
                    if session.mesh {
                        Vec2::from(self.mesh_offsets[i])
                    } else {
                        session.offset
                    },
                )
            });
        }
        if let (Some((grip, start, base_scale, base_angle, offset)), Some(p)) = (session.drag, p) {
            let delta = p - start;
            let shift = ctx.input(|i| i.modifiers.shift);
            if session.mesh {
                let limit = Vec2::splat(2. * self.doc.width.max(self.doc.height) as f32);
                self.mesh_offsets[grip] = (offset + delta).clamp(-limit, limit).into();
            } else if grip == 9 {
                session.offset = offset + delta;
            } else if grip == 8 {
                let angle = base_angle + (p - center).angle() - (start - center).angle();
                self.transform_angle = if shift { snap_angle(angle) } else { angle }.to_degrees();
            } else {
                let rotation = egui::emath::Rot2::from_angle(-base_angle);
                let initial = rotation * (start - center);
                let next = rotation * (p - center);
                let (x, y) = HANDLES[grip];
                let mut ratio = Vec2::splat(1.);
                if x != 0.5 && initial.x.abs() > 0.001 {
                    ratio.x = next.x / initial.x;
                }
                if y != 0.5 && initial.y.abs() > 0.001 {
                    ratio.y = next.y / initial.y;
                }
                if shift && x != 0.5 && y != 0.5 {
                    ratio = Vec2::splat(if (ratio.x - 1.).abs() > (ratio.y - 1.).abs() {
                        ratio.x
                    } else {
                        ratio.y
                    });
                }
                self.transform_scale_x = (base_scale.x * ratio.x).clamp(-16., 16.);
                self.transform_scale_y = (base_scale.y * ratio.y).clamp(-16., 16.);
            }
            ctx.request_repaint();
        }
        if response.drag_stopped_by(primary) {
            session.drag = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage7_session_maps_reuse_full_selection_bounds() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(30_000, 3000);
        app.doc.layers[0].pixels.set_pixel(10, 10, [255; 4]);
        app.selection = Selection {
            active: true,
            mask: vec![255; 90_000_000],
        };
        for mesh in [false, true] {
            app.start_transform(mesh);
            app.transform_scale_x = -1.;
            app.mesh_offsets = [[2., 0.]; 16];
            let session = app.transform_session.as_ref().unwrap();
            let layer = &app.doc.layers[0];
            let params = app.transform_parameters();
            let reference = if mesh {
                efude_canvas::WarpMap::mesh(layer, &app.selection, [4, 4], &params.mesh)
            } else {
                efude_canvas::WarpMap::affine(
                    layer,
                    &app.selection,
                    params.scale.into(),
                    params.angle,
                    [0.; 2],
                )
            }
            .unwrap();
            let map = session.map(layer, app.selection.active, params).unwrap();
            for x in [12, 29_989] {
                assert_eq!(
                    map.preview_pixel(layer, &app.selection, x, 10),
                    reference.preview_pixel(layer, &app.selection, x, 10)
                );
            }
            let start = std::time::Instant::now();
            for _ in 0..32 {
                std::hint::black_box(session.map(layer, app.selection.active, params).unwrap());
            }
            let per_map = start.elapsed() / 32;
            eprintln!("30,000 x 3,000 selection, mesh={mesh}: {per_map:?} per map");
            assert!(per_map < std::time::Duration::from_millis(3));
        }
    }
}
