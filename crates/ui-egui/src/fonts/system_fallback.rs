//! A character none of SolidCraft's faces has (bundled, chosen, or found for a known script in
//! [`super`]) is drawn with an installed font that has it, not as an empty box (#20005).
//!
//! The installed fonts are listed, and the characters each one draws read from it, once on a
//! background thread ([`fontdb`]). Only then is [`SystemFallback`] added to the context, so the
//! window never waits for it: adding a provider makes egui ask again for the characters it
//! could not draw, and repaint. egui asks [`SystemFallback::font_for`] at most once per family
//! and character, and the font it gets is appended to that family's fallback chain, so the rest
//! of the script follows from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use egui::{FallbackRequest, FontData, FontFamily, FontInsert, FontPriority, InsertFontFamily};
use skrifa::MetadataProvider as _;

/// An installed face egui can draw, and the characters it has, as sorted, disjoint ranges.
struct Face {
    id: fontdb::ID,
    chars: Box<[(u32, u32)]>,
}

impl Face {
    fn has(&self, c: char) -> bool {
        let c = u32::from(c);
        let i = self.chars.partition_point(|&(_, last)| last < c);
        self.chars.get(i).is_some_and(|&(first, _)| first <= c)
    }
}

/// An [`egui::FontProvider`] that finds a character in the installed fonts.
pub(crate) struct SystemFallback {
    db: fontdb::Database,
    faces: Vec<Face>,
    /// The face found for a character, for a monospaced family or not.
    found: Mutex<HashMap<(char, bool), Option<fontdb::ID>>>,
}

impl SystemFallback {
    /// Over the faces of `db`, reading the characters each one has. Slow: not on the UI thread.
    fn over(db: fontdb::Database) -> Self {
        let faces = db.faces().filter_map(|f| Some(Face { id: f.id, chars: chars_of(&db, f.id)? })).collect();
        Self { db, faces, found: Mutex::new(HashMap::new()) }
    }

    /// The installed face that draws `c` best: one of the family's kind (monospaced or not),
    /// upright, normal width, nearest the regular weight, a Noto face before others, then by name
    /// so the choice does not depend on the order the folders were read in.
    fn best_face(&self, c: char, monospace: bool) -> Option<&fontdb::FaceInfo> {
        let faces = self.faces.iter().filter(|f| f.has(c)).filter_map(|f| self.db.face(f.id));
        faces.min_by_key(|f| {
            let name = f.families.first().map(|(n, _)| n.as_str()).unwrap_or_default();
            (
                f.monospaced != monospace,
                f.style != fontdb::Style::Normal,
                f.stretch != fontdb::Stretch::Normal,
                f.weight.0.abs_diff(fontdb::Weight::NORMAL.0),
                !name.starts_with("Noto"),
                name.to_owned(),
                f.index,
            )
        })
    }
}

impl egui::FontProvider for SystemFallback {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        let monospace = matches!(request.family, FontFamily::Monospace);
        let id = *self
            .found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((c, monospace))
            .or_insert_with(|| self.best_face(c, monospace).map(|f| f.id));
        let face = self.db.face(id?)?;
        let bytes = self.db.with_face_data(face.id, |data, _| data.to_vec())?;
        let name = format!("system:{}:{}", face.post_script_name, face.index);
        log::debug!("{name} draws {c:?}");
        let mut data = FontData::from_owned(bytes);
        data.index = face.index;
        Some(FontInsert::new(&name, data, vec![InsertFontFamily { family: request.family.clone(), priority: FontPriority::Lowest }]))
    }
}

/// The characters face `id` has a glyph for, as sorted, disjoint ranges.
fn chars_of(db: &fontdb::Database, id: fontdb::ID) -> Option<Box<[(u32, u32)]>> {
    db.with_face_data(id, chars_in)?
}

