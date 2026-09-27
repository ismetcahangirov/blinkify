//! The picture filters the preview and the export share (#128, #129,
//! ADR-0021): a clip's crop, mapped into the pixels a decoder delivers, and
//! the quarter turns between one orientation and another.
//!
//! A crop is stored in the source's **display** pixels — the picture upright,
//! as the user drew it (ADR-0019). Neither consumer decodes upright: the
//! preview and the export both run FFmpeg with `-noautorotate`, so a portrait
//! phone clip arrives as its coded landscape picture, and a proxy (#26)
//! arrives upright but smaller. [`Crop::filter`] is the one place the
//! rectangle becomes an FFmpeg `crop=`; the preview decoder and the export
//! renderer both call it, so the two can never disagree about which pixels a
//! crop keeps. Getting the mapping wrong crops the wrong corner of every
//! rotated clip, so it is tested here for all four rotations, and against
//! FFmpeg's own autorotation in the engine's integration tests.

use crate::probe::VideoInfo;
use crate::project::crop::CropRect;

/// A picture as a decoder delivers it: its size in its own pixels, and the
/// counter-clockwise turn — 0, 90, 180 or 270 degrees — that shows it
/// upright. The same rule the renderer draws with and FFmpeg's autorotation
/// applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rotation: u32,
}

impl Decoded {
    /// A source's video stream as coded: what a decoder without autorotation
    /// delivers.
    #[must_use]
    pub fn coded(video: &VideoInfo) -> Self {
        Self {
            width: video.width,
            height: video.height,
            rotation: video.rotation,
        }
    }

    /// A picture that is already upright, such as a proxy.
    #[must_use]
    pub fn upright(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rotation: 0,
        }
    }

    /// Counter-clockwise quarter turns to upright: 0 to 3.
    #[must_use]
    pub fn quarter_turns(self) -> u32 {
        quarter_turns(self.rotation)
    }

    /// The picture's size upright.
    #[must_use]
    pub fn display(self) -> (u32, u32) {
        if self.quarter_turns() % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// `degrees`, counter-clockwise, as quarter turns 0 to 3. The probe
/// normalises a display matrix to a multiple of 90; anything else is read as
/// the quarter turn below it.
#[must_use]
pub fn quarter_turns(degrees: u32) -> u32 {
    (degrees % 360).div_euclid(90)
}

/// A clip's crop as one consumer decodes it: the rectangle the user drew, in
/// upright pixels of `picture`, and the picture it is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    pub rect: CropRect,
    pub picture: Decoded,
}

impl Crop {
    /// The rectangle in the pixels the decoder delivers: turned with the
    /// picture, measured from the decoded picture's own top-left corner.
    ///
    /// A picture shown turned counter-clockwise by 90 degrees has its
    /// displayed top edge along its coded right edge, so a rectangle `y`
    /// rows below the displayed top is `y` columns in from the coded right.
    /// The sizes swap on a quarter turn; offsets and sizes on the chroma grid
    /// stay on it, because the coded sizes of a subsampled picture are even.
    #[must_use]
    pub fn decoded(&self) -> CropRect {
        let CropRect {
            x,
            y,
            width,
            height,
        } = self.rect;
        let (coded_width, coded_height) = (self.picture.width, self.picture.height);
        let right = x.saturating_add(width);
        let bottom = y.saturating_add(height);
        match self.picture.quarter_turns() {
            1 => CropRect {
                x: coded_width.saturating_sub(bottom),
                y: x,
                width: height,
                height: width,
            },
            2 => CropRect {
                x: coded_width.saturating_sub(right),
                y: coded_height.saturating_sub(bottom),
                width,
                height,
            },
            3 => CropRect {
                x: y,
                y: coded_height.saturating_sub(right),
                width: height,
                height: width,
            },
            _ => self.rect,
        }
    }

