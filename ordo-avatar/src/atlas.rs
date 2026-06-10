//! Avatar sprite atlas — descriptor types + phoneme→viseme
//! grouping. The driver emits **intent** (`Phoneme::Ae`,
//! `Expression::Speaking`, `GlitchLevel::Light`); this module
//! defines the contract the renderer uses to translate intent
//! into atlas cell coordinates.
//!
//! ## Why split intent from cells
//!
//! 40 phonemes is too many for the eye to distinguish at 30Hz.
//! Standard practice (Preston Blair, Hanna-Barbera, modern
//! lip-sync engines) folds phonemes down to ~8 visemes. The
//! grouping is a renderer-side decision — the protocol carries
//! the raw phoneme so a future renderer with a richer atlas
//! (12 visemes? 16?) can re-group without a wire-format break.
//!
//! ## Descriptor file
//!
//! The descriptor is serialized to `avatar.json` next to the
//! atlas PNGs in the renderer's static asset directory. The
//! renderer (step 6) loads the JSON at boot and indexes into the
//! atlas PNGs by `cell_width × cell_index` along the X axis.
//! Cells in v0.1 are arranged in a single horizontal row per
//! PNG — keeps the math trivial and keeps each PNG small enough
//! to ship with the WebView2 bundle.
//!
//! ## What stays stable
//!
//! - The viseme grouping (`phoneme_to_viseme`).
//! - The cell indices for [`Expression`] and [`GlitchLevel`]
//!   (their enum order).
//! - The PNG layout (single row, fixed cell size).
//!
//! When a real artist replaces the placeholder PNGs, only the
//! pixels change — the descriptor + this code stay put.

use ordo_protocol::{Expression, GlitchLevel, Phoneme};
use serde::{Deserialize, Serialize};

/// Number of mouth visemes the atlas carries. Standard
/// 8-viseme grouping — see [`phoneme_to_viseme`].
pub const MOUTH_VISEME_COUNT: u8 = 8;

/// Number of expression cells in the atlas. One per
/// [`Expression`] variant — including the v0.1-reserved
/// `Amused` and `Glitched` so a future driver that emits them
/// has a cell to land on.
pub const EXPRESSION_CELL_COUNT: u8 = 6;

/// Number of glitch overlay cells. `GlitchLevel::None` has no
/// cell (no overlay is composited); only `Light` and `Heavy`.
pub const GLITCH_CELL_COUNT: u8 = 2;

/// Top-level atlas descriptor. One JSON file per renderer
/// asset bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AtlasDescriptor {
    /// Wire-format version. Bump on any breaking change to
    /// cell layout, viseme grouping, or PNG path conventions.
    /// Renderers should refuse to load mismatched versions.
    pub version: u32,
    /// Width of one cell in pixels. All atlases share the same
    /// cell size so the renderer doesn't need three sets of
    /// blit math.
    pub cell_width: u32,
    /// Height of one cell in pixels.
    pub cell_height: u32,
    pub mouth: AtlasLayer,
    pub expression: AtlasLayer,
    pub glitch: AtlasLayer,
}

impl AtlasDescriptor {
    /// Build the default v0.1 descriptor. Cell paths point at
    /// `avatar/<layer>.png` relative to the renderer's static
    /// asset root; bundlers (Vite) resolve these to URLs.
    pub fn v1(cell_size: u32) -> Self {
        Self {
            version: 1,
            cell_width: cell_size,
            cell_height: cell_size,
            mouth: AtlasLayer {
                atlas_path: "avatar/mouth.png".into(),
                cell_count: MOUTH_VISEME_COUNT,
            },
            expression: AtlasLayer {
                atlas_path: "avatar/expression.png".into(),
                cell_count: EXPRESSION_CELL_COUNT,
            },
            glitch: AtlasLayer {
                atlas_path: "avatar/glitch.png".into(),
                cell_count: GLITCH_CELL_COUNT,
            },
        }
    }
}

/// One PNG-backed layer. Cells run left-to-right in a single
/// horizontal row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AtlasLayer {
    /// Path to the PNG, relative to the renderer's static
    /// asset root.
    pub atlas_path: String,
    /// Number of cells in the row. Renderers should bounds-
    /// check before indexing.
    pub cell_count: u8,
}

