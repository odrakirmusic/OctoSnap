// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine behind a trait, and the front door that uses one.
//!
//! `spec/07` §2.2 names RapidOCR -- PaddleOCR's detection and recognition models on ONNX
//! Runtime -- as the default, with Tesseract 5 as the alternative, and the owner settled it
//! on RapidOCR (`docs/decisions.md` D82). The trait exists anyway: everything around the
//! inference is the same either way, and a [`Reader`] over [`Null`] is how the result
//! window, the notification and the two shortcuts are tested without ~15 MB of native
//! library and a model download in the loop.

use crate::{Breaks, Code, Gray, OcrError, Shaped, Word};

/// A script a pack can be downloaded for, per `spec/07` §2.1's "30+ languages".
///
/// Scripts and not languages: a RapidOCR pack is one recognition model per *script*, and
/// the thirty-odd languages are what those five models between them can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
    Latin,
    Cyrillic,
    Cjk,
    Arabic,
    Devanagari,
}

impl Script {
    pub const ALL: [Self; 5] =
        [Self::Latin, Self::Cyrillic, Self::Cjk, Self::Arabic, Self::Devanagari];

    /// The name to show in the language-packs list.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Latin => "Latin",
            Self::Cyrillic => "Cyrillic",
            Self::Cjk => "Chinese, Japanese and Korean",
            Self::Arabic => "Arabic",
            Self::Devanagari => "Devanagari",
        }
    }

    /// The short tag used in filenames and in settings.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Latin => "latin",
            Self::Cyrillic => "cyrillic",
            Self::Cjk => "cjk",
            Self::Arabic => "arabic",
            Self::Devanagari => "devanagari",
        }
    }

    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|script| script.tag() == tag)
    }
}

/// One downloadable model pack, with the size `spec/07` §2.1 wants shown next to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub script: Script,
    /// What it takes on disk, in bytes. Shown before the download, so it is the engine's
    /// to state rather than something discovered on the way.
    pub bytes: u64,
    pub installed: bool,
}

/// Something that turns pixels into boxes of text.
///
/// Given the capture's own greyscale and answering in the capture's own pixels: whatever
/// scaling the engine needs is the engine's business, because only it knows what its
/// models want (D83). `Sync` because the app reads on a worker thread and the engine
/// outlives the read.
pub trait Engine: std::fmt::Debug + Send + Sync {
    /// The packs this engine knows about, installed or not.
    fn packs(&self) -> Vec<Pack>;

    /// Reads an image. `script` of `None` is `spec/07` §2.1's auto-detect.
    ///
    /// # Errors
    /// If the model for the script is missing, or the inference fails.
    fn read(&self, image: &Gray, script: Option<Script>) -> Result<Vec<Word>, OcrError>;
}

/// So an application can hold one engine behind `Arc<dyn Engine>` and hand a clone to
/// each [`Reader`] it builds, rather than one reader owning the models for the process.
impl<E: Engine + ?Sized> Engine for std::sync::Arc<E> {
    fn packs(&self) -> Vec<Pack> {
        (**self).packs()
    }

    fn read(&self, image: &Gray, script: Option<Script>) -> Result<Vec<Word>, OcrError> {
        (**self).read(image, script)
    }
}

/// An engine that reads nothing, successfully.
///
/// The `NullBridge` of `crates/shell`, for the same reason: everything above the engine --
/// the result window, the two shortcuts, the notification, the clipboard -- has to be
/// runnable and testable on a machine where no pack has been downloaded, and "no text
/// here" is a real answer that the UI has to handle anyway.
#[derive(Debug, Default, Clone, Copy)]
pub struct Null;

impl Engine for Null {
    fn packs(&self) -> Vec<Pack> {
        Vec::new()
    }

    fn read(&self, _image: &Gray, _script: Option<Script>) -> Result<Vec<Word>, OcrError> {
        Ok(Vec::new())
    }
}

/// An engine that answers with what it was given. Tests only.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct Fixed(pub Vec<Word>);

#[cfg(test)]
impl Engine for Fixed {
    fn packs(&self) -> Vec<Pack> {
        vec![Pack { script: Script::Latin, bytes: 0, installed: true }]
    }

    fn read(&self, _image: &Gray, _script: Option<Script>) -> Result<Vec<Word>, OcrError> {
        Ok(self.0.clone())
    }
}

/// What a capture turned out to be.
///
/// `spec/07` §2.1 puts a code *instead of* text, not beside it, so this is an either-or
/// rather than a pair of fields: a capture with a QR code in it is a capture of the code,
/// and the clipboard gets the payload.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    Codes(Vec<Code>),
    Text(Shaped),
}

impl Read {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Codes(codes) => codes.is_empty(),
            Self::Text(shaped) => shaped.is_empty(),
        }
    }

    /// What goes on the clipboard.
    #[must_use]
    pub fn text(&self, breaks: Breaks) -> String {
        match self {
            Self::Codes(codes) => {
                codes.iter().map(|code| code.text.as_str()).collect::<Vec<_>>().join("\n")
            }
            Self::Text(shaped) => shaped.text(breaks),
        }
    }

    /// The first line, which is what the notification previews.
    #[must_use]
    pub fn first_line(&self) -> Option<&str> {
        match self {
            Self::Codes(codes) => codes.first().map(|code| code.text.as_str()),
            Self::Text(shaped) => shaped.first_line(),
        }
    }
}

