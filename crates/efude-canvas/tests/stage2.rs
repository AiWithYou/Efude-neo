// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 AiWithYou
use efude_canvas::filters::{Filter, GRADIENT_MAP_PRESETS};
use efude_canvas::*;
#[test]
fn layer_properties_display_and_history() {
    let mut d = Document::new(1, 1);
    let mut h = History::default();
    let l = &mut d.layers[0];
    let p = [0, 255, 0, 79];
    l.pixels.set_pixel(0, 0, p);
    let before = l.property_state();
    l.alpha_lock = true;
    l.layer_color = Some([0, 0, 255]);
    h.set_next_label("Tint");
    h.record_layer_properties(l.id, before, l.property_state());
    assert_eq!(composite_transparent(&d), [182, 182, 255, 79]);
    assert_eq!(d.layers[0].pixels.pixel(0, 0), p);
    assert_eq!(h.undo_labels(), ["Tint"]);
    h.undo_document(&mut d);
    assert!(!d.layers[0].alpha_lock);
    assert_eq!(h.redo_labels(), ["Tint"]);
    h.redo_document(&mut d);
    assert_eq!(h.undo_len(), 1);
    assert_eq!(h.redo_len(), 0);
}
#[test]
fn gradient_shape_selection_and_crop() {
    let mut d = Document::new(5, 5);
    let mut h = History::default();
    let l = &mut d.layers[0];
    l.alpha_lock = true;
    l.pixels.set_pixel(2, 2, [0, 0, 0, 80]);
    let mut mask = vec![0; 25];
    mask[12] = 128;
    let stops = [
        GradientStop {
            position: 0.,
            color: [255; 4],
        },
        GradientStop {
            position: 1.,
            color: [255; 4],
        },
    ];
    for kind in [
        GradientKind::Linear,
        GradientKind::Radial,
        GradientKind::Reflected,
    ] {
        h.begin();
        assert!(draw_gradient(
            l,
            5,
            5,
            [0., 0.],
            [5., 0.],
            kind,
            &stops,
            Some(&mask),
            &mut h
        ));
        h.commit_labeled("Gradient");
        assert_eq!(l.pixels.pixel(2, 2), [128, 128, 128, 80]);
        h.undo(l);
    }
    for kind in [
        ShapeKind::Rectangle,
        ShapeKind::RoundedRect { radius: 1. },
        ShapeKind::Ellipse,
        ShapeKind::Polygon { sides: 4 },
        ShapeKind::Line,
    ] {
        h.begin();
        assert!(draw_shape(
            l,
            5,
            5,
            [0., 0.],
            [5., 5.],
            kind,
            ShapeStyle {
                fill: Some([255; 4]),
                stroke: Some(([255; 4], 2.))
            },
            None,
            &mut h
        ));
        h.commit();
        assert_eq!(l.pixels.pixel(2, 2)[3], 80);
        assert_eq!(l.pixels.pixel(1, 1), [0; 4]);
        h.undo(l);
    }
    assert_eq!(layer_alpha_selection(l, 5, 5)[12], 80);
    assert_eq!(document_content_bounds(&d), Some([2, 2, 1, 1]));
    assert_eq!(crop_document(&mut d, [2, 2, 1, 1], &mut h), Ok(true));
    assert_eq!(d.layers[0].pixels.pixel(0, 0)[3], 80);
    h.undo_document(&mut d);
    assert_eq!(d.width, 5);
}
#[test]
fn merges_bake_mask_and_tint_and_undo() {
    let mut d = Document::new(1, 1);
    let mut h = History::default();
    let mut top = Layer::new(2, "top", 1, 1);
    top.layer_color = Some([200, 10, 0]);
    top.pixels.set_pixel(0, 0, [0, 0, 0, 120]);
    d.layers.push(top);
    let before = composite_transparent(&d);
    assert_eq!(merge_down(&mut d, 1, &mut h), Some(0));
    assert_eq!(composite_transparent(&d), before);
    h.undo_document(&mut d);
    assert_eq!(d.layers.len(), 2);
    d.layers[0].kind = LayerKind::Folder;
    d.layers[0].opacity = 0.5;
    d.layers[1].parent_id = Some(1);
    let before = composite_transparent(&d);
    assert_eq!(merge_folder(&mut d, 0, &mut h), Some(0));
    assert_eq!(composite_transparent(&d), before);
    h.undo_document(&mut d);
    assert_eq!(d.layers[1].parent_id, Some(1));
}
#[test]
fn tonal_filters_and_outline() {
    for (filter, expected) in [
        (Filter::Invert, [191, 191, 191, 120]),
        (Filter::LuminanceToOpacity, [0, 0, 0, 90]),
        (Filter::Posterize { levels: 2 }, [0, 0, 0, 120]),
        (Filter::Threshold { level: 64 }, [255, 255, 255, 120]),
        (
            Filter::ColorBalance {
                shadows: [0.; 3],
                midtones: [0.; 3],
                highlights: [0.; 3],
                preserve_luminosity: true,
            },
            [64, 64, 64, 120],
        ),
        (
            Filter::GradientMap {
                stops: [(0., [0; 3]), (0.5, [0; 3]), (1., [255; 3])],
            },
            [0, 0, 0, 120],
        ),
    ] {
        let mut p = [64, 64, 64, 120];
        filter.apply(&mut p, 1, 1, (0, 0));
        assert_eq!(p, expected);
    }
    assert!((6..=8).contains(&GRADIENT_MAP_PRESETS.len()));
    let f = Filter::Outline {
        width: 1,
        color: [255, 0, 0, 255],
        outside_only: true,
    };
    assert_eq!(f.reach(), 2);
    let mut p = [0; 12];
    p[7] = 255;
    f.apply(&mut p, 3, 1, (0, 0));
    assert_eq!(p, [255, 0, 0, 255, 0, 0, 0, 255, 255, 0, 0, 255]);
}

