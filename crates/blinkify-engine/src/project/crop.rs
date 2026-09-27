//! Crop: a rectangle of a clip's picture (#127, ADR-0019).
//!
//! The rectangle is in the source's **display** pixels — after its rotation,
//! as the user sees and draws it. Storing coded pixels would make the same
//! drawing mean different things on a rotated source, so each consumer maps
//! it to the orientation it decodes in (the export renderer, the preview) and
//! is tested there.
//!
//! A crop changes the pixels, so the clip's pictures are re-encoded; nothing
//! else is. Its sound is copied, the clips around it are copied. That is why
//! crop is a rectangle per clip and never a sequence-wide filter.
//!
//! What a rectangle must be is checked here, once, against the source's
//! shape: inside the frame, at least [`MIN_SIZE`] a side, and on the chroma
//! grid — a 4:2:0 picture stores its colour at half resolution, so an odd
//! offset or size either fails in the encoder or shifts the colour by half a
//! sample.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::StreamGeometry;
use crate::probe::ChromaSubsampling;

/// The smallest side a crop may have, in pixels.
pub const MIN_SIZE: u32 = 16;

/// A rectangle of a picture, in display pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropRect {
    /// The whole of a `width` × `height` picture.
    #[must_use]
    pub fn whole(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// The first column right of the rectangle, if it fits in 32 bits.
    #[must_use]
    pub fn right(&self) -> Option<u32> {
        self.x.checked_add(self.width)
    }

    /// The first row below the rectangle, if it fits in 32 bits.
    #[must_use]
    pub fn bottom(&self) -> Option<u32> {
        self.y.checked_add(self.height)
    }

    /// Whether it is all of `shape`'s picture: a crop that crops nothing.
    #[must_use]
    pub fn is_whole(&self, shape: &StreamGeometry) -> bool {
        *self == Self::whole(shape.width, shape.height)
    }
}

/// The step a crop's offsets and sizes must be multiples of, in display
/// pixels, horizontally and vertically: the chroma grid of the picture as it
/// is displayed. A rotation of a quarter turn swaps the axes.
#[must_use]
pub fn alignment(shape: &StreamGeometry) -> (u32, u32) {
    let coded = match shape.chroma {
        Some(ChromaSubsampling::Yuv422) => (2, 1),
        Some(ChromaSubsampling::Yuv444 | ChromaSubsampling::Gray | ChromaSubsampling::Rgb) => {
            (1, 1)
        }
        // Unknown is treated as the common case, which is also the strictest.
        Some(ChromaSubsampling::Yuv420) | None => (2, 2),
    };
    if shape.rotation % 180 == 90 {
        (coded.1, coded.0)
    } else {
        coded
    }
}

/// Why a rectangle cannot crop a picture: each says what to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CropError {
    #[error("the crop runs outside the picture: it must lie within {width} × {height} pixels")]
    OutsideFrame { width: u32, height: u32 },
    #[error("the crop is too small: each side must be at least {MIN_SIZE} pixels")]
    TooSmall,
    #[error(
        "this source stores its colour at half resolution, so the crop's {what} must be a \
         multiple of {step} pixels; {value} is not"
    )]
    OffChromaGrid {
        what: &'static str,
        step: u32,
        value: u32,
    },
}

