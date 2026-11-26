// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use super::*;
pub(super) fn swatch_color([r, g, b, a]: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}
impl EfudeApp {
    pub(crate) fn color_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let color_before = self.color;
        self.color_ui_contents(ui, ctx);
        // Picking any colour leaves the transparent colour.
        if self.color != color_before {
            self.transparent_color = false;
        }
    }

    /// The "transparent colour" switch: a checkerboard swatch. While it is
    /// on, brushes and fills erase.
    fn transparent_color_button(&mut self, ui: &mut egui::Ui) {
        let label = self.text("透明色", "Transparent");
        let hint = self.text(
            "透明色で描く：ブラシや塗りつぶしが、その形のまま消します。色を選ぶと戻ります",
            "Paint with transparency: brushes and fills erase in their own shape. Picking a colour turns it off",
        );
        ui.horizontal(|ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(40.0, 20.0), egui::Sense::click());
            let painter = ui.painter();
            let cell = 5.0;
            for row in 0..4 {
                for column in 0..8 {
                    let grey = if (row + column) % 2 == 0 { 255 } else { 190 };
                    painter.rect_filled(
                        Rect::from_min_size(
                            rect.min + Vec2::new(column as f32 * cell, row as f32 * cell),
                            Vec2::splat(cell),
                        ),
                        0.0,
                        Color32::from_gray(grey),
                    );
                }
            }
            let stroke = if self.transparent_color {
                Stroke::new(2.5, crate::layout::ACCENT)
            } else {
                Stroke::new(1.0, Color32::from_gray(90))
            };
            painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Outside);
            let clicked = response.on_hover_text(hint).clicked();
            let toggled = ui
                .selectable_label(self.transparent_color, label)
                .on_hover_text(hint)
                .clicked();
            if clicked || toggled {
                self.transparent_color = !self.transparent_color;
            }
        });
    }

    fn color_ui_contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.picker_ui(ui, ctx);
        ui.add(
            egui::Slider::new(&mut self.intermediate_mix, 0.0..=1.0).text(
                if self.language_english {
                    "Intermediate Color Ratio"
                } else {
                    "中間色の割合"
                },
            ),
        );
        let blend_channel =
            |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * self.intermediate_mix).round() as u8;
        let intermediate =
            std::array::from_fn(|i| blend_channel(self.color[i], self.secondary_color[i]));
        ui.horizontal(|ui| {
            ui.label(self.text("中間色", "Intermediate Color"));
            let response = ui.add(
                egui::Button::new(if self.language_english {
                    "Set as Current Color"
                } else {
                    "現在色に設定"
                })
                .fill(swatch_color(intermediate))
                .min_size(Vec2::new(96.0, 24.0)),
            );
            if response.clicked() {
                self.color = intermediate;
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut chosen = None;
            let mut removed = None;
            for (i, rgba) in self.palette.iter().enumerate() {
                let swatch = Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
                let response = ui
                    .add(
                        egui::Button::new("")
                            .fill(swatch)
                            .min_size(Vec2::splat(24.)),
                    )
                    .on_hover_text(self.text(
                        "クリックで選択 / 右クリックで削除",
                        "Click to use, right-click to remove",
                    ));
                if response.clicked() {
                    chosen = Some(i);
                }
                if response.secondary_clicked() {
                    removed = Some(i);
                }
            }
            if let Some(i) = chosen {
                self.color = self.palette[i];
            }
            if let Some(i) = removed {
                self.palette.remove(i);
            }
        });
        if ui
            .button(self.text("現在色をパレットに追加", "Add Current Color to Palette"))
            .clicked()
        {
            self.palette.push(self.color);
            if self.palette.len() > 32 {
                self.palette.remove(0);
            }
        }
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                self.text("パレット", "Palette"),
                self.palette.len()
            ));
            if ui
                .button(self.text("すべて消去", "Clear Palette"))
                .clicked()
            {
                self.palette.clear();
            }
        });
    }
}

