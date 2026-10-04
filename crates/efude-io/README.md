# efude-io

File formats: the `.efude` document format, PSD import and export, PNG / JPEG / BMP / GIF import and PNG / JPEG export.

PSD layer-mask import honors bit 0 (coordinates relative to the layer), bit 1
(disabled mask), and bit 2 (invert when blending). Bit 2 is obsolete in the
[Adobe PSD specification](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/),
but files that set it are decoded by inverting both the mask samples and the
default outside its rectangle. Disabled masks stay disabled even when inverted.
Declared mask density or feather parameters (bit 4) are checked against their
mask-data section. Active masks with density other than 255 or nonzero feather
use the merged image with a warning, since these effects are not reproduced.
Disabled masks and neutral parameters preserve the editable layers. Truncated
parameter data and invalid feather values are rejected.

Part of the engine of [Efude](https://github.com/852wa/Efude), an open-source
painting and manga app for Windows. The [`efude`](https://crates.io/crates/efude)
crate re-exports all engine crates. The API is not stable before 1.0.

Licensed under MIT or Apache-2.0, at your option
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