#[test]
fn shape_applies_selection_once_where_fill_and_stroke_overlap() {
    let mut d = Document::new(3, 3);
    let mut history = History::default();
    assert!(draw_shape(
        &mut d.layers[0],
        3,
        3,
        [0., 0.],
        [3., 3.],
        ShapeKind::Rectangle,
        ShapeStyle {
            fill: Some([255; 4]),
            stroke: Some(([0, 0, 0, 255], 3.))
        },
        Some(&[128; 9]),
        &mut history
    ));
    assert_eq!(d.layers[0].pixels.pixel(0, 1), [0, 0, 0, 128]);
    assert_eq!(history.undo_len(), 1);
}
#[test]
fn gradients_evaluate_pixel_centres_in_two_dimensions() {
    let stops = [
        GradientStop {
            position: 1.,
            color: [255; 4],
        },
        GradientStop {
            position: 0.,
            color: [0, 0, 0, 255],
        },
    ];
    for (kind, left, below) in [
        (GradientKind::Linear, 0, 0),
        (GradientKind::Reflected, 128, 0),
        (GradientKind::Radial, 128, 255),
    ] {
        let mut d = Document::new(5, 5);
        let mut h = History::default();
        assert!(draw_gradient(
            &mut d.layers[0],
            5,
            5,
            [2.5, 2.5],
            [4.5, 2.5],
            kind,
            &stops,
            None,
            &mut h
        ));
        assert_eq!(d.layers[0].pixels.pixel(1, 2)[0], left);
        assert_eq!(d.layers[0].pixels.pixel(2, 4)[0], below);
    }
}
#[test]
fn crop_rejects_overflow_merge_rejects_locked_and_bounds_respects_ancestor_masks() {
    let mut d = Document::new(4, 3);
    let mut h = History::default();
    d.layers[0].kind = LayerKind::Folder;
    let mut mask = TilePixels::new(4, 3);
    mask.ensure_tile_filled(0, 0, [255; 4]);
    mask.set_pixel(1, 1, [0; 4]);
    d.layers[0].mask = Some(mask);
    let mut l = Layer::new(2, "child", 4, 3);
    l.parent_id = Some(1);
    l.pixels.set_pixel(1, 1, [255; 4]);
    l.pixels.set_pixel(3, 1, [255; 4]);
    d.layers.push(l);
    assert_eq!(document_content_bounds(&d), Some([3, 1, 1, 1]));
    d.layers[0].visible = false;
    assert_eq!(document_content_bounds(&d), None);
    d.layers[1].locked = true;
    assert_eq!(merge_folder(&mut d, 0, &mut h), None);
    assert_eq!(crop_document(&mut d, [u32::MAX, 0, 2, 1], &mut h), Err(()));
    assert_eq!(crop_document(&mut d, [0, 0, 4, 3], &mut h), Ok(false));
    assert_eq!(h.undo_len(), 0);
}