pub(super) fn parse_hex(input: &str) -> Option<[u8; 3]> {
    let raw = input.trim().strip_prefix('#').unwrap_or(input.trim());
    if raw.len() != 6 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(raw, 16).ok()?.to_be_bytes();
    Some([rgb[1], rgb[2], rgb[3]])
}
impl EfudeApp {
    pub(super) fn swap_colors(&mut self) {
        std::mem::swap(&mut self.color, &mut self.secondary_color);
        self.transparent_color = false;
    }
    pub(super) fn reset_colors(&mut self) {
        self.color = [0, 0, 0, 255];
        self.secondary_color = [255; 4];
        self.transparent_color = false;
    }
    pub(super) fn remember_color(&mut self) {
        if self.transparent_color || self.color[3] == 0 {
            return;
        }
        self.recent_colors.retain(|c| *c != self.color);
        self.recent_colors.insert(0, self.color);
        self.recent_colors.truncate(16);
    }
    pub(super) fn commit_brush_history(&mut self) {
        let label = match self.tool {
            Tool::Blur => self.text("ぼかし", "Blur"),
            Tool::Smudge => self.text("指先", "Smudge"),
            Tool::Eraser => self.text("消しゴム", "Erase"),
            _ if self.transparent_color => self.text("消しゴム", "Erase"),
            _ => {
                self.commit_paint_history("描画", "Paint");
                return;
            }
        };
        self.history.commit_labeled(label);
    }
    pub(super) fn commit_paint_history(&mut self, ja: &'static str, en: &'static str) {
        let revision = self.history.content_revision();
        self.history.commit_labeled(self.text(ja, en));
        if revision != self.history.content_revision() {
            self.remember_color();
        }
    }
    pub(super) fn set_picker_hsv(&mut self, h: f32, s: f32, v: f32) {
        self.hue_cache = h;
        self.saturation_cache = s;
        self.color[..3].copy_from_slice(&egui::ecolor::Hsva::new(h, s, v, 1.).to_srgb());
        self.transparent_color = false;
    }
    fn picker_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        use egui::ecolor::Hsva;
        let mut hsv = Hsva::from_srgba_unmultiplied(self.color);
        if hsv.s > 0. && hsv.v > 0. {
            self.hue_cache = hsv.h;
        }
        if hsv.v > 0. {
            self.saturation_cache = hsv.s;
        } else {
            hsv.s = self.saturation_cache;
        }
        hsv.h = self.hue_cache;
        let side = 176usize;
        let wheel = self
            .color_wheel
            .get_or_insert_with(|| {
                let pixels = (0..side * side)
                    .map(|i| {
                        let p = Vec2::new((i % side) as f32 + 0.5, (i / side) as f32 + 0.5)
                            / (side as f32 * 0.5)
                            - Vec2::splat(1.);
                        if (0.84..=1.).contains(&p.length()) {
                            Hsva::new(
                                (p.angle() / std::f32::consts::TAU).rem_euclid(1.),
                                1.,
                                1.,
                                1.,
                            )
                            .into()
                        } else {
                            Color32::TRANSPARENT
                        }
                    })
                    .collect();
                ctx.load_texture(
                    "hue-ring",
                    egui::ColorImage {
                        size: [side; 2],
                        pixels,
                    },
                    egui::TextureOptions::LINEAR,
                )
            })
            .id();
        if self.sv_texture.as_ref().is_none_or(|(h, _)| *h != hsv.h) {
            let pixels = (0..96 * 96)
                .map(|i| {
                    Hsva::new(hsv.h, (i % 96) as f32 / 95., 1. - (i / 96) as f32 / 95., 1.).into()
                })
                .collect();
            self.sv_texture = Some((
                hsv.h,
                ctx.load_texture(
                    "sv-square",
                    egui::ColorImage {
                        size: [96; 2],
                        pixels,
                    },
                    egui::TextureOptions::LINEAR,
                ),
            ));
        }
        let size = ui.available_width().min(176.);
        let (ring, response) =
            ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click_and_drag());
        let square = Rect::from_center_size(ring.center(), Vec2::splat(size * 0.57));
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
        ui.painter().image(wheel, ring, uv, Color32::WHITE);
        if let Some((_, tex)) = &self.sv_texture {
            ui.painter().image(tex.id(), square, uv, Color32::WHITE);
        }
        for pos in [
            ring.center() + Vec2::angled(hsv.h * std::f32::consts::TAU) * size * 0.46,
            square.min + Vec2::new(hsv.s, 1. - hsv.v) * square.size(),
        ] {
            ui.painter()
                .circle_stroke(pos, 4., Stroke::new(3., Color32::BLACK));
            ui.painter()
                .circle_stroke(pos, 4., Stroke::new(1., Color32::WHITE));
        }
        if (response.dragged() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if square.contains(origin) {
                self.set_picker_hsv(
                    hsv.h,
                    ((p.x - square.left()) / square.width()).clamp(0., 1.),
                    (1. - (p.y - square.top()) / square.height()).clamp(0., 1.),
                );
            } else if (0.42..=0.5).contains(&(origin.distance(ring.center()) / size)) {
                self.set_picker_hsv(
                    ((p - ring.center()).angle() / std::f32::consts::TAU).rem_euclid(1.),
                    hsv.s,
                    hsv.v,
                );
            }
        }
        ui.horizontal(|ui| {
            let (area, _) = ui.allocate_exact_size(Vec2::new(48., 35.), egui::Sense::hover());
            let labels = if self.language_english {
                ["Background", "Foreground"]
            } else {
                ["背景色", "描画色"]
            };
            for (n, (pos, color)) in [
                (area.min + Vec2::new(15., 10.), &mut self.secondary_color),
                (area.min, &mut self.color),
            ]
            .into_iter()
            .enumerate()
            {
                let mut swatch = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("dual-swatch", n))
                        .max_rect(Rect::from_min_size(pos, Vec2::new(30., 24.))),
                );
                let (rect, response) =
                    swatch.allocate_exact_size(Vec2::new(30., 24.), egui::Sense::click());
                egui::color_picker::show_color_at(swatch.painter(), swatch_color(*color), rect);
                swatch.painter().rect_stroke(
                    rect,
                    2.,
                    swatch.style().interact(&response).bg_stroke,
                    egui::StrokeKind::Inside,
                );
                let response = response.on_hover_text(labels[n]);
                let popup = response.id.with("picker");
                if response.clicked() {
                    swatch.memory_mut(|m| m.toggle_popup(popup));
                }
                egui::popup_below_widget(
                    &swatch,
                    popup,
                    &response,
                    egui::PopupCloseBehavior::CloseOnClickOutside,
                    |ui| {
                        ui.spacing_mut().slider_width = 200.;
                        let mut rgb = Color32::from_rgb(color[0], color[1], color[2]);
                        if egui::color_picker::color_picker_color32(
                            ui,
                            &mut rgb,
                            egui::color_picker::Alpha::Opaque,
                        ) {
                            color[..3].copy_from_slice(&rgb.to_array()[..3]);
                        }
                        ui.add(
                            egui::Slider::new(&mut color[3], 0..=255)
                                .show_value(false)
                                .text(if self.language_english {
                                    "Alpha"
                                } else {
                                    "不透明度"
                                }),
                        );
                    },
                );
            }
            if ui
                .button("⇄")
                .on_hover_text(self.text("色を交換 (X)", "Swap colors (X)"))
                .clicked()
            {
                self.swap_colors();
            }
            if ui
                .button("D")
                .on_hover_text(self.text("黒と白に戻す", "Reset to black and white"))
                .clicked()
            {
                self.reset_colors();
            }
        });
        self.transparent_color_button(ui);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.hsv_sliders, false, "RGB");
            ui.selectable_value(&mut self.hsv_sliders, true, "HSV");
        });
        let mut rgba = self.color;
        let initial = rgba;
        if self.hsv_sliders {
            let mut changed = false;
            for (v, label) in [(&mut hsv.h, "H"), (&mut hsv.s, "S"), (&mut hsv.v, "V")] {
                changed |= ui
                    .add(egui::Slider::new(v, 0.0..=1.0).text(label))
                    .changed();
            }
            if changed {
                self.set_picker_hsv(hsv.h, hsv.s, hsv.v);
                rgba = self.color;
            }
        } else {
            for (v, label) in rgba[..3].iter_mut().zip(["R", "G", "B"]) {
                ui.add(egui::Slider::new(v, 0..=255).text(label));
            }
        }
        ui.add(egui::Slider::new(&mut rgba[3], 0..=255).text(self.text("不透明度", "Alpha")));
        if rgba != initial {
            self.color = rgba;
        }
        let id = ui.make_persistent_id("color-hex");
        if !self.hex_focused {
            self.hex_edit = format!("#{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2]);
        }
        let edit = ui.add(
            egui::TextEdit::singleline(&mut self.hex_edit)
                .id(id)
                .desired_width(90.),
        );
        self.hex_focused = edit.has_focus();
        if edit.lost_focus() {
            if let Some([r, g, b]) = parse_hex(&self.hex_edit) {
                self.color = [r, g, b, rgba[3]];
                self.transparent_color = false;
            }
            let [r, g, b, _] = self.color;
            self.hex_edit = format!("#{r:02X}{g:02X}{b:02X}");
        }
        ui.horizontal_wrapped(|ui| {
            for &color in &self.recent_colors {
                if ui
                    .add(
                        egui::Button::new("")
                            .min_size(Vec2::splat(18.))
                            .fill(swatch_color(color)),
                    )
                    .clicked()
                {
                    self.color = color;
                    self.transparent_color = false;
                }
            }
        });
    }
}
