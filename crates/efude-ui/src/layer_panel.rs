// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use super::*;
#[derive(Clone, Copy)]
enum LayerAction {
    NewRaster,
    NewVector,
    NewFolder,
    Duplicate,
    MergeDown,
    MergeFolder,
    MergeVisible,
    CopyMerged,
    AlphaLock,
    Tint,
    Clip,
    SelectAlpha,
    Rasterize,
    Mask,
    Delete,
    Clear,
}
impl LayerAction {
    fn button(self, ui: &mut egui::Ui, enabled: bool, english: bool) -> bool {
        let (ja, en, key) = match self {
            Self::NewRaster => ("新規ラスター", "New raster", "Ctrl+Shift+N"),
            Self::NewVector => ("新規ベクター", "New vector", ""),
            Self::NewFolder => ("新規フォルダー", "New folder", ""),
            Self::Duplicate => ("複製", "Duplicate", "Ctrl+J"),
            Self::MergeDown => ("下のレイヤーと結合", "Merge down", "Ctrl+E"),
            Self::MergeFolder => ("フォルダーを結合", "Merge folder", ""),
            Self::MergeVisible => ("表示レイヤーを統合", "Merge visible", ""),
            Self::CopyMerged => ("表示レイヤーをコピー統合", "Copy merged to new layer", ""),
            Self::AlphaLock => ("透明ピクセルをロック", "Lock transparent pixels", ""),
            Self::Tint => ("レイヤーカラー", "Layer color", ""),
            Self::Clip => ("クリッピング", "Clipping", ""),
            Self::SelectAlpha => ("不透明度から選択範囲を作成", "Select from opacity", ""),
            Self::Rasterize => ("ラスタライズ", "Rasterize", ""),
            Self::Mask => ("マスクを追加／削除", "Add / remove mask", ""),
            Self::Delete => ("削除", "Delete", ""),
            Self::Clear => ("レイヤーをすべて削除…", "Delete all layers…", ""),
        };
        ui.add_enabled(
            enabled,
            egui::Button::new(if english { en } else { ja }).shortcut_text(key),
        )
        .clicked()
    }
}
impl EfudeApp {
    pub(crate) fn layers_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let owned = self.editable_object_layers();
        let english = self.language_english;
        self.layer_controls(ui);
        egui::ScrollArea::vertical()
            .id_salt("layer-list-and-details")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(self.text("表示レイヤーを統合", "Merge Visible Layers"))
                        .clicked()
                    {
                        self.merge_visible_layers();
                    }
                    if ui
                        .button(self.text("＋ラスター", "+ Raster Layer"))
                        .on_hover_text("Ctrl+Shift+N")
                        .clicked()
                    {
                        self.add_raster_layer();
                    }
                    if ui
                        .button(self.text("＋ベクター", "+ Vector Layer"))
                        .on_hover_text(self.text(
                            "線をあとから消したり動かしたりできるレイヤー",
                            "A layer whose lines can be erased and moved later",
                        ))
                        .clicked()
                    {
                        self.add_vector_layer();
                    }
                    if ui
                        .button(self.text("＋子レイヤー", "+ Child Layer"))
                        .clicked()
                        && {
                            self.finish_pending_canvas_gesture();
                            self.can_add_layers(1)
                        }
                    {
                        let parent =
                            if self.doc.layers[self.selected_layer].kind == LayerKind::Folder {
                                Some(self.doc.layers[self.selected_layer].id)
                            } else {
                                self.doc.layers[self.selected_layer].parent_id
                            };
                        let id = self.next_layer_id();
                        let mut child = efude_canvas::Layer::new(
                            id,
                            if english {
                                format!("Layer {id}")
                            } else {
                                format!("レイヤー {id}")
                            },
                            self.doc.width,
                            self.doc.height,
                        );
                        child.parent_id = parent;
                        let at = self.doc.layers.len();
                        self.history.insert_layer(&mut self.doc.layers, at, child);
                        self.selected_layer = at;
                    }
                    if ui.button(self.text("＋フォルダ", "+ Folder")).clicked() {
                        self.add_folder();
                    }
                    if ui
                        .button(self.text("複製", "Duplicate"))
                        .on_hover_text("Ctrl+J")
                        .clicked()
                    {
                        self.duplicate_layer_subtree();
                    }
                    if ui.button(self.text("削除", "Delete")).clicked() && self.doc.layers.len() > 1
                    {
                        self.delete_document_layer(self.selected_layer);
                    }
                });
                let layer_count = self.doc.layers.len();
                // Dragging a row: where it would land (layer id and placement).
                let dragging = self.layer_drag;
                let pointer = ui.ctx().pointer_latest_pos();
                let mut drop_target: Option<(u64, efude_canvas::LayerPlacement)> = None;
                let dragged_subtree = dragging
                    .map(|id| efude_canvas::subtree_ids(&self.doc.layers, id))
                    .unwrap_or_default();
                let mut rasterize = None;
                self.layer_rows.clear();
                let mut context_action = None;
                for row_index in (0..layer_count)
                    .rev()
                    .map(Some)
                    .chain(std::iter::once(None))
                {
                    let details = row_index.is_none();
                    let i = row_index.unwrap_or(self.selected_layer);
                    let mut parent = self.doc.layers[i].parent_id;
                    let mut depth = 0usize;
                    let mut hidden = false;
                    let mut traversed = 0;
                    while let Some(pid) = parent {
                        traversed += 1;
                        if traversed > self.doc.layers.len() {
                            break;
                        }
                        if let Some((pi, p)) = self
                            .doc
                            .layers
                            .iter()
                            .enumerate()
                            .find(|(_, l)| l.id == pid)
                        {
                            depth += 1;
                            if !p.expanded {
                                hidden = true;
                            }
                            parent = p.parent_id;
                            if pi == i {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    if hidden {
                        continue;
                    }
                    if !details
                        && !ui.is_rect_visible(Rect::from_min_size(
                            ui.next_widget_position(),
                            Vec2::new(ui.available_width(), 42.),
                        ))
                    {
                        let (row, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 42.),
                            egui::Sense::hover(),
                        );
                        self.layer_rows.push((self.doc.layers[i].id, row));
                        continue;
                    }
                    let has_children = self
                        .doc
                        .layers
                        .iter()
                        .any(|child| child.parent_id == Some(self.doc.layers[i].id));
                    let layer_id = self.doc.layers[i].id;
                    let property_before = self.doc.layers[i].property_state();
                    let macro_before = if self.macro_recording.is_some() {
                        let layer = &self.doc.layers[i];
                        Some((
                            layer.name.clone(),
                            layer.opacity,
                            layer.visible,
                            layer.blend,
                        ))
                    } else {
                        None
                    };
                    let parent_id = self.doc.layers[i].parent_id;
                    let mut descendants = vec![layer_id];
                    let mut descendant_cursor = 0;
                    while descendant_cursor < descendants.len() {
                        let ancestor = descendants[descendant_cursor];
                        let children = self
                            .doc
                            .layers
                            .iter()
                            .filter(|candidate| candidate.parent_id == Some(ancestor))
                            .map(|candidate| candidate.id)
                            .filter(|child| !descendants.contains(child))
                            .collect::<Vec<_>>();
                        descendants.extend(children);
                        descendant_cursor += 1;
                    }
                    let parent_name = parent_id
                        .and_then(|id| {
                            self.doc
                                .layers
                                .iter()
                                .find(|layer| layer.id == id)
                                .map(|layer| layer.name.clone())
                        })
                        .unwrap_or_else(|| {
                            if english {
                                "Top Level".into()
                            } else {
                                "最上位".into()
                            }
                        });
                    let folder_options = self
                        .doc
                        .layers
                        .iter()
                        .filter(|candidate| {
                            candidate.kind == LayerKind::Folder
                                && !descendants.contains(&candidate.id)
                        })
                        .map(|candidate| (candidate.id, candidate.name.clone()))
                        .collect::<Vec<_>>();
                    let siblings = self
                        .doc
                        .layers
                        .iter()
                        .enumerate()
                        .filter_map(|(index, layer)| {
                            (layer.parent_id == parent_id).then_some(index)
                        })
                        .collect::<Vec<_>>();
                    let move_up_target = siblings.iter().copied().find(|index| *index > i);
                    let move_down_target = siblings.iter().copied().rev().find(|index| *index < i);
                    let mut move_to = None;
                    let mut select_layer = false;
                    let mut thumbnail_target = None;
                    let mut change_mask = None;
                    let mut mask_action = None;
                    let mut change_parent = None;
                    {
                        let thumbnails = if details {
                            None
                        } else {
                            Some(self.thumbnail_ids(i, ctx))
                        };
                        let rasterizable = self.doc.layers[i].is_vector()
                            || owned.contains(&self.doc.layers[i].id);
                        let l = &mut self.doc.layers[i];
                        let selected = i == self.selected_layer;
                        if !details {
                            let (row, response) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 42.0),
                                egui::Sense::click_and_drag(),
                            );
                            if response.drag_started() {
                                self.layer_drag = Some(layer_id);
                            }
                            self.layer_rows.push((layer_id, row));
                            let painter = ui.painter().clone();
                            if selected {
                                painter.rect_filled(
                                    row,
                                    7.0,
                                    crate::layout::ACCENT.gamma_multiply(0.30),
                                );
                                painter.rect_stroke(
                                    row,
                                    7.0,
                                    Stroke::new(1.2, crate::layout::ACCENT),
                                    egui::StrokeKind::Inside,
                                );
                            } else if response.hovered() {
                                painter.rect_filled(row, 7.0, Color32::from_rgb(40, 41, 50));
                            }
                            let text_color = if l.visible {
                                Color32::from_rgb(225, 225, 235)
                            } else {
                                Color32::from_gray(150)
                            };
                            let mut x = row.left() + 6.0 + depth as f32 * 14.0;
                            let mut clicked_control = false;
                            // Folder expander.
                            if l.kind == LayerKind::Folder && has_children {
                                let r = Rect::from_center_size(
                                    Pos2::new(x + 7.0, row.center().y),
                                    Vec2::splat(14.0),
                                );
                                if ui
                                    .interact(
                                        r,
                                        ui.id().with(("layer-expand", layer_id)),
                                        egui::Sense::click(),
                                    )
                                    .clicked()
                                {
                                    l.expanded = !l.expanded;
                                    clicked_control = true;
                                }
                                let c = r.center();
                                let points = if l.expanded {
                                    vec![
                                        c + Vec2::new(-4.0, -2.0),
                                        c + Vec2::new(4.0, -2.0),
                                        c + Vec2::new(0.0, 3.0),
                                    ]
                                } else {
                                    vec![
                                        c + Vec2::new(-2.0, -4.0),
                                        c + Vec2::new(3.0, 0.0),
                                        c + Vec2::new(-2.0, 4.0),
                                    ]
                                };
                                painter.add(egui::Shape::convex_polygon(
                                    points,
                                    text_color,
                                    Stroke::NONE,
                                ));
                            }
                            x += 16.0;
                            // Visibility (eye).
                            let eye = Rect::from_center_size(
                                Pos2::new(x + 9.0, row.center().y),
                                Vec2::new(18.0, 14.0),
                            );
                            let eye_response = ui
                                .interact(
                                    eye,
                                    ui.id().with(("layer-eye", layer_id)),
                                    egui::Sense::click(),
                                )
                                .on_hover_text(if english {
                                    "Show / hide"
                                } else {
                                    "表示 / 非表示"
                                });
                            if eye_response.clicked() {
                                l.visible = !l.visible;
                                clicked_control = true;
                            }
                            let eye_color = if l.visible {
                                Color32::from_rgb(210, 210, 222)
                            } else {
                                Color32::from_gray(130)
                            };
                            let lens: Vec<Pos2> = (0..=16)
                                .map(|k| {
                                    let a = k as f32 / 16.0 * std::f32::consts::TAU;
                                    eye.center() + Vec2::new(8.0 * a.cos(), 4.5 * a.sin())
                                })
                                .collect();
                            painter
                                .add(egui::Shape::closed_line(lens, Stroke::new(1.3, eye_color)));
                            if l.visible {
                                painter.circle_filled(eye.center(), 2.4, eye_color);
                            } else {
                                painter.line_segment(
                                    [eye.left_bottom(), eye.right_top()],
                                    Stroke::new(1.3, eye_color),
                                );
                            }
                            x += 26.0;
                            if let Some((body, mask)) = thumbnails {
                                for (tex, is_mask) in
                                    std::iter::once((body, false)).chain(mask.map(|id| (id, true)))
                                {
                                    let r = Rect::from_min_size(
                                        Pos2::new(x, row.center().y - 17.),
                                        Vec2::splat(34.),
                                    );
                                    painter.image(
                                        tex,
                                        r,
                                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                                        Color32::WHITE,
                                    );
                                    if selected && self.editing_mask == is_mask {
                                        painter.rect_stroke(
                                            r,
                                            2.,
                                            Stroke::new(1.5, crate::layout::ACCENT),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    if response.clicked()
                                        && response
                                            .interact_pointer_pos()
                                            .is_some_and(|p| r.contains(p))
                                    {
                                        select_layer = true;
                                        thumbnail_target = Some(is_mask);
                                        clicked_control = true;
                                    }
                                    x += 37.;
                                }
                            }
                            // Folder marker, name, and move buttons on the right.
                            if l.kind == LayerKind::Folder {
                                let f = Rect::from_min_size(
                                    Pos2::new(x, row.center().y - 6.0),
                                    Vec2::new(16.0, 12.0),
                                );
                                painter.rect_stroke(
                                    f,
                                    2.0,
                                    Stroke::new(1.2, text_color),
                                    egui::StrokeKind::Inside,
                                );
                                painter.line_segment(
                                    [
                                        f.left_top() + Vec2::new(0.0, 3.0),
                                        f.right_top() + Vec2::new(0.0, 3.0),
                                    ],
                                    Stroke::new(1.0, text_color),
                                );
                                x += 22.0;
                            } else if l.is_vector() {
                                // A curve between two anchor points.
                                let c = Pos2::new(x + 8.0, row.center().y);
                                let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                                    [
                                        c + Vec2::new(-7.0, 5.0),
                                        c + Vec2::new(-3.0, -9.0),
                                        c + Vec2::new(3.0, 9.0),
                                        c + Vec2::new(7.0, -5.0),
                                    ],
                                    false,
                                    Color32::TRANSPARENT,
                                    Stroke::new(1.3, text_color),
                                );
                                painter.add(curve);
                                for end in [c + Vec2::new(-7.0, 5.0), c + Vec2::new(7.0, -5.0)] {
                                    painter.rect_filled(
                                        Rect::from_center_size(end, Vec2::splat(3.5)),
                                        0.0,
                                        text_color,
                                    );
                                }
                                x += 22.0;
                            }
                            painter.text(
                                Pos2::new(x, row.center().y),
                                egui::Align2::LEFT_CENTER,
                                format!(
                                    "{}{}{}{}{}{}",
                                    l.name,
                                    if l.blend == BlendMode::Normal {
                                        String::new()
                                    } else {
                                        format!(" · {}", blend_mode_name(l.blend, english))
                                    },
                                    if l.opacity < 1. {
                                        format!(" {:.0}%", l.opacity * 100.)
                                    } else {
                                        String::new()
                                    },
                                    if l.alpha_lock { " α" } else { "" },
                                    if l.locked { " 🔒" } else { "" },
                                    if l.clipping { " ↳" } else { "" }
                                ),
                                egui::FontId::proportional(14.0),
                                text_color,
                            );
                            for (offset, up) in [(46.0, true), (24.0, false)] {
                                let r = Rect::from_center_size(
                                    Pos2::new(row.right() - offset + 10.0, row.center().y),
                                    Vec2::splat(18.0),
                                );
                                let arrow = ui
                                    .interact(
                                        r,
                                        ui.id().with(("layer-move", layer_id, up)),
                                        egui::Sense::click(),
                                    )
                                    .on_hover_text(if up {
                                        if english { "Move up" } else { "上へ" }
                                    } else if english {
                                        "Move down"
                                    } else {
                                        "下へ"
                                    });
                                if arrow.hovered() {
                                    painter.rect_filled(r, 4.0, Color32::from_rgb(56, 57, 68));
                                }
                                let c = r.center();
                                let points = if up {
                                    vec![
                                        c + Vec2::new(-4.5, 2.5),
                                        c + Vec2::new(4.5, 2.5),
                                        c + Vec2::new(0.0, -3.5),
                                    ]
                                } else {
                                    vec![
                                        c + Vec2::new(-4.5, -2.5),
                                        c + Vec2::new(4.5, -2.5),
                                        c + Vec2::new(0.0, 3.5),
                                    ]
                                };
                                painter.add(egui::Shape::convex_polygon(
                                    points,
                                    Color32::from_gray(170),
                                    Stroke::NONE,
                                ));
                                if arrow.clicked() {
                                    move_to = if up { move_up_target } else { move_down_target };
                                    clicked_control = true;
                                }
                            }
                            if response.clicked() && !clicked_control {
                                select_layer = true;
                            }
                            // Where a dragged layer would land on this row: above or
                            // below it, or into a folder (the middle of its row, or
                            // its lower part when it is open).
                            if dragging.is_some()
                                && !dragged_subtree.contains(&layer_id)
                                && let Some(pointer) = pointer.filter(|p| row.contains(*p))
                            {
                                use efude_canvas::LayerPlacement;
                                let fraction = (pointer.y - row.top()) / row.height();
                                let placement = if l.kind == LayerKind::Folder {
                                    if fraction < 0.3 {
                                        LayerPlacement::Above
                                    } else if fraction > 0.7 && !(l.expanded && has_children) {
                                        LayerPlacement::Below
                                    } else {
                                        LayerPlacement::Into
                                    }
                                } else if fraction < 0.5 {
                                    LayerPlacement::Above
                                } else {
                                    LayerPlacement::Below
                                };
                                let accent = Stroke::new(2.5, crate::layout::ACCENT);
                                match placement {
                                    LayerPlacement::Above => {
                                        painter.line_segment(
                                            [row.left_top(), row.right_top()],
                                            accent,
                                        );
                                    }
                                    LayerPlacement::Below => {
                                        painter.line_segment(
                                            [row.left_bottom(), row.right_bottom()],
                                            accent,
                                        );
                                    }
                                    LayerPlacement::Into => {
                                        painter.rect_stroke(
                                            row,
                                            7.0,
                                            accent,
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                }
                                drop_target = Some((layer_id, placement));
                            }
                            let raster = l.kind == LayerKind::Raster;
                            let alpha_lock = raster && !l.is_vector();
                            response.context_menu(|ui| {
                                use LayerAction::*;
                                for action in [
                                    Duplicate,
                                    MergeDown,
                                    MergeFolder,
                                    AlphaLock,
                                    Tint,
                                    SelectAlpha,
                                    Rasterize,
                                    Mask,
                                    Delete,
                                ] {
                                    let enabled = match action {
                                        MergeDown | MergeFolder => self.merge_available(
                                            i,
                                            matches!(action, MergeFolder),
                                            &owned,
                                        ),
                                        Rasterize => rasterizable,
                                        AlphaLock => alpha_lock,
                                        Tint => raster,
                                        _ => true,
                                    };
                                    if action.button(ui, enabled, english) {
                                        context_action = Some((i, action));
                                        ui.close_menu();
                                    }
                                }
                            });
                        } else if i == self.selected_layer {
                            egui::Frame::new()
                                .fill(Color32::from_rgb(34, 35, 42))
                                .corner_radius(7.0)
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(if english { "Name" } else { "名前" });
                                        ui.text_edit_singleline(&mut l.name);
                                    });
                                    egui::ComboBox::from_label(if english {
                                        "Parent Folder"
                                    } else {
                                        "親フォルダー"
                                    })
                                    .selected_text(parent_name)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_label(
                                                l.parent_id.is_none(),
                                                if english { "Top Level" } else { "最上位" },
                                            )
                                            .clicked()
                                        {
                                            change_parent = Some(None);
                                        }
                                        for (folder_id, folder_name) in &folder_options {
                                            if ui
                                                .selectable_label(
                                                    l.parent_id == Some(*folder_id),
                                                    folder_name,
                                                )
                                                .clicked()
                                            {
                                                change_parent = Some(Some(*folder_id));
                                            }
                                        }
                                    });
                                    ui.checkbox(
                                        &mut l.linear_blend,
                                        if english {
                                            "Blend in Linear Space"
                                        } else {
                                            "リニア空間で合成"
                                        },
                                    );
                                    if rasterizable
                                        && ui
                                            .button(if english {
                                                "Rasterize"
                                            } else {
                                                "ラスタライズ"
                                            })
                                            .on_hover_text(if english {
                                                "Turn the lines into pixels (a raster layer)"
                                            } else {
                                                "線を画素にして、ラスターレイヤーにします"
                                            })
                                            .clicked()
                                    {
                                        rasterize = Some(i);
                                    }
                                    if let Some(tone) = l.tone.as_mut() {
                                        ui.separator();
                                        ui.label(
                                            egui::RichText::new(if english {
                                                "Tone"
                                            } else {
                                                "トーン"
                                            })
                                            .strong(),
                                        );
                                        super::comic::tone_settings_ui(ui, tone, english);
                                    }
                                    if l.mask.is_none() {
                                        if ui
                                            .button(if english {
                                                "+ Layer Mask"
                                            } else {
                                                "＋ レイヤーマスク"
                                            })
                                            .clicked()
                                        {
                                            change_mask = Some(true);
                                        }
                                    } else {
                                        ui.checkbox(
                                            &mut self.editing_mask,
                                            if english {
                                                "Edit Mask"
                                            } else {
                                                "マスクを編集"
                                            },
                                        );
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button(if english {
                                                    "Invert Mask"
                                                } else {
                                                    "マスク反転"
                                                })
                                                .clicked()
                                            {
                                                mask_action = Some(0u8);
                                            }
                                            if ui
                                                .add_enabled(
                                                    self.selection.active,
                                                    egui::Button::new(if english {
                                                        "From Selection"
                                                    } else {
                                                        "選択範囲から"
                                                    }),
                                                )
                                                .clicked()
                                            {
                                                mask_action = Some(1u8);
                                            }
                                        });
                                        if ui
                                            .button(if english {
                                                "Remove Mask"
                                            } else {
                                                "マスクを削除"
                                            })
                                            .clicked()
                                        {
                                            change_mask = Some(false);
                                        }
                                    }
                                });
                        }
                    }
                    if select_layer {
                        self.select_layer(i);
                        if let Some(mask) = thumbnail_target {
                            self.editing_mask = mask;
                        }
                    }
                    if let Some(add_mask) = change_mask {
                        let mask = add_mask.then(|| {
                            efude_canvas::TilePixels::new(self.doc.width, self.doc.height)
                        });
                        self.history.set_layer_mask(&mut self.doc.layers, i, mask);
                        if !add_mask {
                            self.editing_mask = false;
                        }
                    }
                    if let Some(action) = mask_action {
                        self.apply_layer_mask_action(i, action);
                    }
                    if let Some(new_parent) = change_parent {
                        self.doc.layers[i].parent_id = new_parent;
                    }
                    let property_after = self.doc.layers[i].property_state();
                    self.history
                        .record_layer_properties(layer_id, property_before, property_after);
                    if let Some((name, opacity, visible, blend)) = macro_before {
                        let layer = &self.doc.layers[i];
                        let mut steps = Vec::new();
                        if name != layer.name {
                            steps.push(macros::Step::SetName {
                                name: layer.name.clone(),
                            });
                        }
                        if opacity != layer.opacity {
                            steps.push(macros::Step::SetOpacity {
                                value: layer.opacity,
                            });
                        }
                        if visible != layer.visible {
                            steps.push(macros::Step::SetVisibility {
                                visible: layer.visible,
                            });
                        }
                        if blend != layer.blend {
                            steps.push(macros::Step::SetBlend { mode: layer.blend });
                        }
                        for step in steps {
                            self.record_macro_step(step, layer_id);
                        }
                    }
                    if let Some(j) = move_to {
                        let selected_id = self.doc.layers[self.selected_layer].id;
                        if let Some(new_index) =
                            self.history.move_layer_subtree(&mut self.doc.layers, i, j)
                        {
                            self.selected_layer = if selected_id == layer_id {
                                new_index
                            } else {
                                self.doc
                                    .layers
                                    .iter()
                                    .position(|layer| layer.id == selected_id)
                                    .unwrap_or(new_index)
                            };
                        }
                    }
                }
                if let Some((i, action)) = context_action {
                    self.select_layer(i);
                    self.run_layer_action(action);
                }
                if let Some(index) = rasterize {
                    self.rasterize_layer(index);
                }
                if let Some(id) = dragging {
                    let ctx = ui.ctx().clone();
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                    if let (Some(pointer), Some(layer)) =
                        (pointer, self.doc.layers.iter().find(|layer| layer.id == id))
                    {
                        let painter = ctx.layer_painter(egui::LayerId::new(
                            egui::Order::Tooltip,
                            egui::Id::new("layer-drag"),
                        ));
                        let text = painter.layout_no_wrap(
                            layer.name.clone(),
                            egui::FontId::proportional(14.0),
                            Color32::from_rgb(235, 235, 245),
                        );
                        let rect = Rect::from_min_size(
                            pointer + Vec2::new(14.0, 6.0),
                            text.size() + Vec2::new(16.0, 8.0),
                        );
                        painter.rect_filled(
                            rect,
                            6.0,
                            Color32::from_rgba_unmultiplied(40, 42, 54, 230),
                        );
                        painter.galley(rect.min + Vec2::new(8.0, 4.0), text, Color32::WHITE);
                    }
                    if ctx.input(|input| input.pointer.any_released() || !input.pointer.any_down())
                    {
                        self.layer_drag = None;
                        if let Some((target, placement)) = drop_target {
                            let selected = self.doc.layers.get(self.selected_layer).map(|l| l.id);
                            if self
                                .history
                                .place_layer(&mut self.doc.layers, id, target, placement)
                            {
                                if let Some(index) = selected
                                    .and_then(|s| self.doc.layers.iter().position(|l| l.id == s))
                                {
                                    self.selected_layer = index;
                                }
                                self.canvas_texture_dirty = true;
                            }
                        }
                    }
                }
            });
    }

    fn add_folder(&mut self) {
        self.finish_pending_canvas_gesture();
        if !self.can_add_layers(1) {
            return;
        }
        let english = self.language_english;
        let id = self.next_layer_id();
        let mut folder = efude_canvas::Layer::new(
            id,
            if english {
                format!("Folder {id}")
            } else {
                format!("フォルダ {id}")
            },
            self.doc.width,
            self.doc.height,
        );
        folder.kind = LayerKind::Folder;
        folder.parent_id = if self.doc.layers[self.selected_layer].kind == LayerKind::Folder {
            Some(self.doc.layers[self.selected_layer].id)
        } else {
            self.doc.layers[self.selected_layer].parent_id
        };
        let at = self.doc.layers.len();
        self.history.insert_layer(&mut self.doc.layers, at, folder);
        self.selected_layer = self.doc.layers.len() - 1;
    }
    fn merge_available(
        &self,
        i: usize,
        folder: bool,
        owned: &std::collections::HashSet<u64>,
    ) -> bool {
        #[cfg(test)]
        MERGE_CHECKS.set(MERGE_CHECKS.get() + 1);
        efude_canvas::merge_available(&self.doc, i, folder)
            && (owned.is_empty()
                || efude_canvas::merge_targets(&self.doc, i, folder)
                    .is_some_and(|ids| ids.iter().all(|id| !owned.contains(id))))
    }
    pub(super) fn merge_selected(&mut self, folder: bool) {
        self.finish_pending_canvas_gesture();
        let i = self.selected_layer;
        if let Some(ids) = efude_canvas::merge_targets(&self.doc, i, folder)
            && self
                .editable_object_layers()
                .iter()
                .any(|id| ids.contains(id))
        {
            self.status = self
                .text(
                    "結合する前にフキダシ・コマをラスタライズしてください",
                    "Rasterize the balloon or panel before merging",
                )
                .into();
            return;
        }
        match efude_canvas::merge_compatibility(&self.doc, i, folder) {
            efude_canvas::MergeCompatibility::Exact => self.perform_merge(folder),
            efude_canvas::MergeCompatibility::ChangesAppearance(reason) => {
                self.pending_merge = Some((
                    folder,
                    self.doc.layers[i].id,
                    self.history.state_token(),
                    reason,
                ))
            }
            efude_canvas::MergeCompatibility::Unavailable => {}
        }
    }
    pub(super) fn resolve_merge_confirmation(&mut self, confirmed: bool) {
        if let Some((folder, id, token, _)) = self.pending_merge.take()
            && confirmed
            && self.history.state_token() == token
            && self.doc.layers[self.selected_layer].id == id
        {
            self.perform_merge(folder);
        }
    }
    fn perform_merge(&mut self, folder: bool) {
        self.finish_pending_canvas_gesture();
        self.history.begin();
        let merge = if folder {
            efude_canvas::merge_folder
        } else {
            efude_canvas::merge_down
        };
        let targets =
            efude_canvas::merge_targets(&self.doc, self.selected_layer, folder).unwrap_or_default();
        if let Some(i) = merge(&mut self.doc, self.selected_layer, &mut self.history) {
            let removed: Vec<_> = targets
                .into_iter()
                .filter(|id| *id != self.doc.layers[i].id)
                .collect();
            self.clear_removed_layer_references(&removed);
            self.selected_layer = i;
            self.editing_mask = false;
        }
        self.history
            .commit_labeled(self.text("レイヤー結合", "Merge layers"));
    }
    pub(super) fn select_layer_alpha(&mut self) {
        self.finish_pending_canvas_gesture();
        let l = &self.doc.layers[self.selected_layer];
        let mask = if l.kind == LayerKind::Folder {
            efude_canvas::folder_alpha_selection(&self.doc, self.selected_layer)
        } else {
            efude_canvas::layer_alpha_selection(l, self.doc.width, self.doc.height)
        };
        if !mask.iter().any(|&a| a != 0) {
            self.status = self
                .text("不透明なピクセルがありません", "No opaque pixels to select")
                .into();
            return;
        }
        self.change_selection(|selection, _, _| {
            selection.active = true;
            selection.mask = mask;
        });
    }
    pub(super) fn layer_menu(&mut self, ui: &mut egui::Ui) {
        use LayerAction::*;
        let owned = self.editable_object_layers();
        for action in [
            NewRaster,
            NewVector,
            NewFolder,
            Duplicate,
            MergeDown,
            MergeFolder,
            MergeVisible,
            CopyMerged,
            AlphaLock,
            Tint,
            Clip,
            SelectAlpha,
            Rasterize,
            Clear,
        ] {
            let l = &self.doc.layers[self.selected_layer];
            let enabled = match action {
                MergeDown | MergeFolder => {
                    self.merge_available(self.selected_layer, matches!(action, MergeFolder), &owned)
                }
                Rasterize => l.is_vector() || owned.contains(&l.id),
                AlphaLock => l.kind == LayerKind::Raster && !l.is_vector(),
                Tint => l.kind == LayerKind::Raster,
                _ => true,
            };
            if action.button(ui, enabled, self.language_english) {
                self.run_layer_action(action);
                ui.close_menu();
            }
        }
    }
    fn run_layer_action(&mut self, action: LayerAction) {
        self.finish_pending_canvas_gesture();
        let i = self.selected_layer;
        match action {
            LayerAction::NewRaster => self.add_raster_layer(),
            LayerAction::NewVector => self.add_vector_layer(),
            LayerAction::NewFolder => self.add_folder(),
            LayerAction::Duplicate => self.duplicate_layer_subtree(),
            LayerAction::MergeDown => self.merge_selected(false),
            LayerAction::MergeFolder => self.merge_selected(true),
            LayerAction::MergeVisible => self.merge_visible(false),
            LayerAction::CopyMerged => self.merge_visible(true),
            LayerAction::SelectAlpha => self.select_layer_alpha(),
            LayerAction::Rasterize => self.rasterize_layer(i),
            LayerAction::Clear => self.confirm_layer_clear = true,
            LayerAction::Delete => {
                if self.doc.layers.len() > 1 {
                    self.delete_document_layer(i);
                }
            }
            LayerAction::Mask => {
                let mask = self.doc.layers[i]
                    .mask
                    .is_none()
                    .then(|| efude_canvas::TilePixels::new(self.doc.width, self.doc.height));
                self.history.set_layer_mask(&mut self.doc.layers, i, mask);
                self.sync_mask_edit_mode();
            }
            LayerAction::Clip | LayerAction::Tint | LayerAction::AlphaLock => {
                let l = &mut self.doc.layers[i];
                if matches!(action, LayerAction::AlphaLock | LayerAction::Tint)
                    && l.kind == LayerKind::Folder
                    || matches!(action, LayerAction::AlphaLock) && l.is_vector()
                {
                    return;
                }
                let before = l.property_state();
                match action {
                    LayerAction::Clip => l.clipping = !l.clipping,
                    LayerAction::Tint => {
                        l.layer_color = l.layer_color.is_none().then_some([88, 140, 220])
                    }
                    _ => l.alpha_lock = !l.alpha_lock,
                }
                self.history
                    .record_layer_properties(l.id, before, l.property_state());
            }
        }
    }
    fn layer_controls(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let l = &mut self.doc.layers[self.selected_layer];
        let before = l.property_state();
        let (opacity, blend, id) = (l.opacity, l.blend, l.id);
        let width = ui.available_width();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().slider_width = (width * 0.33).clamp(45., 140.);
            egui::ComboBox::from_id_salt("layer-blend")
                .width((width * 0.30).clamp(60., 120.))
                .selected_text(blend_mode_name(l.blend, english))
                .show_ui(ui, |ui| {
                    for mode in BLEND_MODES {
                        ui.selectable_value(&mut l.blend, mode, blend_mode_name(mode, english));
                    }
                })
                .response
                .on_hover_text(if english {
                    "Blend mode"
                } else {
                    "合成モード"
                });

            ui.add(
                egui::Slider::new(&mut l.opacity, 0.0..=1.).text(if english {
                    "Opacity"
                } else {
                    "不透明度"
                }),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(l.kind == LayerKind::Raster && !l.is_vector(), |ui| {
                ui.toggle_value(&mut l.alpha_lock, "α")
            });
            for (value, ja, en) in [
                (&mut l.clipping, "クリップ", "Clip"),
                (&mut l.locked, "ロック", "Lock"),
                (&mut l.reference, "参照", "Reference"),
                (&mut l.sketch, "下描き", "Sketch"),
            ] {
                ui.toggle_value(value, if english { en } else { ja });
            }
            ui.add_enabled_ui(l.kind == LayerKind::Raster, |ui| {
                if ui
                    .selectable_label(
                        l.layer_color.is_some(),
                        if english { "Tint" } else { "着色" },
                    )
                    .clicked()
                {
                    l.layer_color = l.layer_color.is_none().then_some([88, 140, 220]);
                }
                if let Some(rgb) = &mut l.layer_color {
                    ui.color_edit_button_srgb(rgb);
                }
            });
        });
        self.history
            .record_layer_properties(l.id, before, l.property_state());
        let (new_opacity, new_blend) = (l.opacity, l.blend);
        if opacity != new_opacity {
            self.record_macro_step(macros::Step::SetOpacity { value: new_opacity }, id);
        }
        if blend != new_blend {
            self.record_macro_step(macros::Step::SetBlend { mode: new_blend }, id);
        }
    }
    fn thumbnail_ids(
        &mut self,
        i: usize,
        ctx: &egui::Context,
    ) -> (egui::TextureId, Option<egui::TextureId>) {
        let l = &self.doc.layers[i];
        let revision = self.history.content_revision();
        let cached = self
            .thumbnails
            .entry(l.id)
            .or_insert_with(|| (u64::MAX, None, None));
        if cached.0 != revision || cached.1.is_none() {
            let make = |mask: bool| {
                let fit = 34. / self.doc.width.max(self.doc.height) as f32;
                let pixels = (0..34 * 34)
                    .map(|p| {
                        let x = ((p % 34) as f32 + 0.5 - 17.) / fit + self.doc.width as f32 / 2.;
                        let y = ((p / 34) as f32 + 0.5 - 17.) / fit + self.doc.height as f32 / 2.;
                        let check = if (p % 34 / 4 + p / 34 / 4) % 2 == 0 {
                            160
                        } else {
                            196
                        };
                        if x < 0.
                            || y < 0.
                            || x >= self.doc.width as f32
                            || y >= self.doc.height as f32
                        {
                            return Color32::from_gray(check);
                        }
                        let (x, y) = (x as u32, y as u32);
                        if mask {
                            return Color32::from_gray(
                                l.mask
                                    .as_ref()
                                    .unwrap()
                                    .pixel_or_tile_default(x, y, [255; 4])[0],
                            );
                        }
                        let [r, g, b, a] = efude_canvas::display_pixel(
                            l,
                            self.doc.dpi,
                            x,
                            y,
                            l.pixels.pixel(x, y),
                        );
                        let over = |v: u8| {
                            ((v as u32 * a as u32 + check as u32 * (255 - a as u32)) / 255) as u8
                        };
                        Color32::from_rgb(over(r), over(g), over(b))
                    })
                    .collect();
                ctx.load_texture(
                    "layer-thumbnail",
                    egui::ColorImage {
                        size: [34; 2],
                        pixels,
                    },
                    egui::TextureOptions::LINEAR,
                )
            };
            *cached = (
                revision,
                Some(make(false)),
                l.mask.is_some().then(|| make(true)),
            );
        }
        (
            cached.1.as_ref().unwrap().id(),
            cached.2.as_ref().map(|tex| tex.id()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stage5_vector_and_folder_properties_are_disabled() {
        let mut app = EfudeApp::default();
        app.add_vector_layer();
        app.run_layer_action(LayerAction::AlphaLock);
        assert!(!app.doc.layers[app.selected_layer].alpha_lock);
        app.add_folder();
        app.run_layer_action(LayerAction::Tint);
        app.run_layer_action(LayerAction::AlphaLock);
        let l = &app.doc.layers[app.selected_layer];
        assert!(!l.alpha_lock && l.layer_color.is_none());
    }
    #[test]
    fn stage5_thumbnail_uses_display_color() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(1, 1);
        app.doc.layers[0].layer_color = Some([0, 0, 255]);
        app.doc.layers[0].pixels.set_pixel(0, 0, [0, 0, 0, 255]);
        let ctx = egui::Context::default();
        let mut id = None;
        let out = ctx.run(Default::default(), |ctx| {
            id = Some(app.thumbnail_ids(0, ctx).0)
        });
        let image = &out
            .textures_delta
            .set
            .iter()
            .find(|(tex, _)| Some(*tex) == id)
            .unwrap()
            .1
            .image;
        let egui::ImageData::Color(image) = image else {
            panic!("color texture")
        };
        assert_eq!(image.pixels[17 * 34 + 17], Color32::BLUE);
    }
    #[test]
    fn stage3_delete_confirmation_belongs_to_one_document() {
        let mut app = EfudeApp::default();
        app.add_raster_layer();
        app.run_layer_action(LayerAction::Clear);
        assert!(app.confirm_layer_clear);
        app.open_document_tab();
        assert!(!app.confirm_layer_clear);
        assert_eq!(app.doc.layers.len(), 1);
        app.switch_tab(0);
        assert_eq!(app.doc.layers.len(), 2);
    }
    #[test]
    fn stage3_layer_controls_fit_the_default_dock_width() {
        let mut app = EfudeApp::default();
        let ctx = egui::Context::default();
        for english in [false, true] {
            app.language_english = english;
            let width = 360.;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = Rect::from_min_size(ui.cursor().min, Vec2::new(width, 400.));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    child.spacing_mut().slider_width = 200.;
                    app.layer_controls(&mut child);
                    assert!(
                        child.min_rect().width() <= width + 0.5,
                        "layer bar grew to {} in {} px",
                        child.min_rect().width(),
                        width
                    );
                });
            });
        }
    }
    #[test]
    fn stage3_layer_tint_changes_composite_and_undo_preserves_the_original() {
        let mut app = EfudeApp::default();
        app.doc = Document::new(1, 1);
        app.doc.layers[0].pixels.set_pixel(0, 0, [30, 30, 30, 255]);
        app.run_layer_action(LayerAction::Tint);
        assert_eq!(app.doc.layers[0].pixels.pixel(0, 0), [30, 30, 30, 255]);
        let output = efude_canvas::composite_transparent(&app.doc);
        assert!(output[2] > output[0]);
        app.undo();
        assert_eq!(
            efude_canvas::composite_transparent(&app.doc),
            [30, 30, 30, 255]
        );
    }
}

#[cfg(test)]
thread_local! { pub(super) static MERGE_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
