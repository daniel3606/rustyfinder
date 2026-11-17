//! Guessing how a text file is encoded, for preview and quick edit.
//!
//! Order (preview-quicklook spec §3): a byte-order mark decides; else valid
//! UTF-8 is UTF-8; else a statistical detector picks a legacy encoding; and
//! if that is not confident, Mac OS Roman, so every byte still displays.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guess {
    /// IANA charset name, e.g. "UTF-8", "Shift_JIS", "macintosh".
    pub name: String,
    /// Bytes of byte-order mark to skip before decoding.
    pub bom_length: usize,
    /// False if the bytes look like binary data rather than text.
    pub is_text: bool,
}

/// Guess the encoding of `bytes` (a whole file or a leading sample).
pub fn detect(bytes: &[u8]) -> Guess {
    let guess = |name: &str, bom_length: usize, is_text: bool| Guess { name: name.to_string(), bom_length, is_text };
    // UTF-32 marks must be checked before UTF-16: FF FE 00 00 starts with
    // the UTF-16 LE mark.
    if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        return guess("UTF-32LE", 4, true);
    }
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        return guess("UTF-32BE", 4, true);
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return guess("UTF-8", 3, true);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return guess("UTF-16LE", 2, true);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return guess("UTF-16BE", 2, true);
    }

    // Without a mark, a NUL byte means binary data (or unmarked UTF-16,
    // which cannot be told apart reliably and is rare).
    let is_text = !bytes.contains(&0);
    if valid_utf8_sample(bytes) {
        return guess("UTF-8", 0, is_text);
    }

    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Deny);
    // The detector always answers. If its answer cannot actually decode
    // the bytes, it did not really decide: fall back to Mac OS Roman, in
    // which every byte is a character.
    let (_, had_errors) = encoding.decode_without_bom_handling(bytes);
    if had_errors {
        guess("macintosh", 0, is_text)
    } else {
        guess(encoding.name(), 0, is_text)
    }
}

/// Valid UTF-8, allowing a multi-byte character cut off at the very end
/// (the caller may have passed only the start of a large file).
fn valid_utf8_sample(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && bytes.len() - e.valid_up_to() < 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_order_marks_decide() {
        assert_eq!(detect(b"\xEF\xBB\xBFhello").name, "UTF-8");
        assert_eq!(detect(b"\xEF\xBB\xBFhello").bom_length, 3);
        assert_eq!(detect(b"\xFF\xFEh\x00i\x00"), Guess { name: "UTF-16LE".into(), bom_length: 2, is_text: true });
        assert_eq!(detect(b"\xFE\xFF\x00h\x00i").name, "UTF-16BE");
        assert_eq!(detect(b"\xFF\xFE\x00\x00h\x00\x00\x00").name, "UTF-32LE");
        assert_eq!(detect(b"\x00\x00\xFE\xFF\x00\x00\x00h").name, "UTF-32BE");
    }

    #[test]
    fn valid_utf8_is_utf8() {
        assert_eq!(detect(b"plain ascii").name, "UTF-8");
        assert_eq!(detect("caf\u{e9} \u{65e5}\u{672c}\u{8a9e} \u{1f4c1}".as_bytes()).name, "UTF-8");
        assert_eq!(detect(b"").name, "UTF-8");
        // A sample cut in the middle of a character is still UTF-8.
        let cut = &"\u{65e5}\u{672c}".as_bytes()[..5];
        assert_eq!(detect(cut).name, "UTF-8");
    }

    #[test]
    fn spec_pv03_shift_jis_without_a_mark_is_recognised() {
        let text = "\u{3053}\u{308c}\u{306f}\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}\u{3067}\u{3059}\u{3002}\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{30de}\u{30cd}\u{30fc}\u{30b8}\u{30e3}\u{30fc}\u{306e}\u{30d7}\u{30ec}\u{30d3}\u{30e5}\u{30fc}\u{3092}\u{78ba}\u{8a8d}\u{3057}\u{307e}\u{3059}\u{3002}";
        let (bytes, _, failed) = encoding_rs::SHIFT_JIS.encode(text);
        assert!(!failed);
        let guess = detect(&bytes);
        assert_eq!(guess.name, "Shift_JIS");
        assert!(guess.is_text);
        let (decoded, _) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(&bytes);
        assert_eq!(decoded, text);
    }

    #[test]
    fn other_legacy_encodings_are_recognised() {
        let cases: [(&encoding_rs::Encoding, &str); 3] = [
            (encoding_rs::EUC_KR, "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}. \u{d30c}\u{c77c} \u{ad00}\u{b9ac}\u{c790} \u{bbf8}\u{b9ac}\u{bcf4}\u{ae30}\u{b97c} \u{d655}\u{c778}\u{d569}\u{b2c8}\u{b2e4}."),
            (encoding_rs::WINDOWS_1251, "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}, \u{44d}\u{442}\u{43e} \u{442}\u{435}\u{43a}\u{441}\u{442} \u{43d}\u{430} \u{440}\u{443}\u{441}\u{441}\u{43a}\u{43e}\u{43c} \u{44f}\u{437}\u{44b}\u{43a}\u{435} \u{434}\u{43b}\u{44f} \u{43f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{43a}\u{438}."),
            (encoding_rs::GBK, "\u{8fd9}\u{662f}\u{4e00}\u{6bb5}\u{7528}\u{4e8e}\u{6d4b}\u{8bd5}\u{7f16}\u{7801}\u{68c0}\u{6d4b}\u{7684}\u{7b80}\u{4f53}\u{4e2d}\u{6587}\u{6587}\u{672c}\u{3002}\u{6587}\u{4ef6}\u{7ba1}\u{7406}\u{5668}\u{9884}\u{89c8}\u{3002}"),
        ];
        for (encoding, text) in cases {
            let (bytes, _, failed) = encoding.encode(text);
            assert!(!failed);
            let name = detect(&bytes).name;
            let found = encoding_rs::Encoding::for_label(name.as_bytes()).unwrap();
            let (decoded, _) = found.decode_without_bom_handling(&bytes);
            assert_eq!(decoded, text, "detected {name} for {}", encoding.name());
        }
    }

    #[test]
    fn binary_data_is_flagged_and_every_input_gets_some_encoding() {
        let binary = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D];
        let guess = detect(&binary);
        assert!(!guess.is_text);
        assert!(!guess.name.is_empty());
        // Bytes that fit nothing in particular still get an encoding in
        // which every byte displays.
        let odd = [0x81, 0x8D, 0x8F, 0x90, 0x9D];
        let name = detect(&odd).name;
        let encoding = encoding_rs::Encoding::for_label(name.as_bytes()).unwrap();
        assert!(!encoding.decode_without_bom_handling(&odd).1, "{name} cannot show these bytes");
    }
}
