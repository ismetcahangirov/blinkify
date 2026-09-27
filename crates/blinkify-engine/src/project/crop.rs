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

/// A shape a crop can be fitted to (#130): the source's own, or one of the
/// four the reframe (#132) offers. A rectangle of any other shape is "free":
/// what the sides are when they match no preset, not a preset itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Aspect {
    /// The whole picture: no crop.
    #[serde(rename = "source")]
    Source,
    #[serde(rename = "16:9")]
    Widescreen,
    #[serde(rename = "9:16")]
    Vertical,
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "4:5")]
    Portrait,
}

impl Aspect {
    /// Every preset, in the order the controls list them.
    pub const PRESETS: [Self; 5] = [
        Self::Source,
        Self::Widescreen,
        Self::Vertical,
        Self::Square,
        Self::Portrait,
    ];

    /// Width to height on screen, or `None` for the source's own.
    #[must_use]
    pub fn ratio(self) -> Option<(u32, u32)> {
        match self {
            Self::Source => None,
            Self::Widescreen => Some((16, 9)),
            Self::Vertical => Some((9, 16)),
            Self::Square => Some((1, 1)),
            Self::Portrait => Some((4, 5)),
        }
    }

    /// `9:16`, or `source`: what the user reads.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Widescreen => "16:9",
            Self::Vertical => "9:16",
            Self::Square => "1:1",
            Self::Portrait => "4:5",
        }
    }
}

impl std::fmt::Display for Aspect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The largest multiple of `step` not above `value`.
fn floor_to(value: u32, step: u32) -> u32 {
    value - value % step.max(1)
}

/// One side of a rectangle of a given shape from the other, on a picture's
/// grid. The pixel aspect is part of it: a rectangle's shape on screen is its
/// width times the pixel aspect, over its height.
struct Sides {
    /// Pixel width over pixel height, as a fraction.
    num: u128,
    den: u128,
    across: u32,
    down: u32,
}

impl Sides {
    fn of(shape: &StreamGeometry, (width, height): (u32, u32)) -> Self {
        let pixel = shape.pixel_aspect;
        // A pixel aspect that is not a ratio is taken as square, as the
        // sequence settings take it.
        let (pixel_num, pixel_den) = if pixel.num > 0 && pixel.den > 0 {
            (pixel.num.unsigned_abs(), pixel.den.unsigned_abs())
        } else {
            (1, 1)
        };
        let (across, down) = alignment(shape);
        Self {
            num: u128::from(width) * u128::from(pixel_den),
            den: u128::from(height) * u128::from(pixel_num),
            across,
            down,
        }
    }

    /// `value × num / den`, to the nearest multiple of `step`; a half rounds
    /// up, so 607.5 on a grid of 2 is 608.
    fn nearest(value: u32, num: u128, den: u128, step: u32) -> u32 {
        let step = u128::from(step.max(1));
        let steps = (2 * u128::from(value) * num + den * step).div_euclid(2 * den * step);
        u32::try_from(steps * step).unwrap_or(u32::MAX)
    }

    fn width_for(&self, height: u32) -> u32 {
        Self::nearest(height, self.num, self.den, self.across)
    }

    fn height_for(&self, width: u32) -> u32 {
        Self::nearest(width, self.den, self.num, self.down)
    }
}

/// The largest rectangle of `aspect` centred on a picture of `shape`, on its
/// chroma grid: the one place a preset is rounded (#130), so the numbers the
/// controls show are the numbers stored, and the reframe (#132) and the
/// preview's handles (#131) fit the same rectangle.
///
/// A picture already of that shape, to within the grid, is returned whole:
/// cropping it would remove rounding, not picture, and cost a re-encode for
/// nothing. `Source` is always the whole picture. `None` when the picture is
/// too small to hold the shape at [`MIN_SIZE`] a side.
#[must_use]
pub fn centred(shape: &StreamGeometry, aspect: Aspect) -> Option<CropRect> {
    let whole = CropRect::whole(shape.width, shape.height);
    let Some(ratio) = aspect.ratio() else {
        return Some(whole);
    };
    let sides = Sides::of(shape, ratio);
    let widest = floor_to(shape.width, sides.across);
    let tallest = floor_to(shape.height, sides.down);
    if sides.width_for(shape.height) >= widest && sides.height_for(shape.width) >= tallest {
        return Some(whole);
    }
    let (width, height) = if sides.width_for(tallest) <= widest {
        (sides.width_for(tallest), tallest)
    } else {
        (widest, sides.height_for(widest).min(tallest))
    };
    if width < MIN_SIZE || height < MIN_SIZE {
        return None;
    }
    let rect = CropRect {
        x: floor_to((shape.width - width).div_euclid(2), sides.across),
        y: floor_to((shape.height - height).div_euclid(2), sides.down),
        width,
        height,
    };
    debug_assert_eq!(check(&rect, shape), Ok(()));
    Some(rect)
}