#[test]
fn circular_outline_matches_direct_maximum_including_partial_coverage() {
    let (width, height) = (19, 13);
    let orig: Vec<u8> = (0..width * height)
        .flat_map(|i| [33, 77, 151, if i % 9 == 0 { (17 * i) as u8 } else { 0 }])
        .collect();
    for r in [1, 2, 5, 12] {
        let mut actual = orig.clone();
        Filter::Outline {
            width: r,
            color: [33, 77, 151, 255],
            outside_only: false,
        }
        .apply(&mut actual, width, height, (83, 61));
        for y in 0..height {
            for x in 0..width {
                let mut maximum = 0f32;
                for sy in 0..height {
                    for sx in 0..width {
                        let distance = (x.abs_diff(sx) as f32).hypot(y.abs_diff(sy) as f32);
                        maximum = maximum.max(
                            orig[4 * (sy * width + sx) + 3] as f32
                                * (r as f32 + 1. - distance).clamp(0., 1.),
                        );
                    }
                }
                let i = 4 * (y * width + x) + 3;
                let a = orig[i] as f32 / 255.;
                let expected = (a * 255. + maximum.round() * (1. - a)).round() as u8;
                assert_eq!(actual[i], expected);
            }
        }
    }
}

#[test]
fn merge_down_reapplies_shared_parent_opacity_once() {
    let mut d = Document::new(1, 1);
    d.layers[0].kind = LayerKind::Folder;
    d.layers[0].opacity = 0.5;
    for id in [2, 3] {
        let mut l = Layer::new(id, "paint", 1, 1);
        l.parent_id = Some(1);
        l.pixels.set_pixel(0, 0, [255; 4]);
        d.layers.push(l);
    }
    let mut h = History::default();
    assert_eq!(composite_transparent(&d)[3], 192);
    assert_eq!(merge_down(&mut d, 2, &mut h), Some(1));
    assert_eq!(d.layers[1].parent_id, Some(1));
    assert_eq!(composite_transparent(&d)[3], 128);
    h.undo_document(&mut d);
    assert_eq!(composite_transparent(&d)[3], 192);
}
#[test]
fn merging_a_toned_clipped_linear_blend_layer_uses_document_coordinates() {
    let mut d = Document::new(TILE_SIZE + 9, 4);
    let mut upper = Layer::new(2, "ink", d.width, d.height);
    upper.blend = BlendMode::Multiply;
    upper.linear_blend = true;
    upper.opacity = 0.7;
    upper.clipping = true;
    upper.tone = Some(ToneSettings::default());
    upper.layer_color = Some([72, 13, 101]);
    let mut mask = TilePixels::new(d.width, d.height);
    mask.ensure_tile_filled(256, 0, [255; 4]);
    mask.set_pixel(257, 1, [89; 4]);
    upper.mask = Some(mask);
    for x in 253..264 {
        for y in 0..4 {
            d.layers[0].pixels.set_pixel(x, y, [165, 207, 85, 114]);
            upper.pixels.set_pixel(x, y, [0, 0, 0, 196]);
        }
    }
    d.layers.push(upper);
    let original = composite_transparent(&d);
    let mut h = History::default();
    assert_eq!(merge_down(&mut d, 1, &mut h), Some(0));
    assert_eq!(d.layers[0].pixels.to_dense(), original);
    h.undo_document(&mut d);
    assert!(d.layers[1].clipping);
    assert!(d.layers[1].tone.is_some());
}