/// The characters face `index` of `data` maps to a glyph, as egui reads them (skrifa's character
/// map: the Unicode subtable of format 4 or 12 it picks, up to `char::MAX` and the face's glyph
/// count, so a malformed range cannot run away), as sorted, disjoint ranges. `None` when it maps
/// none, as a last-resort face such as macOS's `LastResort.otf` (whose format 13 subtable egui does
/// not read), or egui cannot draw it: without epaint's `color_fonts`, which SolidCraft does not
/// turn on, it draws outlines only, so a face of bitmaps alone (Noto Color Emoji) is left out.
fn chars_in(data: &[u8], index: u32) -> Option<Box<[(u32, u32)]>> {
    let font = skrifa::FontRef::from_index(data, index).ok()?;
    let drawable = [b"glyf", b"CFF ", b"CFF2"].into_iter().any(|t| font.table_data(skrifa::Tag::new(t)).is_some());
    if !drawable {
        return None;
    }
    let mut chars: Vec<u32> = font.charmap().mappings().filter(|(_, g)| g.to_u32() != 0).map(|(c, _)| c).collect();
    chars.sort_unstable();
    chars.dedup();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for c in chars {
        match ranges.last_mut() {
            Some((_, last)) if *last + 1 == c => *last = c,
            _ => ranges.push((c, c)),
        }
    }
    (!ranges.is_empty()).then(|| ranges.into_boxed_slice())
}

