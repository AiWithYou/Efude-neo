# efude-canvas

The canvas: tiled layers and folders, blend modes, masks, selections, undo history, vector layers and image filters.

Vector selection erasure cuts the stroke path where it crosses selected pixel
cells (mask coverage at least 128), including crossings between stored points.
It preserves nonintersecting strokes and interpolates the width at cut ends.
As with the vector eraser, the surviving paths retain round caps; a gap narrower
than the line width can still be covered by those caps.

Alpha lock preserves the destination alpha byte, including partially transparent
edges. Layer colour tints the source RGB by its luminance after halftoning.
Gradient and shape tools sample pixel centres and apply the selection coverage
once to the finished paint. Both return whether any pixels changed. A call can
stand alone or join an open history transaction.

Layer merges composite their inputs over transparency. The resulting raster keeps
the common parent, whose mask and opacity apply once to the result. For example,
two overlapping opaque siblings under a 50% parent produce about 75% alpha
before merging and 50% afterwards. Undo restores both siblings and their settings.
Merging two clipped siblings keeps the result clipped to their base. An unclipped upper
layer cannot merge into a clipped lower sibling. A folder or sibling pair cannot be
merged into an unclipped raster if any visible clipped pixel depends on a base outside
the merge. `MergeCompatibility` identifies parent masks or opacity,
background-dependent blending, and translucent clipping bases before a merge.
Content bounds include visible vector strokes. Free transforms and fixed 4×4 mesh
warps inverse-map each destination pixel and bilinearly interpolate paint, masks and
selection coverage. Raster alpha lock also applies to paste, fills and filters, and
prevents erasure and transforms that would relocate alpha.

Crops and content bounds use rectangles in document pixels as
`[x, y, width, height]`. Undo labels run oldest first and redo labels start with
the next operation to replay.

Part of the engine of [Efude](https://github.com/852wa/Efude), an open-source
painting and manga app for Windows. The [`efude`](https://crates.io/crates/efude)
crate re-exports all engine crates. The API is not stable before 1.0.

Licensed under MIT or Apache-2.0, at your option
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