#[test]
fn crop_moves_hidden_rgb_soft_masks_vectors_and_guide_together() {
    let mut d = Document::new(12, 14);
    let mut h = History::default();
    d.layers[0].pixels.set_pixel(3, 3, [8, 7, 6, 0]);
    let mut mask = TilePixels::new(12, 14);
    mask.ensure_tile_filled(0, 0, [255; 4]);
    mask.set_pixel(3, 3, [77; 4]);
    d.layers[0].mask = Some(mask);
    let mut guide = GuideImage::fit_to_canvas(1, 1, vec![255; 4], &d).unwrap();
    guide.offset_x = 4.;
    guide.offset_y = 3.;
    d.guide = Some(guide);
    let mut v = Layer::new(2, "vector", 12, 14);
    v.vector = Some(vec![VectorStroke {
        points: vec![VectorPoint {
            x: 5.,
            y: 6.,
            width: 2.,
        }],
        color: [1, 2, 3, 255],
        hardness: 1.,
        anchors: vec![VectorAnchor {
            x: 5.,
            y: 6.,
            width: 2.,
            in_x: 1.,
            in_y: -1.,
            out_x: 2.,
            out_y: 3.,
            corner: false,
        }],
    }]);
    d.layers.push(v);
    let before = postcard::to_allocvec(&d.layers).unwrap();
    assert_eq!(crop_document(&mut d, [2, 3, 6, 7], &mut h), Ok(true));
    assert_eq!(d.layers[0].pixels.pixel(1, 0), [8, 7, 6, 0]);
    assert_eq!(d.layers[0].mask.as_ref().unwrap().pixel(1, 0), [77; 4]);
    let v = &d.layers[1].vector.as_ref().unwrap()[0];
    assert_eq!(
        [v.points[0].x, v.points[0].y, v.anchors[0].x, v.anchors[0].y],
        [3., 3., 3., 3.]
    );
    let guide = d.guide.as_ref().unwrap();
    assert_eq!([guide.offset_x, guide.offset_y], [2., 0.]);
    h.undo_document(&mut d);
    assert_eq!((d.width, d.height), (12, 14));
    assert_eq!(postcard::to_allocvec(&d.layers).unwrap(), before);
}

#[test]
fn visible_raster_bounds_include_coverage_from_a_vector_clipping_base() {
    let mut doc = Document::new(3, 1);
    doc.layers[0].vector = Some(vec![]);
    doc.layers[0].pixels.set_pixel(0, 0, [255; 4]);
    doc.layers[0].pixels.set_pixel(1, 0, [255; 4]);
    let mut layer = Layer::new(2, "clipped", 3, 1);
    layer.clipping = true;
    layer.pixels.set_pixel(1, 0, [255; 4]);
    layer.pixels.set_pixel(2, 0, [255; 4]);
    doc.layers.push(layer);
    assert_eq!(document_content_bounds(&doc), Some([0, 0, 2, 1]));
}

#[test]
fn threshold_includes_exact_rec709_boundary() {
    let mut pixels = [12, 12, 12, 62, 13, 13, 13, 110, 14, 14, 14, 255];
    Filter::Threshold { level: 13 }.apply(&mut pixels, 3, 1, (0, 0));
    assert_eq!(
        pixels,
        [0, 0, 0, 62, 255, 255, 255, 110, 255, 255, 255, 255]
    );
}

