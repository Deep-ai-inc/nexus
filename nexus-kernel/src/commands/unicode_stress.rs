//! Unicode stress test command — outputs comprehensive Unicode edge cases
//! to stress-test text rendering, font fallback, layout, shaping, BiDi,
//! and selection.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct UnicodeStressCommand;

impl NexusCommand for UnicodeStressCommand {
    fn name(&self) -> &'static str {
        "unicode-stress"
    }

    fn execute(&self, _args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut out = String::new();

        emit_section(&mut out, "Complex Scripts & BiDi", &[
            ("Arabic ligatures (connected)", "لإرللا"),
            ("Arabic ligatures (spaced)", "لإ ر ل لا"),
            ("Allah ligature", "اللّٰه"),
            ("Mixed LTR/RTL", "The title is \"\u{0645}\u{0641}\u{062A}\u{0627}\u{062D} \u{0645}\u{0639}\u{0627}\u{064A}\u{064A}\u{0631} \u{0627}\u{0644}\u{0648}\u{064A}\u{0628}\" in Arabic."),
            ("Nested BiDi", "He said \"She said '\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}' to me\" yesterday."),
            ("Hebrew", "\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{05E2}\u{05D5}\u{05DC}\u{05DD}"),
            ("Numbers in RTL", "\u{0661}\u{0662}\u{0663}\u{0664}\u{0665}"),
        ]);

        emit_section(&mut out, "Combining Marks & Normalization", &[
            ("Precomposed \u{00F1} (U+00F1)", "\u{00F1}"),
            ("Decomposed n+\u{0303} (U+006E U+0303)", "n\u{0303}"),
            ("Precomposed caf\u{00E9}", "caf\u{00E9}"),
            ("Decomposed cafe\u{0301}", "cafe\u{0301}"),
            ("Stacked (a + 5 marks)", "a\u{0310}\u{0304}\u{0306}\u{0305}\u{033F}"),
            ("Zalgo", "H\u{0321}\u{034A}e\u{0329}l\u{0340}l\u{0340}o\u{0340} W\u{0340}o\u{0340}r\u{0340}l\u{0340}d\u{0340}"),
        ]);

        emit_section(&mut out, "Emoji & Color Fonts", &[
            ("Standard", "\u{1F600}\u{1F389}\u{1F525}\u{1F4AF}"),
            ("Skin tone modifier", "\u{1F44D}\u{1F3FF}"),
            ("ZWJ family", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}"),
            ("ZWJ profession", "\u{1F469}\u{200D}\u{1F680}"),
            ("Flag (regional indicator)", "\u{1F1FA}\u{1F1F8}\u{1F1EF}\u{1F1F5}\u{1F1E9}\u{1F1EA}"),
            ("Flag (tag sequence, Scotland)", "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}"),
            ("UN flag", "\u{1F1FA}\u{1F1F3}"),
            ("Keycap sequences", "1\u{FE0F}\u{20E3}2\u{FE0F}\u{20E3}3\u{FE0F}\u{20E3}"),
            ("Variation selector (emoji)", "\u{2764}\u{FE0F}"),
            ("Variation selector (text)", "\u{2764}\u{FE0E}"),
            ("Compound ZWJ", "\u{1F469}\u{200D}\u{1F4BB}\u{1F468}\u{200D}\u{1F52C}\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}"),
        ]);

        emit_section(&mut out, "Whitespace & Invisible Characters", &[
            ("Zero-width space", "User\u{200B}Name"),
            ("Zero-width non-joiner", "fi\u{200C}nd"),
            ("Zero-width joiner", "a\u{200D}b"),
            ("Right-to-left mark", "\u{200F}abc"),
            ("Standard space (U+0020)", "a b"),
            ("No-break space (U+00A0)", "a\u{00A0}b"),
            ("Em space (U+2003)", "a\u{2003}b"),
            ("Thin space (U+2009)", "a\u{2009}b"),
            ("Soft hyphen", "long\u{00AD}word"),
            ("BOM prefix", "\u{FEFF}text"),
        ]);

        emit_section(&mut out, "CJK & Fullwidth", &[
            ("Japanese (Kanji/Kana mix)", "\u{79C1}\u{306F}\u{30AC}\u{30E9}\u{30B9}\u{3092}\u{98DF}\u{3079}\u{3089}\u{308C}\u{307E}\u{3059}\u{3002}"),
            ("CJK ideographs", "\u{6F22}\u{5B57}\u{30C6}\u{30B9}\u{30C8}\u{D55C}\u{AD6D}\u{C5B4}"),
            ("Fullwidth ASCII", "\u{FF28}\u{FF25}\u{FF2C}\u{FF2C}\u{FF2F}"),
            ("Halfwidth katakana", "\u{FF76}\u{FF80}\u{FF76}\u{FF85}"),
            ("Mixed width", "Hello\u{4E16}\u{754C}abc"),
            ("Vertical punctuation", "\u{FF08}text\u{FF09}"),
            ("Rare CJK Extension B", "\u{20BB7}"),
        ]);

        emit_section(&mut out, "Astral Plane Characters", &[
            ("Linear B Syllabary (U+10000)", "\u{10000}"),
            ("Egyptian Hieroglyphs (U+13000)", "\u{13000}"),
            ("Musical G Clef (U+1D11E)", "\u{1D11E}"),
            ("Tetragram for Centre (U+1D306)", "\u{1D306}"),
        ]);

        emit_section(&mut out, "Edge Cases & Naughty Strings", &[
            ("Widest char (U+FDFD)", "\u{FDFD}"),
            ("Cyrillic 'a' vs Latin 'a'", "\u{0430} vs a"),
            ("Vertical tab", "Line\u{000B}Break"),
            ("Replacement char", "\u{FFFD}"),
        ]);

        let fire_200 = "\u{1F525}".repeat(200);
        emit_section(&mut out, "Extreme Lengths", &[
            ("200\u{00D7} fire emoji", &fire_200),
            ("Alternating width", "a\u{6F22}b\u{5B57}c\u{D14C}d\u{C2A4}e\u{D2B8}"),
            ("Empty string", ""),
            ("Single char", "a"),
        ]);

        let mega = "The quick brown \u{1F98A} jumps over the lazy \u{1F415}.  \u{0645}\u{0631}\u{062D}\u{0628}\u{0627} \u{0628}\u{0627}\u{0644}\u{0639}\u{0627}\u{0644}\u{0645} (RTL). Zalgo: H\u{0321}\u{034A}e\u{0329}l\u{0340}l\u{0340}o\u{0340}. Family: \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}. Surrogate: \u{1D306}. Wide: \u{FDFD}.";
        emit_section(&mut out, "Mega String (combined)", &[
            ("Everything at once", mega),
        ]);

        // Table alignment stress — formatted as aligned columns
        out.push_str("━━━ Table Alignment Stress ━━━\n");
        out.push_str(&format!(
            "{:<10} {:<8} {:<8} {:<10} {}\n",
            "ascii", "cjk", "emoji", "mixed", "rtl"
        ));
        out.push_str(&format!(
            "{:<10} {:<8} {:<8} {:<10} {}\n",
            "─────", "────", "─────", "─────", "───"
        ));
        for (ascii, cjk, emoji, mixed, rtl) in [
            ("hello", "\u{4F60}\u{597D}", "\u{1F44B}", "hi\u{4E16}\u{754C}", "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}"),
            ("world", "\u{4E16}\u{754C}", "\u{1F30D}", "ok\u{6F22}\u{5B57}", "\u{0639}\u{0627}\u{0644}\u{0645}"),
            ("test", "\u{6D4B}\u{8BD5}", "\u{1F9EA}", "go\u{D55C}\u{AD6D}", "\u{0627}\u{062E}\u{062A}\u{0628}\u{0627}\u{0631}"),
            ("A", "\u{5B57}", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}", "x\u{20BB7}y", "\u{FDFD}"),
        ] {
            out.push_str(&format!(
                "{:<10} {:<8} {:<8} {:<10} {}\n",
                ascii, cjk, emoji, mixed, rtl
            ));
        }

        Ok(Value::String(out))
    }
}

fn emit_section(out: &mut String, title: &str, rows: &[(&str, &str)]) {
    out.push_str(&format!("━━━ {} ━━━\n", title));
    for (category, text) in rows {
        out.push_str(&format!(
            "  {:<40} {} ({}cp)\n",
            category,
            text,
            text.chars().count()
        ));
    }
    out.push('\n');
}
