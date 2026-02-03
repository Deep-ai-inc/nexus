#!/bin/bash
# Unicode stress test — outputs through terminal (PTY) path
# Run this as an external command to test terminal emulator rendering
# vs the native Value::String rendering of `unicode-stress`

printf '━━━ Complex Scripts & BiDi ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Arabic ligatures (connected)' 'لإرللا' 6
printf '  %-40s %s (%dcp)\n' 'Arabic ligatures (spaced)' 'لإ ر ل لا' 9
printf '  %-40s %s (%dcp)\n' 'Allah ligature' 'اللّٰه' 6
printf '  %-40s %s (%dcp)\n' 'Mixed LTR/RTL' 'The title is "مفتاح معايير الويب" in Arabic.' 43
printf '  %-40s %s (%dcp)\n' 'Nested BiDi' 'He said "She said '\''مرحبا'\'' to me" yesterday.' 46
printf '  %-40s %s (%dcp)\n' 'Hebrew' 'שלום עולם' 9
printf '  %-40s %s (%dcp)\n' 'Numbers in RTL' '١٢٣٤٥' 5
echo

printf '━━━ Combining Marks & Normalization ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Precomposed ñ (U+00F1)' 'ñ' 1
printf '  %-40s %s (%dcp)\n' 'Decomposed n+̃ (U+006E U+0303)' 'ñ' 2
printf '  %-40s %s (%dcp)\n' 'Precomposed café' 'café' 4
printf '  %-40s %s (%dcp)\n' 'Decomposed café' 'café' 5
printf '  %-40s %s (%dcp)\n' 'Stacked (a + 5 marks)' 'a̐̄̆̅̿' 6
printf '  %-40s %s (%dcp)\n' 'Zalgo' 'H̡͊eͩl̀l̀ò Wòr̀l̀d̀' 16
echo

printf '━━━ Emoji & Color Fonts ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Standard' '😀🎉🔥💯' 4
printf '  %-40s %s (%dcp)\n' 'Skin tone modifier' '👍🏿' 2
printf '  %-40s %s (%dcp)\n' 'ZWJ family' '👨‍👩‍👧‍👦' 7
printf '  %-40s %s (%dcp)\n' 'ZWJ profession' '👩‍🚀' 3
printf '  %-40s %s (%dcp)\n' 'Flag (regional indicator)' '🇺🇸🇯🇵🇩🇪' 6
printf '  %-40s %s (%dcp)\n' 'Flag (tag sequence, Scotland)' '🏴󠁧󠁢󠁳󠁣󠁴󠁿' 7
printf '  %-40s %s (%dcp)\n' 'UN flag' '🇺🇳' 2
printf '  %-40s %s (%dcp)\n' 'Keycap sequences' '1️⃣2️⃣3️⃣' 9
printf '  %-40s %s (%dcp)\n' 'Variation selector (emoji)' '❤️' 2
printf '  %-40s %s (%dcp)\n' 'Variation selector (text)' '❤︎' 2
printf '  %-40s %s (%dcp)\n' 'Compound ZWJ' '👩‍💻👨‍🔬🏳️‍🌈' 12
echo

printf '━━━ Whitespace & Invisible Characters ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Zero-width space' 'User​Name' 9
printf '  %-40s %s (%dcp)\n' 'Zero-width non-joiner' 'fi‌nd' 5
printf '  %-40s %s (%dcp)\n' 'Zero-width joiner' 'a‍b' 3
printf '  %-40s %s (%dcp)\n' 'Right-to-left mark' '‏abc' 4
printf '  %-40s %s (%dcp)\n' 'Standard space (U+0020)' 'a b' 3
printf '  %-40s %s (%dcp)\n' 'No-break space (U+00A0)' 'a b' 3
printf '  %-40s %s (%dcp)\n' 'Em space (U+2003)' 'a b' 3
printf '  %-40s %s (%dcp)\n' 'Thin space (U+2009)' 'a b' 3
printf '  %-40s %s (%dcp)\n' 'Soft hyphen' 'long­word' 9
printf '  %-40s %s (%dcp)\n' 'BOM prefix' '﻿text' 5
echo

printf '━━━ CJK & Fullwidth ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Japanese (Kanji/Kana mix)' '私はガラスを食べられます。' 13
printf '  %-40s %s (%dcp)\n' 'CJK ideographs' '漢字テスト한국어' 8
printf '  %-40s %s (%dcp)\n' 'Fullwidth ASCII' 'ＨＥＬＬＯ' 5
printf '  %-40s %s (%dcp)\n' 'Halfwidth katakana' 'ｶﾀｶﾅ' 4
printf '  %-40s %s (%dcp)\n' 'Mixed width' 'Hello世界abc' 10
printf '  %-40s %s (%dcp)\n' 'Vertical punctuation' '（text）' 6
printf '  %-40s %s (%dcp)\n' 'Rare CJK Extension B' '𠮷' 1
echo

printf '━━━ Astral Plane Characters ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Linear B Syllabary (U+10000)' '𐀀' 1
printf '  %-40s %s (%dcp)\n' 'Egyptian Hieroglyphs (U+13000)' '𓀀' 1
printf '  %-40s %s (%dcp)\n' 'Musical G Clef (U+1D11E)' '𝄞' 1
printf '  %-40s %s (%dcp)\n' 'Tetragram for Centre (U+1D306)' '𝌆' 1
echo

printf '━━━ Edge Cases & Naughty Strings ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Widest char (U+FDFD)' '﷽' 1
printf '  %-40s %s (%dcp)\n' "Cyrillic 'a' vs Latin 'a'" 'а vs a' 6
printf '  %-40s %s (%dcp)\n' 'Replacement char' '�' 1
echo

printf '━━━ Extreme Lengths ━━━\n'
printf '  %-40s ' '200× fire emoji'
python3 -c "print('🔥' * 200, end='')"
printf ' (%dcp)\n' 200
printf '  %-40s %s (%dcp)\n' 'Alternating width' 'a漢b字cテdスeト' 10
printf '  %-40s %s (%dcp)\n' 'Empty string' '' 0
printf '  %-40s %s (%dcp)\n' 'Single char' 'a' 1
echo

printf '━━━ Mega String (combined) ━━━\n'
printf '  %-40s %s (%dcp)\n' 'Everything at once' 'The quick brown 🦊 jumps over the lazy 🐕.  مرحبا بالعالم (RTL). Zalgo: H̡͊eͩl̀l̀ò. Family: 👨‍👩‍👧‍👦. Surrogate: 𝌆. Wide: ﷽.' 92
echo

printf '━━━ Table Alignment Stress ━━━\n'
printf '%-10s %-8s %-8s %-10s %s\n' 'ascii' 'cjk' 'emoji' 'mixed' 'rtl'
printf '%-10s %-8s %-8s %-10s %s\n' '─────' '────' '─────' '─────' '───'
printf '%-10s %-8s %-8s %-10s %s\n' 'hello' '你好' '👋' 'hi世界' 'مرحبا'
printf '%-10s %-8s %-8s %-10s %s\n' 'world' '世界' '🌍' 'ok漢字' 'عالم'
printf '%-10s %-8s %-8s %-10s %s\n' 'test' '测试' '🧪' 'go한국' 'اختبار'
printf '%-10s %-8s %-8s %-10s %s\n' 'A' '字' '👨‍👩‍👧‍👦' 'x𠮷y' '﷽'