/// Check that `rect` can crop a picture of `shape`.
///
/// # Errors
///
/// See [`CropError`]; the first problem found.
pub fn check(rect: &CropRect, shape: &StreamGeometry) -> Result<(), CropError> {
    let inside = rect.right().is_some_and(|right| right <= shape.width)
        && rect.bottom().is_some_and(|bottom| bottom <= shape.height);
    if !inside {
        return Err(CropError::OutsideFrame {
            width: shape.width,
            height: shape.height,
        });
    }
    if rect.width < MIN_SIZE || rect.height < MIN_SIZE {
        return Err(CropError::TooSmall);
    }
    let (across, down) = alignment(shape);
    for (what, step, value) in [
        ("left edge", across, rect.x),
        ("width", across, rect.width),
        ("top edge", down, rect.y),
        ("height", down, rect.height),
    ] {
        if value % step != 0 {
            return Err(CropError::OffChromaGrid { what, step, value });
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::probe::Rational;

    fn shape(
        width: u32,
        height: u32,
        chroma: Option<ChromaSubsampling>,
        rotation: u32,
    ) -> StreamGeometry {
        StreamGeometry {
            width,
            height,
            frame_rate: Rational { num: 30, den: 1 },
            pixel_aspect: Rational { num: 1, den: 1 },
            variable_frame_rate: false,
            hdr: false,
            chroma,
            rotation,
        }
    }

    fn hd() -> StreamGeometry {
        shape(1920, 1080, Some(ChromaSubsampling::Yuv420), 0)
    }

    #[test]
    fn a_rectangle_inside_the_frame_on_the_chroma_grid_is_accepted() {
        let rect = CropRect {
            x: 656,
            y: 0,
            width: 608,
            height: 1080,
        };
        assert_eq!(check(&rect, &hd()), Ok(()));
        assert!(CropRect::whole(1920, 1080).is_whole(&hd()));
        assert!(!rect.is_whole(&hd()));
    }

    #[test]
    fn a_rectangle_outside_the_frame_is_refused() {
        for rect in [
            CropRect {
                x: 1900,
                y: 0,
                width: 32,
                height: 32,
            },
            CropRect {
                x: 0,
                y: 1080,
                width: 32,
                height: 32,
            },
            CropRect {
                x: u32::MAX,
                y: 0,
                width: 32,
                height: 32,
            },
        ] {
            assert_eq!(
                check(&rect, &hd()),
                Err(CropError::OutsideFrame {
                    width: 1920,
                    height: 1080
                }),
                "{rect:?}"
            );
        }
    }

    #[test]
    fn a_rectangle_smaller_than_the_minimum_is_refused() {
        let rect = CropRect {
            x: 0,
            y: 0,
            width: 14,
            height: 400,
        };
        assert_eq!(check(&rect, &hd()), Err(CropError::TooSmall));
    }

    #[test]
    fn an_odd_offset_or_size_on_a_4_2_0_source_is_refused_with_what_to_change() {
        let odd = CropRect {
            x: 101,
            y: 0,
            width: 600,
            height: 600,
        };
        let error = check(&odd, &hd()).expect_err("refused");
        assert_eq!(
            error,
            CropError::OffChromaGrid {
                what: "left edge",
                step: 2,
                value: 101
            }
        );
        assert!(error.to_string().contains("multiple of 2 pixels; 101"));
        let odd_height = CropRect {
            height: 599,
            ..CropRect { x: 100, ..odd }
        };
        assert!(matches!(
            check(&odd_height, &hd()),
            Err(CropError::OffChromaGrid { what: "height", .. })
        ));
    }

    #[test]
    fn the_grid_follows_the_subsampling_and_the_rotation() {
        let odd_x = CropRect {
            x: 1,
            y: 0,
            width: 64,
            height: 64,
        };
        let odd_y = CropRect {
            x: 0,
            y: 1,
            width: 64,
            height: 64,
        };
        let full = shape(1920, 1080, Some(ChromaSubsampling::Yuv444), 0);
        assert_eq!(check(&odd_x, &full), Ok(()));
        assert_eq!(check(&odd_y, &full), Ok(()));
        // 4:2:2 halves colour across only; a quarter turn makes that down.
        let upright = shape(1920, 1080, Some(ChromaSubsampling::Yuv422), 0);
        assert!(check(&odd_x, &upright).is_err());
        assert_eq!(check(&odd_y, &upright), Ok(()));
        let turned = shape(1080, 1920, Some(ChromaSubsampling::Yuv422), 90);
        assert_eq!(check(&odd_x, &turned), Ok(()));
        assert!(check(&odd_y, &turned).is_err());
        // Unknown subsampling is held to the strictest grid.
        let unknown = shape(1920, 1080, None, 0);
        assert!(check(&odd_x, &unknown).is_err());
        assert!(check(&odd_y, &unknown).is_err());
    }
}