/// Whether `rect` has the shape of `aspect` on a picture of `shape`, to
/// within the grid, wherever it sits and however large it is. The reframe
/// (#132) keeps a crop that already has the shape it asks for, where the user
/// put it.
#[must_use]
pub fn has_aspect(rect: &CropRect, shape: &StreamGeometry, aspect: Aspect) -> bool {
    match aspect.ratio() {
        None => rect.is_whole(shape),
        Some(ratio) => {
            let sides = Sides::of(shape, ratio);
            sides.width_for(rect.height) == rect.width
                || sides.height_for(rect.width) == rect.height
        }
    }
}

/// A preset and the rectangle it fits on one picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PresetRect {
    pub aspect: Aspect,
    pub rect: CropRect,
}

/// What the crop controls need to know about a source's picture (#130): its
/// size as displayed, the grid the sides keep to, and each preset's
/// rectangle, all worked out here so the controls work nothing out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CropFrame {
    /// Display pixels, rotation applied.
    pub width: u32,
    pub height: u32,
    /// The step of the left edge and the width.
    pub across: u32,
    /// The step of the top edge and the height.
    pub down: u32,
    /// The smallest side a crop may have.
    pub min_size: u32,
    /// Every preset the picture can hold, in [`Aspect::PRESETS`] order.
    pub presets: Vec<PresetRect>,
}

impl CropFrame {
    #[must_use]
    pub fn of(shape: &StreamGeometry) -> Self {
        let (across, down) = alignment(shape);
        Self {
            width: shape.width,
            height: shape.height,
            across,
            down,
            min_size: MIN_SIZE,
            presets: Aspect::PRESETS
                .iter()
                .filter_map(|&aspect| {
                    centred(shape, aspect).map(|rect| PresetRect { aspect, rect })
                })
                .collect(),
        }
    }
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

    fn rect(x: u32, y: u32, width: u32, height: u32) -> CropRect {
        CropRect {
            x,
            y,
            width,
            height,
        }
    }

    fn preset(shape: &StreamGeometry, aspect: Aspect) -> CropRect {
        centred(shape, aspect).expect("the picture holds it")
    }

    #[test]
    fn a_preset_on_a_landscape_picture_is_its_largest_centred_rectangle() {
        let hd = hd();
        // #130's acceptance: 9:16 on 1920 × 1080 is 608 × 1080 from 656.
        assert_eq!(preset(&hd, Aspect::Vertical), rect(656, 0, 608, 1080));
        assert_eq!(preset(&hd, Aspect::Square), rect(420, 0, 1080, 1080));
        assert_eq!(preset(&hd, Aspect::Portrait), rect(528, 0, 864, 1080));
        // Already 16:9, and the source's own: the whole picture, no crop.
        assert!(preset(&hd, Aspect::Widescreen).is_whole(&hd));
        assert!(preset(&hd, Aspect::Source).is_whole(&hd));
        for aspect in Aspect::PRESETS {
            let fitted = preset(&hd, aspect);
            assert_eq!(check(&fitted, &hd), Ok(()), "{aspect}");
            assert!(has_aspect(&fitted, &hd, aspect), "{aspect}");
        }
    }

    #[test]
    fn a_preset_on_a_portrait_phone_clip_is_fitted_to_the_picture_as_displayed() {
        // A phone held upright: coded 1920 × 1080 with a quarter turn, so
        // displayed 1080 × 1920.
        let phone = shape(1080, 1920, Some(ChromaSubsampling::Yuv420), 90);
        assert_eq!(preset(&phone, Aspect::Widescreen), rect(0, 656, 1080, 608));
        assert_eq!(preset(&phone, Aspect::Square), rect(0, 420, 1080, 1080));
        assert_eq!(preset(&phone, Aspect::Portrait), rect(0, 284, 1080, 1350));
        assert!(preset(&phone, Aspect::Vertical).is_whole(&phone));
        // 4:2:2 halves the colour across the coded picture, which is down
        // on the turned one: the offsets keep to that.
        let turned = shape(1080, 1920, Some(ChromaSubsampling::Yuv422), 90);
        assert_eq!(alignment(&turned), (1, 2));
        let square = preset(&turned, Aspect::Square);
        assert_eq!(square, rect(0, 420, 1080, 1080));
        assert_eq!(check(&square, &turned), Ok(()));
    }