#[test]
fn stage5_clip_pair_preserves_external_base_and_undo() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels.set_pixel(0, 0, [255; 4]);
    for id in [2, 3] {
        let mut layer = Layer::new(id, "clip", 2, 1);
        layer.clipping = true;
        for x in 0..2 {
            layer.pixels.set_pixel(x, 0, [0, id as u8, 0, 255]);
        }
        doc.layers.push(layer);
    }
    let before = composite_transparent(&doc);
    let mut history = History::default();
    assert_eq!(merge_down(&mut doc, 2, &mut history), Some(1));
    assert!(doc.layers[1].clipping);
    assert_eq!(composite_transparent(&doc), before);
    history.undo_document(&mut doc);
    assert_eq!(doc.layers.len(), 3);
    assert_eq!(composite_transparent(&doc), before);
    doc.layers[2].clipping = false;
    assert_eq!(merge_down(&mut doc, 2, &mut history), None);
    doc.layers[2].kind = LayerKind::Folder;
    doc.layers[1].parent_id = Some(doc.layers[2].id);
    assert_eq!(merge_folder(&mut doc, 2, &mut history), None);
}
#[test]
fn stage5_vector_content_survives_trim() {
    let mut d = Document::new(11, 5);
    d.layers[0].vector = Some(vec![]);
    d.layers[0].pixels.set_pixel(9, 3, [255; 4]);
    assert_eq!(document_content_bounds(&d), Some([9, 3, 1, 1]));
    let mut raster = Layer::new(2, "paint", 11, 5);
    raster.pixels.set_pixel(3, 1, [255; 4]);
    d.layers.push(raster);
    let b = document_content_bounds(&d).unwrap();
    assert_eq!(b, [3, 1, 7, 3]);
    crop_document(&mut d, b, &mut History::default()).unwrap();
    assert_eq!(d.layers[0].pixels.pixel(6, 2), [255; 4]);
}
#[test]
fn stage5_inverse_affine_preserves_solid_interior() {
    let mut d = Document::new(31, 21);
    for y in 9..12 {
        for x in 12..19 {
            d.layers[0].pixels.set_pixel(x, y, [255; 4]);
        }
    }
    let before = d.layers[0].pixels.to_dense();
    let mut h = History::default();
    h.begin();
    transform_selection_offset(
        &mut d.layers[0],
        &mut Selection::default(),
        31,
        21,
        2.,
        3.,
        0.,
        [0.; 2],
        &mut h,
    );
    h.commit();
    for y in 7..14 {
        for x in 9..22 {
            assert_eq!(d.layers[0].pixels.pixel(x, y)[3], 255);
        }
    }
    h.undo_document(&mut d);
    assert_eq!(d.layers[0].pixels.to_dense(), before);
}
#[test]
fn stage5_mesh_expansion_and_trapezoid_follow_handles() {
    for skew in [0., 2.] {
        let mut d = Document::new(13, 13);
        d.layers[0].pixels.set_pixel(4, 12, [255; 4]);
        let grid: Vec<_> = (0..16)
            .map(|i| [(i % 4) as f32 * (4. + skew * (i / 4) as f32 / 3.), 0.])
            .collect();
        let mut h = History::default();
        h.begin();
        mesh_warp_grid(
            &mut d.layers[0],
            &mut Selection::default(),
            13,
            13,
            4,
            4,
            &grid,
            &mut h,
        );
        h.commit();
        assert_eq!(d.layers[0].pixels.pixel(8 + skew as u32, 12), [255; 4]);
        h.undo_document(&mut d);
        assert_eq!(d.layers[0].pixels.pixel(4, 12), [255; 4]);
    }
}
#[test]
fn stage5_invalid_transform_parameters_are_bounded() {
    for value in [f32::NAN, f32::INFINITY, 1e9] {
        let mut d = Document::new(4, 4);
        d.layers[0].pixels.set_pixel(1, 1, [255; 4]);
        let mut s = Selection::default();
        let mut h = History::default();
        transform_selection_offset(
            &mut d.layers[0],
            &mut s,
            4,
            4,
            value,
            1.,
            0.,
            [value, 0.],
            &mut h,
        );
        mesh_warp_grid(
            &mut d.layers[0],
            &mut s,
            4,
            4,
            4,
            4,
            &[[value, 0.]; 16],
            &mut h,
        );
        if !value.is_finite() {
            assert_eq!(d.layers[0].pixels.pixel(1, 1), [255; 4]);
        }
    }
}