    /// The FFmpeg filter that keeps the rectangle of the decoded picture.
    /// `exact=1`: FFmpeg would otherwise round a subsampled picture's
    /// offsets down to its chroma grid on its own, and the pixels kept would
    /// silently differ from the ones asked for.
    #[must_use]
    pub fn filter(&self) -> String {
        let rect = self.decoded();
        format!(
            "crop={}:{}:{}:{}:exact=1",
            rect.width, rect.height, rect.x, rect.y
        )
    }
}

/// The filter that turns a picture counter-clockwise by `quarter_turns`, or
/// `None` for none. The same turns FFmpeg's autorotation inserts for a
/// display matrix of that angle.
#[must_use]
pub fn turn(quarter_turns: u32) -> Option<&'static str> {
    match quarter_turns % 4 {
        1 => Some("transpose=cclock"),
        2 => Some("hflip,vflip"),
        3 => Some("transpose=clock"),
        _ => None,
    }
}

/// `rect`, drawn in upright pixels of a `source` picture, on a proxy of it
/// that is `proxy` pixels, upright (#26).
///
/// A proxy is smaller than its source, so a source pixel's edge falls
/// between proxy pixels: every edge is rounded **outwards**, then out again
/// to an even pixel — the proxy is 4:2:0 — so the proxy's rectangle covers
/// every source pixel the crop keeps. It is then clamped to the proxy's
/// frame, which rounding outwards can overrun; only there, at the frame's
/// edge, can an odd-sized proxy leave an odd side.
#[must_use]
pub fn onto_proxy(rect: &CropRect, source: (u32, u32), proxy: (u32, u32)) -> CropRect {
    let (source_width, source_height) = source;
    let (proxy_width, proxy_height) = proxy;
    if source_width == 0 || source_height == 0 {
        return CropRect::whole(proxy_width, proxy_height);
    }
    let down = |value: u32, to: u32, of: u32| {
        u32::try_from((u64::from(value) * u64::from(to)).div_euclid(u64::from(of)))
            .unwrap_or(u32::MAX)
    };
    let up = |value: u32, to: u32, of: u32| {
        u32::try_from((u64::from(value) * u64::from(to)).div_ceil(u64::from(of)))
            .unwrap_or(u32::MAX)
    };
    let even_down = |value: u32| value & !1;
    let even_up = |value: u32| value.saturating_add(value & 1);
    let span = |start: u32, length: u32, to: u32, of: u32| {
        let first = even_down(down(start, to, of)).min(to.saturating_sub(2) & !1);
        let last = even_up(up(start.saturating_add(length), to, of)).min(to);
        // Never less than two pixels, whatever the rounding did.
        let last = last.max(first.saturating_add(2).min(to));
        (first, last.saturating_sub(first))
    };
    let (x, width) = span(rect.x, rect.width, proxy_width, source_width);
    let (y, height) = span(rect.y, rect.height, proxy_height, source_height);
    CropRect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: CropRect = CropRect {
        x: 40,
        y: 100,
        width: 200,
        height: 300,
    };

    /// A point of a `decoded` picture, where it lands upright: the rule the
    /// renderer draws with, independent of the mapping under test.
    fn shown(decoded: Decoded, x: u32, y: u32) -> (u32, u32) {
        let (w, h) = (decoded.width, decoded.height);
        match decoded.quarter_turns() {
            1 => (y, w - 1 - x),
            2 => (w - 1 - x, h - 1 - y),
            3 => (h - 1 - y, x),
            _ => (x, y),
        }
    }

    /// The upright rectangle a decoded rectangle shows as.
    fn upright(rect: CropRect, decoded: Decoded) -> CropRect {
        let corners = [
            shown(decoded, rect.x, rect.y),
            shown(decoded, rect.x + rect.width - 1, rect.y + rect.height - 1),
        ];
        let (x0, x1) = (
            corners[0].0.min(corners[1].0),
            corners[0].0.max(corners[1].0),
        );
        let (y0, y1) = (
            corners[0].1.min(corners[1].1),
            corners[0].1.max(corners[1].1),
        );
        CropRect {
            x: x0,
            y: y0,
            width: x1 - x0 + 1,
            height: y1 - y0 + 1,
        }
    }

    #[test]
    fn each_rotation_crops_the_region_drawn_on_the_upright_picture() {
        // A 640 × 360 coded picture at every turn; the display is 360 × 640
        // on a quarter turn, and the rectangle is drawn on that.
        for rotation in [0, 90, 180, 270] {
            let decoded = Decoded {
                width: 640,
                height: 360,
                rotation,
            };
            let rect = if rotation % 180 == 90 {
                RECT
            } else {
                CropRect {
                    x: 40,
                    y: 60,
                    width: 200,
                    height: 240,
                }
            };
            let crop = Crop {
                rect,
                picture: decoded,
            };
            let mapped = crop.decoded();
            assert!(mapped.x + mapped.width <= 640, "{rotation}: {mapped:?}");
            assert!(mapped.y + mapped.height <= 360, "{rotation}: {mapped:?}");
            assert_eq!(upright(mapped, decoded), rect, "{rotation}");
        }
    }

    #[test]
    fn the_filter_is_the_coded_rectangle_verbatim() {
        // Checked against FFmpeg: autorotating `portrait`, then cropping
        // 200 × 300 at (40, 100), gives the same pixels as this filter on the
        // coded picture turned counter-clockwise.
        let portrait = Decoded {
            width: 640,
            height: 360,
            rotation: 90,
        };
        let crop = Crop {
            rect: RECT,
            picture: portrait,
        };
        assert_eq!(crop.filter(), "crop=300:200:240:40:exact=1");
        let turned = Crop {
            picture: Decoded {
                rotation: 270,
                ..portrait
            },
            ..crop
        };
        assert_eq!(turned.filter(), "crop=300:200:100:120:exact=1");
        let upside_down = Crop {
            rect: CropRect {
                x: 40,
                y: 60,
                width: 200,
                height: 240,
            },
            picture: Decoded {
                rotation: 180,
                ..portrait
            },
        };
        assert_eq!(upside_down.filter(), "crop=200:240:400:60:exact=1");
    }

    #[test]
    fn an_even_rectangle_stays_even_through_every_turn() {
        for rotation in [0, 90, 180, 270] {
            let crop = Crop {
                rect: CropRect {
                    x: 656,
                    y: 0,
                    width: 608,
                    height: 720,
                },
                picture: Decoded {
                    width: if rotation % 180 == 90 { 720 } else { 1920 },
                    height: if rotation % 180 == 90 { 1920 } else { 720 },
                    rotation,
                },
            };
            let mapped = crop.decoded();
            for value in [mapped.x, mapped.y, mapped.width, mapped.height] {
                assert_eq!(value % 2, 0, "{rotation}: {mapped:?}");
            }
        }
    }

    #[test]
    fn a_rectangle_past_the_picture_cannot_underflow() {
        let crop = Crop {
            rect: CropRect {
                x: 0,
                y: 700,
                width: 64,
                height: 64,
            },
            picture: Decoded {
                width: 640,
                height: 360,
                rotation: 90,
            },
        };
        assert_eq!(crop.decoded().x, 0);
    }

    #[test]
    fn a_turn_is_the_filter_autorotation_would_insert() {
        assert_eq!(turn(0), None);
        assert_eq!(turn(1), Some("transpose=cclock"));
        assert_eq!(turn(2), Some("hflip,vflip"));
        assert_eq!(turn(3), Some("transpose=clock"));
        assert_eq!(turn(4), None);
        assert_eq!(quarter_turns(270), 3);
        assert_eq!(quarter_turns(360), 0);
        assert_eq!(Decoded::upright(960, 540).display(), (960, 540));
        assert_eq!(
            Decoded {
                width: 1280,
                height: 720,
                rotation: 90
            }
            .display(),
            (720, 1280)
        );
    }

    /// A proxy rectangle back in source pixels, rounded outwards.
    fn back(rect: &CropRect, source: (u32, u32), proxy: (u32, u32)) -> (u32, u32, u32, u32) {
        let scale_down = |v: u32, to: u32, of: u32| (v * to).div_euclid(of);
        let scale_up = |v: u32, to: u32, of: u32| (v * to).div_ceil(of);
        (
            scale_down(rect.x, source.0, proxy.0),
            scale_down(rect.y, source.1, proxy.1),
            scale_up(rect.x + rect.width, source.0, proxy.0),
            scale_up(rect.y + rect.height, source.1, proxy.1),
        )
    }

    #[test]
    fn a_proxy_rectangle_covers_the_crop_and_little_more() {
        let crops = [
            CropRect {
                x: 656,
                y: 0,
                width: 608,
                height: 1080,
            },
            CropRect {
                x: 2,
                y: 2,
                width: 16,
                height: 16,
            },
            CropRect {
                x: 1000,
                y: 500,
                width: 918,
                height: 578,
            },
        ];
        // An ordinary proxy, and odd ones the rounding has to live with.
        for proxy in [(960, 540), (961, 541), (959, 539), (241, 135)] {
            for rect in &crops {
                let mapped = onto_proxy(rect, (1920, 1080), proxy);
                assert!(mapped.x + mapped.width <= proxy.0, "{proxy:?} {mapped:?}");
                assert!(mapped.y + mapped.height <= proxy.1, "{proxy:?} {mapped:?}");
                assert!(mapped.width >= 2 && mapped.height >= 2, "{mapped:?}");
                assert_eq!(mapped.x % 2, 0);
                assert_eq!(mapped.y % 2, 0);
                let (x0, y0, x1, y1) = back(&mapped, (1920, 1080), proxy);
                // It covers every source pixel of the crop …
                assert!(x0 <= rect.x && y0 <= rect.y, "{proxy:?} {rect:?}");
                assert!(x1 >= rect.x + rect.width, "{proxy:?} {rect:?}");
                assert!(y1 >= rect.y + rect.height, "{proxy:?} {rect:?}");
                // … and at most three proxy pixels more on each side: one
                // for the rounding, two for the even grid.
                let slack_x = 3 * 1920_u32.div_ceil(proxy.0) + 1;
                let slack_y = 3 * 1080_u32.div_ceil(proxy.1) + 1;
                assert!(rect.x - x0 <= slack_x, "{proxy:?} {rect:?}");
                assert!(rect.y - y0 <= slack_y, "{proxy:?} {rect:?}");
                assert!(x1 - (rect.x + rect.width) <= slack_x, "{proxy:?} {rect:?}");
                assert!(y1 - (rect.y + rect.height) <= slack_y, "{proxy:?} {rect:?}");
            }
        }
    }

    #[test]
    fn a_crop_that_rounds_past_the_proxy_frame_is_clamped_to_it() {
        // The right and bottom edges round outwards past an odd proxy.
        let rect = CropRect {
            x: 2,
            y: 2,
            width: 1918,
            height: 1078,
        };
        let mapped = onto_proxy(&rect, (1920, 1080), (961, 541));
        assert_eq!(
            mapped,
            CropRect {
                x: 0,
                y: 0,
                width: 961,
                height: 541
            }
        );
        // The whole source is the whole proxy.
        assert_eq!(
            onto_proxy(&CropRect::whole(1920, 1080), (1920, 1080), (960, 540)),
            CropRect::whole(960, 540)
        );
        // A source with no size maps to the whole proxy, not a panic.
        assert_eq!(
            onto_proxy(&rect, (0, 0), (960, 540)),
            CropRect::whole(960, 540)
        );
    }

    #[test]
    fn a_sliver_at_the_far_edge_keeps_two_pixels() {
        let rect = CropRect {
            x: 1904,
            y: 1064,
            width: 16,
            height: 16,
        };
        let mapped = onto_proxy(&rect, (1920, 1080), (241, 135));
        assert!(mapped.width >= 2 && mapped.height >= 2, "{mapped:?}");
        assert!(mapped.x + mapped.width <= 241, "{mapped:?}");
        assert!(mapped.y + mapped.height <= 135, "{mapped:?}");
    }
}
