// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Regression coverage for layer operations and repeated history navigation.

use super::*;

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