#[test]
fn stage5_quarter_turn_and_reflection_keep_edge_pixels_and_undo() {
    for (sx, angle, target) in [
        (1., std::f32::consts::FRAC_PI_2, (128, 128)),
        (-1., 0., (0, 0)),
    ] {
        let mut doc = Document::new(129, 129);
        doc.layers[0].pixels.set_pixel(128, 0, [55, 20, 200, 255]);
        let mut h = History::default();
        h.begin();
        transform_selection_offset(
            &mut doc.layers[0],
            &mut Selection::default(),
            129,
            129,
            sx,
            1.,
            angle,
            [0.; 2],
            &mut h,
        );
        h.commit();
        let (x, y) = target;
        assert_eq!(doc.layers[0].pixels.pixel(x, y), [55, 20, 200, 255]);
        h.undo_document(&mut doc);
        assert_eq!(doc.layers[0].pixels.pixel(128, 0), [55, 20, 200, 255]);
    }
}

#[test]
fn fractional_move_samples_both_sides_of_one_selected_pixel() {
    let mut d = Document::new(3, 1);
    d.layers[0].pixels.set_pixel(0, 0, [255; 4]);
    let mut s = Selection {
        active: true,
        mask: vec![255, 0, 0],
    };
    transform_selection_offset(
        &mut d.layers[0],
        &mut s,
        3,
        1,
        1.,
        1.,
        0.,
        [0.5, 0.],
        &mut History::default(),
    );
    assert_eq!(s.mask, [128, 128, 0]);
    assert_eq!(d.layers[0].pixels.pixel(0, 0)[3], 128);
    assert_eq!(d.layers[0].pixels.pixel(1, 0)[3], 128);
}

#[test]
fn stage7_clip_merge_checks_each_painted_pixel() {
    for folder in [false, true] {
        for base in [0, 128, 255] {
            let mut doc = Document::new(2, 1);
            doc.layers[0].pixels.set_pixel(1, 0, [255; 4]);
            let mut lower = Layer::new(2, "base", 2, 1);
            lower.pixels.set_pixel(0, 0, [255; 4]);
            lower.pixels.set_pixel(1, 0, [0, 0, 0, base]);
            let mut clipped = Layer::new(3, "clip", 2, 1);
            clipped.clipping = true;
            clipped.pixels.set_pixel(1, 0, [255, 0, 0, 255]);
            if folder {
                let mut group = Layer::new(4, "group", 2, 1);
                group.kind = LayerKind::Folder;
                doc.layers.push(group);
                lower.parent_id = Some(4);
                clipped.parent_id = Some(4);
            }
            doc.layers.extend([lower, clipped]);
            let index = if folder { 1 } else { 2 };
            let before = composite_transparent(&doc);
            let allowed = base == 255;
            assert_eq!(
                merge_compatibility(&doc, index, folder),
                if allowed {
                    MergeCompatibility::Exact
                } else {
                    MergeCompatibility::Unavailable
                }
            );
            let mut history = History::default();
            let merge = if folder { merge_folder } else { merge_down };
            assert_eq!(merge(&mut doc, index, &mut history).is_some(), allowed);
            assert_eq!(composite_transparent(&doc), before);
            assert_eq!(history.undo_len(), usize::from(allowed));
        }
    }
}