    #[test]
    fn a_preset_on_a_square_picture() {
        let square = shape(1080, 1080, Some(ChromaSubsampling::Yuv420), 0);
        assert!(preset(&square, Aspect::Square).is_whole(&square));
        assert_eq!(preset(&square, Aspect::Widescreen), rect(0, 236, 1080, 608));
        assert_eq!(preset(&square, Aspect::Vertical), rect(236, 0, 608, 1080));
        assert_eq!(preset(&square, Aspect::Portrait), rect(108, 0, 864, 1080));
    }

    #[test]
    fn a_preset_on_an_odd_picture_keeps_to_the_grid_and_inside_it() {
        let odd = shape(1919, 1079, Some(ChromaSubsampling::Yuv420), 0);
        let vertical = preset(&odd, Aspect::Vertical);
        assert_eq!(vertical, rect(656, 0, 606, 1078));
        // Within rounding of 16:9 already: not cropped for one pixel.
        assert!(preset(&odd, Aspect::Widescreen).is_whole(&odd));
        // The source's own is the whole picture, odd or not.
        assert!(preset(&odd, Aspect::Source).is_whole(&odd));
        for aspect in [Aspect::Vertical, Aspect::Square, Aspect::Portrait] {
            let fitted = preset(&odd, aspect);
            assert_eq!(check(&fitted, &odd), Ok(()), "{aspect}: {fitted:?}");
        }
        // Codec padding is picture, not rounding: 1088 rows crop to 1080.
        let padded = shape(1920, 1088, Some(ChromaSubsampling::Yuv420), 0);
        assert_eq!(preset(&padded, Aspect::Widescreen), rect(0, 4, 1920, 1080));
        // 4:4:4 has no grid: odd offsets and sizes are exact.
        let full = shape(1001, 1001, Some(ChromaSubsampling::Yuv444), 0);
        assert_eq!(preset(&full, Aspect::Widescreen), rect(0, 219, 1001, 563));
    }

    #[test]
    fn a_preset_follows_the_pixel_aspect() {
        // DV widescreen: 720 × 576 stored, 64:45 pixels, 16:9 on screen.
        let mut dv = shape(720, 576, Some(ChromaSubsampling::Yuv420), 0);
        dv.pixel_aspect = Rational { num: 64, den: 45 };
        assert!(preset(&dv, Aspect::Widescreen).is_whole(&dv));
        // A square on screen is 405 stored pixels wide, rounded to 406.
        assert_eq!(preset(&dv, Aspect::Square), rect(156, 0, 406, 576));
    }

    #[test]
    fn a_picture_too_small_for_a_shape_holds_no_preset_of_it() {
        let sliver = shape(16, 64, Some(ChromaSubsampling::Yuv420), 0);
        assert_eq!(centred(&sliver, Aspect::Widescreen), None);
        let frame = CropFrame::of(&sliver);
        assert!(frame.presets.iter().all(|p| p.aspect != Aspect::Widescreen));
        assert_eq!(
            frame.presets.first().map(|p| p.aspect),
            Some(Aspect::Source)
        );
    }

    #[test]
    fn a_frame_carries_the_grid_and_every_preset_the_controls_offer() {
        let frame = CropFrame::of(&hd());
        assert_eq!((frame.width, frame.height), (1920, 1080));
        assert_eq!((frame.across, frame.down, frame.min_size), (2, 2, 16));
        assert_eq!(
            frame.presets.iter().map(|p| p.aspect).collect::<Vec<_>>(),
            Aspect::PRESETS.to_vec()
        );
        let text = serde_json::to_string(&Aspect::Vertical).expect("json");
        assert_eq!(text, "\"9:16\"");
    }

    #[test]
    fn a_crop_has_an_aspect_wherever_it_sits() {
        let hd = hd();
        assert!(has_aspect(&rect(0, 0, 608, 1080), &hd, Aspect::Vertical));
        assert!(has_aspect(&rect(100, 200, 304, 540), &hd, Aspect::Vertical));
        assert!(!has_aspect(&rect(0, 0, 700, 1080), &hd, Aspect::Vertical));
        assert!(has_aspect(
            &CropRect::whole(1920, 1080),
            &hd,
            Aspect::Source
        ));
        assert!(!has_aspect(&rect(0, 0, 1080, 1080), &hd, Aspect::Source));
    }
}