/// Map a [`Phoneme`] to its viseme cell index `[0, 8)`.
///
/// Standard 8-viseme grouping, biased toward visual
/// distinctness rather than linguistic accuracy:
///
/// | Viseme | Name           | Phonemes                                  |
/// |--------|----------------|-------------------------------------------|
/// | 0      | Rest / closed  | `Rest`                                    |
/// | 1      | Bilabial       | `M`, `B`, `P`                             |
/// | 2      | Labiodental    | `F`, `V`                                  |
/// | 3      | Alveolar       | `T`, `D`, `N`, `S`, `Z`, `L`, `Th`, `Dh`  |
/// | 4      | Velar/glottal  | `K`, `G`, `Ng`, `Hh`                      |
/// | 5      | Open vowel     | `Aa`, `Ae`, `Ah`, `Ay`, `Eh`, `Ey`, `Ih`, `Iy`, `Er` |
/// | 6      | Round vowel    | `Ao`, `Ow`, `Oy`, `Aw`, `Uh`, `Uw`        |
/// | 7      | Glide/affric.  | `R`, `Y`, `W`, `Ch`, `Jh`, `Sh`, `Zh`     |
///
/// A future richer atlas (12+ visemes for tighter sync) can
/// add cells and re-group — the protocol's `Phoneme` enum is
/// the stable currency.
pub fn phoneme_to_viseme(phoneme: Phoneme) -> u8 {
    use Phoneme::*;
    match phoneme {
        Rest => 0,
        M | B | P => 1,
        F | V => 2,
        T | D | N | S | Z | L | Th | Dh => 3,
        K | G | Ng | Hh => 4,
        Aa | Ae | Ah | Ay | Eh | Ey | Ih | Iy | Er => 5,
        Ao | Ow | Oy | Aw | Uh | Uw => 6,
        R | Y | W | Ch | Jh | Sh | Zh => 7,
    }
}

/// Map an [`Expression`] to its cell index `[0, 6)`. Order
/// matches the enum declaration so the atlas authoring is
/// "left-to-right in declaration order."
pub fn expression_to_cell(expression: Expression) -> u8 {
    match expression {
        Expression::Neutral => 0,
        Expression::Speaking => 1,
        Expression::Thinking => 2,
        Expression::Alarmed => 3,
        Expression::Amused => 4,
        Expression::Glitched => 5,
    }
}

/// Map a [`GlitchLevel`] to its overlay cell index, or `None`
/// when no overlay should be composited.
pub fn glitch_to_cell(glitch: GlitchLevel) -> Option<u8> {
    match glitch {
        GlitchLevel::None => None,
        GlitchLevel::Light => Some(0),
        GlitchLevel::Heavy => Some(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_phoneme_maps_to_viseme_zero() {
        assert_eq!(phoneme_to_viseme(Phoneme::Rest), 0);
    }

    #[test]
    fn every_phoneme_lands_in_range() {
        // Sanity: every phoneme variant produces a viseme in
        // `[0, MOUTH_VISEME_COUNT)`. If a future variant is
        // added without updating `phoneme_to_viseme`, the
        // match would fail to compile — but if someone adds a
        // viseme number ≥ 8, this catches it.
        for phoneme in [
            Phoneme::Aa, Phoneme::Ae, Phoneme::Ah, Phoneme::Ao, Phoneme::Aw,
            Phoneme::Ay, Phoneme::Eh, Phoneme::Er, Phoneme::Ey, Phoneme::Ih,
            Phoneme::Iy, Phoneme::Ow, Phoneme::Oy, Phoneme::Uh, Phoneme::Uw,
            Phoneme::B, Phoneme::Ch, Phoneme::D, Phoneme::Dh, Phoneme::F,
            Phoneme::G, Phoneme::Hh, Phoneme::Jh, Phoneme::K, Phoneme::L,
            Phoneme::M, Phoneme::N, Phoneme::Ng, Phoneme::P, Phoneme::R,
            Phoneme::S, Phoneme::Sh, Phoneme::T, Phoneme::Th, Phoneme::V,
            Phoneme::W, Phoneme::Y, Phoneme::Z, Phoneme::Zh, Phoneme::Rest,
        ] {
            assert!(
                phoneme_to_viseme(phoneme) < MOUTH_VISEME_COUNT,
                "{:?} maps out of range",
                phoneme
            );
        }
    }

    #[test]
    fn bilabials_share_a_viseme() {
        let m = phoneme_to_viseme(Phoneme::M);
        assert_eq!(phoneme_to_viseme(Phoneme::B), m);
        assert_eq!(phoneme_to_viseme(Phoneme::P), m);
    }

    #[test]
    fn expression_cells_match_declaration_order() {
        assert_eq!(expression_to_cell(Expression::Neutral), 0);
        assert_eq!(expression_to_cell(Expression::Glitched), 5);
    }

    #[test]
    fn glitch_none_has_no_cell() {
        assert_eq!(glitch_to_cell(GlitchLevel::None), None);
        assert_eq!(glitch_to_cell(GlitchLevel::Light), Some(0));
        assert_eq!(glitch_to_cell(GlitchLevel::Heavy), Some(1));
    }

    #[test]
    fn descriptor_roundtrips_through_json() {
        let descriptor = AtlasDescriptor::v1(128);
        let json = serde_json::to_string(&descriptor).unwrap();
        let back: AtlasDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(descriptor, back);
        // Mouth path is the canonical one expected by the
        // renderer; lock it in.
        assert_eq!(descriptor.mouth.atlas_path, "avatar/mouth.png");
    }
}