#[test]
fn stage7_stationary_feather_preserves_colour_alpha_and_mask() {
    for mesh in [false, true] {
        for alpha in [0, 128, 255] {
            let mut layer = Layer::new(1, "", 7, 7);
            let color = [210, 42, 15, alpha];
            layer.pixels.set_pixel(3, 3, color);
            let mut mask = TilePixels::new(7, 7);
            mask.set_pixel(3, 3, [91; 4]);
            layer.mask = Some(mask);
            let mut selection = Selection {
                active: true,
                mask: vec![0; 49],
            };
            selection.mask[24] = 128;
            let mut history = History::default();
            if mesh {
                selection.mask[0] = 255;
                selection.mask[48] = 255;
                let mut offsets = [[0.; 2]; 16];
                offsets[15] = [0.5, 0.];
                mesh_warp_grid(
                    &mut layer,
                    &mut selection,
                    7,
                    7,
                    4,
                    4,
                    &offsets,
                    &mut history,
                );
            } else {
                transform_selection_offset(
                    &mut layer,
                    &mut selection,
                    7,
                    7,
                    1.,
                    1.,
                    std::f32::consts::FRAC_PI_2,
                    [0.; 2],
                    &mut history,
                );
            }
            assert_eq!(layer.pixels.pixel(3, 3), color);
            assert_eq!(layer.mask.unwrap().pixel(3, 3)[0], 91);
            assert_eq!(selection.mask[24], 128);
        }
    }
}

#[test]
fn stage7_full_coverage_overwrites_partly_selected_pixel() {
    let mut layer = Layer::new(1, "", 2, 1);
    layer.pixels.set_pixel(0, 0, [255, 0, 0, 255]);
    layer.pixels.set_pixel(1, 0, [0, 0, 255, 255]);
    let mut selection = Selection {
        active: true,
        mask: vec![255, 128],
    };
    transform_selection_offset(
        &mut layer,
        &mut selection,
        2,
        1,
        1.,
        1.,
        0.,
        [1., 0.],
        &mut History::default(),
    );
    assert_eq!(layer.pixels.pixel(1, 0), [255, 0, 0, 255]);
}

#[test]
fn stage7_mesh_translates_one_pixel_by_half_a_pixel() {
    let mut layer = Layer::new(1, "", 4, 3);
    layer.pixels.set_pixel(1, 1, [255; 4]);
    let mut selection = Selection {
        active: true,
        mask: vec![0; 12],
    };
    selection.mask[5] = 255;
    mesh_warp_grid(
        &mut layer,
        &mut selection,
        4,
        3,
        4,
        4,
        &[[0.5, 0.]; 16],
        &mut History::default(),
    );
    assert_eq!(selection.mask, [0, 0, 0, 0, 0, 128, 128, 0, 0, 0, 0, 0]);
    for x in [1, 2] {
        assert_eq!(layer.pixels.pixel(x, 1), [255, 255, 255, 128]);
    }
}

#[test]
fn stage7_folded_and_collapsed_mesh_cells_match_preview_and_undo() {
    for shift in [-2., -3.] {
        let mut doc = Document::new(7, 7);
        for x in 0..7 {
            doc.layers[0]
                .pixels
                .set_pixel(x, 3, [x as u8 * 30, 0, 0, 255]);
        }
        let before = doc.layers[0].pixels.to_dense();
        let mut selection = Selection::default();
        let mut offsets = [[0.; 2]; 16];
        for row in offsets.chunks_mut(4) {
            row[1][0] = shift;
        }
        let map = WarpMap::mesh(&doc.layers[0], &selection, [4, 4], &offsets).unwrap();
        let expected: Vec<_> = (0..7)
            .map(|x| map.preview_pixel(&doc.layers[0], &selection, x, 3).0)
            .collect();
        let mut history = History::default();
        history.begin();
        mesh_warp_grid(
            &mut doc.layers[0],
            &mut selection,
            7,
            7,
            4,
            4,
            &offsets,
            &mut history,
        );
        history.commit();
        for (x, pixel) in expected.into_iter().enumerate() {
            assert_eq!(doc.layers[0].pixels.pixel(x as u32, 3), pixel);
        }
        assert_eq!(doc.layers[0].pixels.pixel(5, 3), [150, 0, 0, 255]);
        history.undo_document(&mut doc);
        assert_eq!(doc.layers[0].pixels.to_dense(), before);
    }
}