/// Pixels in, `spec/07` §2.1's paragraphs out: the whole of this crate in one call.
#[derive(Debug)]
pub struct Reader<E: Engine> {
    engine: E,
}

impl<E: Engine> Reader<E> {
    pub const fn new(engine: E) -> Self {
        Self { engine }
    }

    #[must_use]
    pub const fn engine(&self) -> &E {
        &self.engine
    }

    /// Reads an RGBA capture.
    ///
    /// # Errors
    /// If the buffer is not four bytes a pixel, or the engine fails.
    pub fn read_rgba(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        script: Option<Script>,
    ) -> Result<Shaped, OcrError> {
        self.read(&Gray::from_rgba(width, height, rgba)?, script)
    }

    /// Reads a greyscale image as text, whatever else is on it.
    ///
    /// # Errors
    /// If the engine fails.
    pub fn read(&self, image: &Gray, script: Option<Script>) -> Result<Shaped, OcrError> {
        Ok(crate::shape(self.engine.read(image, script)?))
    }

    /// `spec/07` §2.1's order: a code if the capture has one, text otherwise.
    ///
    /// The code is looked for first because it is cheap -- no model, no pack, a few
    /// milliseconds -- and because when there is one the text is not the answer.
    ///
    /// # Errors
    /// If the engine fails. A capture with a code in it never reaches the engine, so it
    /// never fails for want of a language pack.
    pub fn scan(&self, image: &Gray, script: Option<Script>) -> Result<Read, OcrError> {
        let codes = crate::codes(image);
        if codes.is_empty() {
            self.read(image, script).map(Read::Text)
        } else {
            Ok(Read::Codes(codes))
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Bounds, Breaks};

    fn page(width: u32, height: u32, ink: u32, pitch: u32) -> Gray {
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        for y in 0..height {
            for x in 0..width {
                pixels.push(if y % pitch < ink && x % 3 == 0 { 30 } else { 240 });
            }
        }
        Gray::new(width, height, pixels).expect("a page")
    }

    #[test]
    fn a_reader_over_a_null_engine_reads_nothing_and_says_so() {
        let reader = Reader::new(Null);
        let shaped = reader.read(&page(100, 100, 14, 20), None).expect("a read");
        assert!(shaped.is_empty());
        assert_eq!(shaped.text(Breaks::Keep), "");
    }

    #[test]
    fn a_dark_page_reaches_the_engine_as_it_was_captured_and_the_boxes_line_up() {
        // Two lines on a 12 px pitch, in the captured image's own pixels: nothing between
        // the engine and `shape` changes a coordinate, so they are one paragraph.
        let dark = page(100, 200, 8, 12).inverted();
        let engine = Fixed(vec![
            Word::new("first", Bounds::new(10, 100, 40, 8), 0.9),
            Word::new("second", Bounds::new(10, 112, 40, 8), 0.9),
        ]);
        let shaped = Reader::new(engine).read(&dark, None).expect("a read");
        assert_eq!(shaped.paragraphs.len(), 1, "{shaped:?}");
        assert_eq!(shaped.text(Breaks::Join), "first second");
        assert_eq!(shaped.paragraphs[0].lines[0].bounds, Bounds::new(10, 100, 40, 8));
    }

    #[test]
    fn a_capture_with_a_code_on_it_never_reaches_the_engine() {
        // An engine that has no pack at all, so reaching it would be an error.
        #[derive(Debug)]
        struct Broken;
        impl Engine for Broken {
            fn packs(&self) -> Vec<Pack> {
                Vec::new()
            }
            fn read(&self, _: &Gray, _: Option<Script>) -> Result<Vec<Word>, OcrError> {
                Err(OcrError::Missing("the model"))
            }
        }
        let code = crate::qr::tests::drawn("https://example.com/q", 4, (20, 20), (300, 300));
        let read = Reader::new(Broken).scan(&code, None).expect("a code, not an engine");
        assert_eq!(read.text(Breaks::Keep), "https://example.com/q");
        assert_eq!(read.first_line(), Some("https://example.com/q"));
        assert!(matches!(read, Read::Codes(_)));

        // And a page with no code on it does reach it, and fails as it should.
        let blank = Gray::new(200, 200, vec![240; 40_000]).expect("a page");
        assert!(Reader::new(Broken).scan(&blank, None).is_err());
    }

    #[test]
    fn a_script_survives_its_tag() {
        for script in Script::ALL {
            assert_eq!(Script::from_tag(script.tag()), Some(script));
            assert!(!script.name().is_empty());
        }
        assert_eq!(Script::from_tag("klingon"), None);
    }

    #[test]
    fn a_capture_that_is_not_four_bytes_a_pixel_is_refused_before_the_engine_sees_it() {
        let reader = Reader::new(Null);
        assert!(reader.read_rgba(2, 2, &[0; 8], None).is_err());
        assert!(reader.read_rgba(2, 2, &[0; 16], None).is_ok());
    }
}