/// Asks the installed fonts for any character the context's fonts lack, once they are listed.
/// Once per context.
pub(crate) fn install(ctx: &egui::Context) {
    let id = egui::Id::unique("solidcraft_system_font_fallback");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, true));
    let ctx = ctx.clone();
    let listing = std::thread::Builder::new().name("font-fallback".into()).spawn(move || {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        // In a Flatpak sandbox `/usr/share/fonts` holds the runtime's fonts; the host's system,
        // local and user fonts are mounted here. A folder that is missing is skipped.
        #[cfg(target_os = "linux")]
        for dir in ["/run/host/fonts", "/run/host/local-fonts", "/run/host/user-fonts"] {
            db.load_fonts_dir(dir);
        }
        // fontdb reads `~/.local/share/fonts`, not `$XDG_DATA_HOME/fonts` when that is set
        // elsewhere.
        #[cfg(target_os = "linux")]
        if let Some(data) = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).filter(|d| d.is_absolute()) {
            db.load_fonts_dir(data.join("fonts"));
        }
        // Repaints, and egui asks again for the characters it missed.
        ctx.add_font_provider(Arc::new(SystemFallback::over(db)));
    });
    if let Err(error) = listing {
        log::warn!("no installed fonts for missing characters: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::FontProvider as _;

    /// Latin capital letter schwa: in Ubuntu Light, not in Hack.
    const SCHWA: char = 'Ə';

    /// egui's two default faces, Hack (monospaced, regular) and Ubuntu Light (proportional, light).
    fn defaults() -> fontdb::Database {
        let mut db = fontdb::Database::new();
        let defs = egui::FontDefinitions::default();
        for name in ["Hack", "Ubuntu-Light"] {
            if let Some(font) = defs.font_data.get(name) {
                db.load_font_data(font.bytes().to_vec());
            }
        }
        db
    }

    fn family_of(p: &SystemFallback, c: char, monospace: bool) -> Option<String> {
        p.best_face(c, monospace).and_then(|f| f.families.first()).map(|(n, _)| n.clone())
    }

    #[test]
    fn a_family_takes_a_face_of_its_kind_and_only_one_that_has_the_character() {
        let p = SystemFallback::over(defaults());
        assert_eq!(p.faces.len(), 2);
        assert_eq!(family_of(&p, 'a', true).as_deref(), Some("Hack"));
        assert_eq!(family_of(&p, 'a', false).as_deref(), Some("Ubuntu"));
        // Only Ubuntu has it, so even a monospaced family takes it.
        assert_eq!(family_of(&p, SCHWA, true).as_deref(), Some("Ubuntu"));
        assert_eq!(family_of(&p, '한', false), None);
    }

    #[test]
    fn a_face_has_the_characters_of_its_ranges_only() {
        let face = Face { id: fontdb::ID::dummy(), chars: Box::new([(0x41, 0x5a), (0x61, 0x7a), (0x259, 0x259)]) };
        for c in ['A', 'Z', 'a', 'm', 'z', 'ə'] {
            assert!(face.has(c), "{c:?}");
        }
        for c in ['\0', '@', '[', '`', '{', 'Ə', '한', char::MAX] {
            assert!(!face.has(c), "{c:?}");
        }
    }

    #[test]
    fn a_character_is_looked_up_once_per_kind_of_family() {
        let p = SystemFallback::over(defaults());
        for _ in 0..2 {
            assert!(p.font_for(&FallbackRequest { cluster: "Ə", family: &FontFamily::Monospace }).is_some());
            assert!(p.font_for(&FallbackRequest { cluster: "한", family: &FontFamily::Proportional }).is_none());
        }
        let found = p.found.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(found.len(), 2);
        assert!(found[&(SCHWA, true)].is_some());
        assert_eq!(found[&('한', false)], None);
    }

    #[test]
    fn a_context_without_the_character_draws_it_with_the_found_face() {
        let mut only_hack = egui::FontDefinitions::empty();
        let defs = egui::FontDefinitions::default();
        let Some(hack) = defs.font_data.get("Hack") else { panic!("egui's default fonts") };
        only_hack.font_data.insert("Hack".into(), hack.clone());
        only_hack.families.insert(FontFamily::Proportional, vec!["Hack".into()]);
        only_hack.families.insert(FontFamily::Monospace, vec!["Hack".into()]);
        let ctx = egui::Context::default();
        ctx.set_fonts(only_hack);
        ctx.add_font_provider(Arc::new(SystemFallback::over(defaults())));
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert!(ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), SCHWA)));
        assert!(!ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), '한')));
    }

    /// A font of a `cmap` with one subtable (`platform`, `encoding`) of format 12 or 13 mapping
    /// `first..=last` from glyph 1, and a glyph table `glyphs` (`glyf`, or `CBDT` for bitmaps):
    /// no more than skrifa needs to read it.
    fn font(platform: u16, encoding: u16, format: u16, first: u32, last: u32, glyphs: &[u8; 4]) -> Vec<u8> {
        let mut cmap = Vec::new();
        for v in [0u16, 1, platform, encoding] {
            cmap.extend(v.to_be_bytes());
        }
        cmap.extend(12u32.to_be_bytes());
        cmap.extend(format.to_be_bytes());
        cmap.extend(0u16.to_be_bytes());
        for v in [28u32, 0, 1, first, last, 1] {
            cmap.extend(v.to_be_bytes());
        }
        let tables: [(&[u8; 4], &[u8]); 2] = [(b"cmap", &cmap), (glyphs, &[0; 4])];
        let mut font = 0x0001_0000u32.to_be_bytes().to_vec();
        font.extend(2u16.to_be_bytes());
        font.extend([0; 6]);
        let mut offset = 12 + 16 * tables.len();
        for (tag, data) in tables {
            font.extend(tag);
            font.extend(0u32.to_be_bytes());
            for v in [offset, data.len()] {
                font.extend(u32::try_from(v).unwrap_or_default().to_be_bytes());
            }
            offset += data.len();
        }
        for (_, data) in tables {
            font.extend(data);
        }
        font
    }

    #[test]
    fn a_face_has_the_characters_egui_finds_in_it() {
        // Format 12, Windows UCS-4: egui reads it.
        assert_eq!(chars_in(&font(3, 10, 12, 0x4e00, 0x4e10, b"glyf"), 0).as_deref(), Some(&[(0x4e00, 0x4e10)][..]));
        // Format 13, as in macOS's LastResort.otf: it claims every character, egui reads none,
        // so the face is left out rather than picked before the face that draws them.
        assert_eq!(chars_in(&font(0, 6, 13, 0, 0x10_ffff, b"glyf"), 0), None);
        assert_eq!(chars_in(&font(3, 10, 13, 0, 0x10_ffff, b"glyf"), 0), None);
        // Bitmaps alone, as Noto Color Emoji: egui without `color_fonts` draws nothing from it.
        assert_eq!(chars_in(&font(3, 10, 12, 0x1f600, 0x1f610, b"CBDT"), 0), None);
    }

    #[test]
    fn a_malformed_range_stops_at_the_last_character() {
        // A group to u32::MAX: read up to char::MAX and the glyph count, not 4.3e9 characters.
        let Some(chars) = chars_in(&font(3, 10, 12, 0, u32::MAX, b"glyf"), 0) else { panic!("characters") };
        assert!(chars.iter().all(|&(first, last)| first <= last && last <= u32::from(char::MAX)));
        assert!(chars.iter().map(|&(first, last)| last - first + 1).sum::<u32>() <= u32::from(u16::MAX));
    }

    #[test]
    fn a_request_without_fonts_finds_none() {
        let p = SystemFallback::over(fontdb::Database::new());
        assert!(p.font_for(&FallbackRequest { cluster: "한", family: &FontFamily::Proportional }).is_none());
        assert!(SystemFallback::over(defaults()).font_for(&FallbackRequest { cluster: "", family: &FontFamily::Proportional }).is_none());
    }
}