#[test]
fn hidden_folder_merge_preserves_clipped_red_when_shown() {
    for base in [0, 255] {
        let mut doc = Document::new(1, 1);
        doc.layers[0].pixels.set_pixel(0, 0, [255; 4]);
        let mut folder = Layer::new(2, "hidden", 1, 1);
        folder.kind = LayerKind::Folder;
        folder.visible = false;
        let mut lower = Layer::new(3, "base", 1, 1);
        lower.parent_id = Some(2);
        lower.pixels.set_pixel(0, 0, [0, 0, 0, base]);
        let mut clip = Layer::new(4, "red", 1, 1);
        clip.clipping = true;
        clip.parent_id = Some(2);
        clip.pixels.set_pixel(0, 0, [255, 0, 0, 255]);
        doc.layers.extend([folder, lower, clip]);
        let expected = if base == 255 {
            MergeCompatibility::Exact
        } else {
            MergeCompatibility::Unavailable
        };
        assert_eq!(merge_compatibility(&doc, 1, true), expected);
        assert_eq!(
            merge_folder(&mut doc, 1, &mut History::default()),
            (base == 255).then_some(1)
        );
        assert_eq!(composite_transparent(&doc), [255; 4]);
        doc.layers[1].visible = true;
        assert_eq!(composite_transparent(&doc), [255, 0, 0, 255]);
    }
}

#[test]
fn hidden_folder_merge_does_not_show_a_hidden_child_folder() {
    let mut doc = Document::new(1, 1);
    doc.layers[0].pixels.set_pixel(0, 0, [255; 4]);
    let mut root = Layer::new(2, "root", 1, 1);
    root.kind = LayerKind::Folder;
    root.visible = false;
    let mut base = Layer::new(3, "clear", 1, 1);
    base.parent_id = Some(root.id);
    let mut child = Layer::new(4, "hidden child", 1, 1);
    child.kind = LayerKind::Folder;
    child.parent_id = Some(root.id);
    child.visible = false;
    let mut stroke = Layer::new(5, "clip", 1, 1);
    stroke.clipping = true;
    stroke.parent_id = Some(child.id);
    stroke.pixels.set_pixel(0, 0, [255, 0, 0, 255]);
    doc.layers.extend([root, base, child, stroke]);
    assert_eq!(
        merge_compatibility(&doc, 1, true),
        MergeCompatibility::Exact
    );
    assert_eq!(merge_folder(&mut doc, 1, &mut History::default()), Some(1));
    doc.layers[1].visible = true;
    assert_eq!(composite_transparent(&doc), [255; 4]);
}

#[test]
fn upper_clip_requires_confirmation_only_where_its_base_changes() {
    for (base, upper_x, intervening, expected) in [
        (
            128,
            Some(0),
            0,
            MergeCompatibility::ChangesAppearance(MergeAppearanceReason::TranslucentClipBase),
        ),
        (255, Some(0), 0, MergeCompatibility::Exact),
        (128, None, 0, MergeCompatibility::Exact),
        (128, Some(1), 0, MergeCompatibility::Exact),
        (128, Some(0), 255, MergeCompatibility::Exact),
    ] {
        let mut doc = Document::new(2, 1);
        doc.layers[0].pixels.set_pixel(0, 0, [0, 0, 0, base]);
        let mut clipped = Layer::new(2, "clipped", 2, 1);
        clipped.clipping = true;
        clipped.pixels.set_pixel(0, 0, [255; 4]);
        doc.layers.push(clipped);
        let mut cover = Layer::new(3, "cover", 2, 1);
        cover.pixels.set_pixel(0, 0, [0, 0, 0, intervening]);
        doc.layers.push(cover);
        if let Some(x) = upper_x {
            let mut upper = Layer::new(4, "upper", 2, 1);
            upper.clipping = true;
            upper.pixels.set_pixel(x, 0, [255; 4]);
            doc.layers.push(upper);
        }
        assert_eq!(merge_compatibility(&doc, 1, false), expected);
    }
}
