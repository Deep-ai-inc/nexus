#!/bin/bash
# ANSI escape sequence stress test — exercises terminal rendering of SGR attributes,
# 256-color, true color, cursor movement, line ops, and edge cases.

ESC=$'\033'
CSI="${ESC}["

# ── Helpers ──────────────────────────────────────────────────────────────────

section() {
    printf '\n%s── %s ──%s\n' "${CSI}1;37m" "$1" "${CSI}0m"
}

# ── 1. Basic SGR Attributes ─────────────────────────────────────────────────

section "Basic SGR Attributes"
printf '  %sbold%s  '            "${CSI}1m"  "${CSI}0m"
printf '%sdim%s  '               "${CSI}2m"  "${CSI}0m"
printf '%sitalic%s  '            "${CSI}3m"  "${CSI}0m"
printf '%sunderline%s  '         "${CSI}4m"  "${CSI}0m"
printf '%sblink%s  '             "${CSI}5m"  "${CSI}0m"
printf '%sinverse%s  '           "${CSI}7m"  "${CSI}0m"
printf '%shidden(?)%s  '         "${CSI}8m"  "${CSI}0m"
printf '%sstrikethrough%s  '     "${CSI}9m"  "${CSI}0m"
printf '%sdouble-underline%s\n'  "${CSI}4:2m" "${CSI}0m"

printf '  %sbold+italic%s  '               "${CSI}1;3m"    "${CSI}0m"
printf '%sbold+underline%s  '              "${CSI}1;4m"    "${CSI}0m"
printf '%sitalic+strikethrough%s  '        "${CSI}3;9m"    "${CSI}0m"
printf '%sdim+italic+underline%s  '        "${CSI}2;3;4m"  "${CSI}0m"
printf '%sbold+italic+underline+strike%s\n' "${CSI}1;3;4;9m" "${CSI}0m"

# ── 2. Standard 8/16 ANSI Colors ────────────────────────────────────────────

section "Standard 16 ANSI Colors (fg)"
printf '  '
for c in 30 31 32 33 34 35 36 37 90 91 92 93 94 95 96 97; do
    printf '%s %-3s %s' "${CSI}${c}m" "$c" "${CSI}0m"
done
printf '\n'

section "Standard 16 ANSI Colors (bg)"
printf '  '
for c in 40 41 42 43 44 45 46 47 100 101 102 103 104 105 106 107; do
    printf '%s %-4s%s' "${CSI}${c}m" "$c" "${CSI}0m"
done
printf '\n'

section "Foreground on Background Combinations"
for fg in 30 31 32 33 34 35 36 37; do
    printf '  '
    for bg in 40 41 42 43 44 45 46 47; do
        printf '%s %d;%d %s' "${CSI}${fg};${bg}m" "$fg" "$bg" "${CSI}0m"
    done
    printf '\n'
done

# ── 3. 256-Color Palette ────────────────────────────────────────────────────

section "256-Color Palette (foreground)"
printf '  Standard:   '
for c in $(seq 0 15); do
    printf '%s██%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'

printf '  Color cube: '
for c in $(seq 16 51); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'
printf '              '
for c in $(seq 52 87); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'
printf '              '
for c in $(seq 88 123); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'
printf '              '
for c in $(seq 124 159); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'
printf '              '
for c in $(seq 160 195); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'
printf '              '
for c in $(seq 196 231); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'

printf '  Grayscale:  '
for c in $(seq 232 255); do
    printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'

section "256-Color Palette (background)"
printf '  Standard:   '
for c in $(seq 0 15); do
    printf '%s  %s' "${CSI}48;5;${c}m" "${CSI}0m"
done
printf '\n'

printf '  Grayscale:  '
for c in $(seq 232 255); do
    printf '%s %s' "${CSI}48;5;${c}m" "${CSI}0m"
done
printf '\n'

# ── 4. True Color (24-bit RGB) ──────────────────────────────────────────────

section "True Color Gradients (24-bit RGB)"
printf '  Red:    '
for i in $(seq 0 4 255); do
    printf '%s▀%s' "${CSI}38;2;${i};0;0m" "${CSI}0m"
done
printf '\n'

printf '  Green:  '
for i in $(seq 0 4 255); do
    printf '%s▀%s' "${CSI}38;2;0;${i};0m" "${CSI}0m"
done
printf '\n'

printf '  Blue:   '
for i in $(seq 0 4 255); do
    printf '%s▀%s' "${CSI}38;2;0;0;${i}m" "${CSI}0m"
done
printf '\n'

printf '  Rainbow: '
for i in $(seq 0 2 179); do
    # HSV hue rotation approximation
    if [ $i -lt 30 ]; then
        r=255; g=$(( i * 255 / 30 )); b=0
    elif [ $i -lt 60 ]; then
        r=$(( (60 - i) * 255 / 30 )); g=255; b=0
    elif [ $i -lt 90 ]; then
        r=0; g=255; b=$(( (i - 60) * 255 / 30 ))
    elif [ $i -lt 120 ]; then
        r=0; g=$(( (120 - i) * 255 / 30 )); b=255
    elif [ $i -lt 150 ]; then
        r=$(( (i - 120) * 255 / 30 )); g=0; b=255
    else
        r=255; g=0; b=$(( (180 - i) * 255 / 30 ))
    fi
    printf '%s▀%s' "${CSI}38;2;${r};${g};${b}m" "${CSI}0m"
done
printf '\n'

printf '  BG grad: '
for i in $(seq 0 4 255); do
    printf '%s %s' "${CSI}48;2;${i};$(( 255 - i ));$(( i / 2 ))m" "${CSI}0m"
done
printf '\n'

# ── 5. Styled + Colored Combinations ────────────────────────────────────────

section "Styled + Colored Combinations"
printf '  %sbold red%s  '                      "${CSI}1;31m"               "${CSI}0m"
printf '%sdim green%s  '                       "${CSI}2;32m"               "${CSI}0m"
printf '%sitalic yellow%s  '                   "${CSI}3;33m"               "${CSI}0m"
printf '%sunderline blue%s  '                  "${CSI}4;34m"               "${CSI}0m"
printf '%sstrike magenta%s  '                  "${CSI}9;35m"               "${CSI}0m"
printf '%sinverse cyan%s\n'                    "${CSI}7;36m"               "${CSI}0m"
printf '  %sbold+underline on bright bg%s  '   "${CSI}1;4;37;102m"        "${CSI}0m"
printf '%sdim+italic 256-color%s  '            "${CSI}2;3;38;5;208m"      "${CSI}0m"
printf '%sbold true-color on true-color bg%s\n' "${CSI}1;38;2;255;165;0;48;2;0;0;80m" "${CSI}0m"

# ── 6. SGR Reset Granularity ────────────────────────────────────────────────

section "SGR Reset Granularity"
printf '  %sbold+italic → reset bold → %s still italic? %s\n'   "${CSI}1;3m" "${CSI}22m" "${CSI}0m"
printf '  %sunderline+strike → reset underline → %s still strike? %s\n' "${CSI}4;9m" "${CSI}24m" "${CSI}0m"
printf '  %sfg red+bg green → reset fg → %s bg still green? %s\n' "${CSI}31;42m" "${CSI}39m" "${CSI}0m"
printf '  %sfg red+bg green → reset bg → %s fg still red? %s\n'  "${CSI}31;42m" "${CSI}49m" "${CSI}0m"

# ── 7. Rapid Style Switching ────────────────────────────────────────────────

section "Rapid Style Switching (per-character)"
printf '  '
text="The quick brown fox jumps over the lazy dog"
i=0
while [ $i -lt ${#text} ]; do
    ch="${text:$i:1}"
    fg=$(( 31 + (i % 7) ))
    bold=$(( (i % 3 == 0) ))
    italic=$(( (i % 5 == 0) ))
    ul=$(( (i % 4 == 0) ))
    attrs=""
    [ $bold -eq 1 ] && attrs="${attrs}1;"
    [ $italic -eq 1 ] && attrs="${attrs}3;"
    [ $ul -eq 1 ] && attrs="${attrs}4;"
    printf '%s%s%s' "${CSI}${attrs}${fg}m" "$ch" "${CSI}0m"
    i=$(( i + 1 ))
done
printf '\n'

# ── 8. Dense Color Blocks (stress GPU instance count) ───────────────────────

section "Dense Colored Text (many style changes per row)"
for row in $(seq 1 8); do
    printf '  '
    for col in $(seq 0 79); do
        c=$(( (row * 80 + col) % 216 + 16 ))
        printf '%s█%s' "${CSI}38;5;${c}m" "${CSI}0m"
    done
    printf '\n'
done

# ── 9. Background Color Fills ───────────────────────────────────────────────

section "Background Color Fills"
printf '  '
for c in 41 42 43 44 45 46 47 100 101 102 103 104 105 106; do
    printf '%s      %s' "${CSI}${c}m" "${CSI}0m"
done
printf '\n'

printf '  True-color BG blocks: '
for r in 0 50 100 150 200 255; do
    for b in 0 80 160 255; do
        printf '%s  %s' "${CSI}48;2;${r};100;${b}m" "${CSI}0m"
    done
done
printf '\n'

# ── 10. Underline Styles (if supported) ─────────────────────────────────────

section "Underline Styles"
printf '  %sstraight%s  '      "${CSI}4m"      "${CSI}0m"
printf '%sdouble%s  '         "${CSI}4:2m"    "${CSI}0m"
printf '%scurly%s  '          "${CSI}4:3m"    "${CSI}0m"
printf '%sdotted%s  '         "${CSI}4:4m"    "${CSI}0m"
printf '%sdashed%s  '         "${CSI}4:5m"    "${CSI}0m"
printf '\n'

# Colored underlines (SGR 58;5;N and 58;2;R;G;B)
printf '  Colored: '
printf '%s%sred underline%s  '    "${CSI}4m" "${CSI}58;2;255;0;0m"   "${CSI}0m"
printf '%s%sgreen underline%s  '  "${CSI}4m" "${CSI}58;2;0;255;0m"   "${CSI}0m"
printf '%s%sblue underline%s  '   "${CSI}4m" "${CSI}58;2;0;0;255m"   "${CSI}0m"
printf '%s%s256-color UL%s\n'     "${CSI}4m" "${CSI}58;5;208m"       "${CSI}0m"

# ── 11. Long Lines and Wrapping ─────────────────────────────────────────────

section "Long Colored Line (should wrap correctly)"
printf '  '
for i in $(seq 1 200); do
    c=$(( (i * 3) % 216 + 16 ))
    printf '%s%d%s ' "${CSI}38;5;${c}m" "$i" "${CSI}0m"
done
printf '\n'

# ── 12. Mixed Content: Colors + Unicode ─────────────────────────────────────

section "Mixed: ANSI Colors + Unicode"
printf '  %s🔴 Red emoji label%s  '        "${CSI}31m"    "${CSI}0m"
printf '%s🟢 Green emoji label%s  '       "${CSI}32m"    "${CSI}0m"
printf '%s🔵 Blue emoji label%s\n'        "${CSI}34m"    "${CSI}0m"
printf '  %s漢字 in bold%s  '              "${CSI}1;33m"  "${CSI}0m"
printf '%s한국어 italic%s  '              "${CSI}3;35m"  "${CSI}0m"
printf '%sمرحبا underlined%s\n'           "${CSI}4;36m"  "${CSI}0m"
printf '  %s🔥bold+red+bg%s  '            "${CSI}1;31;43m" "${CSI}0m"
printf '%s👨‍👩‍👧‍👦 ZWJ family in magenta%s  ' "${CSI}35m"   "${CSI}0m"
printf '%s café%s\n'                      "${CSI}2;32m"  "${CSI}0m"

# ── 13. Edge Cases ──────────────────────────────────────────────────────────

section "Edge Cases"
printf '  Empty SGR (reset): before%s[after]%s end\n'    "${CSI}m"  "${CSI}0m"
printf '  Multiple resets: %s%s%s%sstill normal\n'        "${CSI}0m" "${CSI}0m" "${CSI}0m" "${CSI}0m"
printf '  Garbage params:  %signored?%s\n'                "${CSI}999m" "${CSI}0m"
printf '  Many params:     %smany;params%s\n'             "${CSI}1;2;3;4;5;7;9;31;42m" "${CSI}0m"
printf '  No-op sequences: %s%s%svisible\n'               "${CSI}0m" "${CSI}0m" "${CSI}0m"
printf '  Semicolons only: %s(should reset)%s\n'          "${CSI};m" "${CSI}0m"
printf '  Zero param:      %s(should reset)%s\n'          "${CSI}0;0;0m" "${CSI}0m"

# ── 14. Stress: Alternating styles every character ──────────────────────────

section "Stress: Alternating Bold/Normal Every Character"
printf '  '
for i in $(seq 1 80); do
    if [ $(( i % 2 )) -eq 0 ]; then
        printf '%sX%s' "${CSI}1m" "${CSI}0m"
    else
        printf 'x'
    fi
done
printf '\n'

section "Stress: Every Character Different Color"
printf '  '
for i in $(seq 0 79); do
    c=$(( i % 216 + 16 ))
    printf '%s#%s' "${CSI}38;5;${c}m" "${CSI}0m"
done
printf '\n'

section "Stress: True Color Per Character"
printf '  '
for i in $(seq 0 79); do
    r=$(( (i * 3) % 256 ))
    g=$(( (i * 7 + 50) % 256 ))
    b=$(( (i * 11 + 100) % 256 ))
    printf '%s▓%s' "${CSI}38;2;${r};${g};${b}m" "${CSI}0m"
done
printf '\n'

# ── 15. Hyperlinks (OSC 8) ──────────────────────────────────────────────────

section "OSC 8 Hyperlinks"
printf '  %s]8;;https://example.com%s\\click here%s]8;;%s\\\n' "${ESC}" "${ESC}" "${ESC}" "${ESC}"
printf '  %s]8;;https://example.com%s\\%sblue link%s%s]8;;%s\\\n' "${ESC}" "${ESC}" "${CSI}34;4m" "${CSI}0m" "${ESC}" "${ESC}"

# ── 16. Title Setting (OSC 0/1/2) ──────────────────────────────────────────

section "OSC Title Sequences (should not render visibly)"
printf '  Before title set...'
printf '%s]0;ANSI Stress Test%s\\' "${ESC}" "${ESC}"
printf ' after title set.\n'
printf '  %s]2;Window Title Test%s\\(icon title)%s]1;Icon Title%s\\\n' "${ESC}" "${ESC}" "${ESC}" "${ESC}"

# ── 17. Cursor Save/Restore ─────────────────────────────────────────────────

section "Cursor Save/Restore (DECSC/DECRC)"
printf '  Start...'
printf '%s7' "${ESC}"           # Save cursor
printf '%s[5CINSERTED' "${ESC}" # Move right 5, print
printf '%s8' "${ESC}"           # Restore cursor
printf '(restored here)\n'

# ── 18. Tab Stops ───────────────────────────────────────────────────────────

section "Tab Stops"
printf '  Col0\tCol8\tCol16\tCol24\tCol32\tCol40\n'
printf '  A\tB\tC\tD\tE\tF\n'

# ── 19. Box Drawing Characters ─────────────────────────────────────────

section "Box Drawing Characters"
printf '  Light:  ─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼\n'
printf '  Heavy:  ━ ┃ ┏ ┓ ┗ ┛ ┣ ┫ ┳ ┻ ╋\n'
printf '  Double: ═ ║ ╔ ╗ ╚ ╝ ╠ ╣ ╦ ╩ ╬\n'
printf '  Mixed:  ╒ ╕ ╘ ╛ ╞ ╡ ╥ ╨ ╪ ╫ ╳\n'
printf '  Rounded: ╭ ╮ ╰ ╯\n'
printf '  Box:\n'
printf '    ┌──────┬──────┐\n'
printf '    │ Cell │ Cell │\n'
printf '    ├──────┼──────┤\n'
printf '    │ Cell │ Cell │\n'
printf '    └──────┴──────┘\n'
printf '  Double box:\n'
printf '    ╔══════╦══════╗\n'
printf '    ║ Cell ║ Cell ║\n'
printf '    ╠══════╬══════╣\n'
printf '    ║ Cell ║ Cell ║\n'
printf '    ╚══════╩══════╝\n'

# ── 20. Cursor Movements ──────────────────────────────────────────────

section "Cursor Movements"
printf '  CUF (forward):  start%s[10C<-- 10 cols right\n' "${ESC}"
printf '  CHA (col abs):  %s[20Gstarting at col 20\n' "${ESC}"
printf '  Overwrite test: AAAAAAAAAA'
printf '%s[10D' "${ESC}"  # move left 10
printf 'BBBB'             # overwrite first 4 A's
printf '\n'
printf '  Backspace test: 12345\b\b\bXY\n'

# ── 21. Erase Operations ──────────────────────────────────────────────

section "Erase Operations"
# EL mode 0: print text with trailing XXXXX, back up into Xs, erase to EOL
printf '  EL mode 0 (erase to EOL): visibleXXXXX%s[5D%s[K ←Xs gone\n' "${ESC}" "${ESC}"
# EL mode 1: erase from cursor to beginning of line
printf '  EL mode 1 (erase to BOL): ERASED%s[1K visible\n' "${ESC}"
# EL mode 2: erase entire line then overwrite
printf '  EL mode 2 (erase line):   full line%s[2K  (line was erased and replaced)\n' "${ESC}"
# ECH: erase 3 characters at cursor
printf '  ECH (erase chars):        XXXXX%s[5D%s[3X ←3 Xs erased\n' "${ESC}" "${ESC}"

# ── 22. Insert/Delete Characters and Lines ─────────────────────────────

section "Insert/Delete Characters"
printf '  ICH (insert 3): ABCDEF%s[4D%s[3@___\n' "${ESC}" "${ESC}"
printf '  DCH (delete 2): ABCDEF%s[4D%s[2P\n' "${ESC}" "${ESC}"

# ── 23. Scroll Up/Down ────────────────────────────────────────────────

section "Scroll Operations (SU/SD)"
# Simple scroll test — no cursor position query needed
printf '  scroll-line A\n'
printf '  scroll-line B\n'
printf '  scroll-line C\n'
printf '%s[1S' "${ESC}"      # SU: scroll up 1 — shifts content up
printf '  (scrolled up 1 — line A should be further away)\n'
printf '%s[1T' "${ESC}"      # SD: scroll down 1 — shifts content down
printf '  (scrolled down 1 — blank line inserted above)\n'

# ── 24. SGR Attribute Resets (Granular) ────────────────────────────────

section "SGR Granular Resets"
printf '  %sbold%s → reset bold(22m) → %snormal?%s\n'            "${CSI}1m" "" "${CSI}22m" "${CSI}0m"
printf '  %sitalic%s → reset italic(23m) → %snormal?%s\n'        "${CSI}3m" "" "${CSI}23m" "${CSI}0m"
printf '  %sunderline%s → reset UL(24m) → %snormal?%s\n'         "${CSI}4m" "" "${CSI}24m" "${CSI}0m"
printf '  %sblink%s → reset blink(25m) → %snormal?%s\n'          "${CSI}5m" "" "${CSI}25m" "${CSI}0m"
printf '  %sinverse%s → reset inverse(27m) → %snormal?%s\n'      "${CSI}7m" "" "${CSI}27m" "${CSI}0m"
printf '  conceal(8m): [%sHIDDEN%s] ← should be blank between brackets%s\n' "${CSI}8m" "${CSI}28m" "${CSI}0m"
printf '  %sstrikethrough%s → reset strike(29m) → %snormal?%s\n' "${CSI}9m" "" "${CSI}29m" "${CSI}0m"
printf '  %soverline%s(53m) → reset overline(55m) → %snormal?%s\n' "${CSI}53m" "" "${CSI}55m" "${CSI}0m"

# ── 25. Overline (SGR 53) ─────────────────────────────────────────────

section "Overline (SGR 53)"
printf '  %soverlined text%s  '                    "${CSI}53m"       "${CSI}0m"
printf '%soverline+underline%s  '                 "${CSI}53;4m"     "${CSI}0m"
printf '%soverline+bold+red%s\n'                  "${CSI}53;1;31m"  "${CSI}0m"

# ── 26. Combining Characters and Diacritics ────────────────────────────

section "Combining Characters / Diacritics"
printf '  Single combining:  a\xCC\x81 e\xCC\x82 o\xCC\x88 u\xCC\x83 n\xCC\x83\n'
printf '  Stacked combining: a\xCC\x81\xCC\x82\xCC\x83 (3 marks on one base)\n'
printf '  Precomposed vs decomposed: é (precomposed) vs e\xCC\x81 (decomposed)\n'
printf '  Zalgo-lite: H\xCC\x81\xCC\x82\xCC\x83e\xCC\x84\xCC\x85l\xCC\x86\xCC\x87l\xCC\x88\xCC\x89o\xCC\x8A\xCC\x8B\n'

# ── 27. Wide Characters (CJK, Korean, etc.) ───────────────────────────

section "Wide Characters (fullwidth)"
printf '  CJK:       你好世界 (4 chars, 8 cells)\n'
printf '  Korean:    안녕하세요 (5 chars, 10 cells)\n'
printf '  Japanese:  日本語テスト (6 chars, 12 cells)\n'
printf '  Fullwidth: ＡＢＣＤ (4 chars, 8 cells)\n'
printf '  Mixed:     AB你好CD (6 chars, 10 cells)\n'
printf '  Alignment test:\n'
printf '    |12345678|\n'
printf '    |你好世界|\n'
printf '    |ABCDEFGH|\n'

# ── 28. Variation Selectors (Emoji Presentation) ──────────────────────

section "Variation Selectors"
printf '  Text style (U+FE0E):  ☺︎ ☹︎ ❤︎ ⭐︎ ☀︎\n'
printf '  Emoji style (U+FE0F): ☺️ ☹️ ❤️ ⭐️ ☀️\n'
printf '  Mixed in line: Hello ❤️ World ⭐︎ End\n'

# ── 29. Zero-Width Characters ──────────────────────────────────────────

section "Zero-Width Characters"
printf '  ZWSP between: A\xE2\x80\x8BB (should look like AB)\n'
printf '  ZWNJ between: A\xE2\x80\x8CB (should look like AB)\n'
printf '  ZWJ between:  A\xE2\x80\x8DB (should look like AB)\n'
printf '  Soft hyphen:   syl\xC2\xADla\xC2\xADble (invisible hyphens)\n'

# ── 30. Ambiguous-Width Characters ─────────────────────────────────────

section "Ambiguous-Width Characters"
printf '  Greek:   α β γ δ ε Ω\n'
printf '  Math:    ± × ÷ √ ∞ ≈ ≠ ≤ ≥\n'
printf '  Symbols: © ® ™ § ¶ • ·\n'
printf '  Arrows:  ← → ↑ ↓ ↔ ↕\n'
printf '  Blocks:  ░ ▒ ▓ █ ▀ ▄ ▌ ▐\n'

# ── 31. DEC Special Graphics / Line Drawing (Alt Charset) ─────────────

section "DEC Line Drawing (SI/SO charset switch)"
printf '  Switch to G0 line drawing: '
printf '%s(0' "${ESC}"  # Select DEC Special Graphics for G0
printf 'lqqqqqqqqqqk\n'
printf '                               x          x\n'
printf '                               mqqqqqqqqqqj'
printf '%s(B' "${ESC}"  # Back to ASCII
printf '\n  (should draw a box if DEC graphics supported)\n'

# ── 32. CSI s/u Cursor Save/Restore ───────────────────────────────────

section "CSI s/u Cursor Save/Restore"
printf '  Start here...'
printf '%s[s' "${ESC}"       # Save cursor (CSI s)
printf '%s[10C<inserted>' "${ESC}"  # Move right 10
printf '%s[u' "${ESC}"       # Restore cursor (CSI u)
printf '(restored)\n'

# ── 33. Wraparound Edge Case ──────────────────────────────────────────

section "Wraparound Mode (printing at last column)"
COLS=$(tput cols 2>/dev/null || echo 80)
PREFIX='  Fill to edge: '
FILL=$((COLS - ${#PREFIX}))
printf '%s' "$PREFIX"
for i in $(seq 1 "$FILL"); do printf '#'; done
printf '\n  (should have filled to right edge without wrapping early or late)\n'

# ── 34. Tab Stops (HTS / TBC) ─────────────────────────────────────────

section "Custom Tab Stops (HTS/TBC)"
printf '%s[3g' "${ESC}"           # Clear all tab stops (TBC mode 3)
printf '%s[5G%sH' "${ESC}" "${ESC}"   # Move to col 5, set tab stop (HTS)
printf '%s[15G%sH' "${ESC}" "${ESC}"  # Move to col 15, set tab stop
printf '%s[25G%sH' "${ESC}" "${ESC}"  # Move to col 25, set tab stop
printf '\r'                            # Return to start of line
printf '  \tA\tB\tC\n'
printf '  (A at 5, B at 15, C at 25 if custom tabs work)\n'

# ── 35. SGR Edge Cases: Colon vs Semicolon ─────────────────────────────

section "SGR Colon-Separated Params"
printf '  Colon truecolor:  %s38:2::255:100:0mOrange?%s\n'  "${CSI}" "${CSI}0m"
printf '  Colon underline:  %s4:3mCurly?%s\n'               "${CSI}" "${CSI}0m"
printf '  Semicolon equiv:  %s38;2;255;100;0mOrange?%s\n'   "${CSI}" "${CSI}0m"

# ── 36. Overlong / Malformed Sequences ─────────────────────────────────

section "Malformed Sequences (robustness)"
printf '  Incomplete CSI: \033[  (bare CSI+space)\n'
printf '  Missing final:  \033[1  (no m)\n'
printf '  Huge param:     %s99999m(should ignore)%s\n'         "${CSI}" "${CSI}0m"
printf '  Negative param:  %s-1m(should ignore)%s\n'           "${CSI}" "${CSI}0m"
printf '  Empty params:    %s;;;m(should reset)%s\n'           "${CSI}" "${CSI}0m"
printf '  Many semicolons: %s1;2;3;4;5;6;7;8;9;10;11;12;m%s\n' "${CSI}" "${CSI}0m"
printf '  Embedded null:   AB\x00CD (null between chars)\n'

# ── 37. Rapid Full-Row Color Stress ────────────────────────────────────

section "Stress: True Color Per-Cell with BG (80 unique RGB fg+bg per row)"
for row in 1 2 3 4; do
    printf '  '
    for col in $(seq 0 79); do
        r=$(( (row * 60 + col * 3) % 256 ))
        g=$(( (row * 40 + col * 7) % 256 ))
        b=$(( (row * 80 + col * 11) % 256 ))
        printf '%s█%s' "${CSI}38;2;${r};${g};${b};48;2;$(( 255 - r ));$(( 255 - g ));$(( 255 - b ))m" "${CSI}0m"
    done
    printf '\n'
done

# ── 38. Emoji Sequences ───────────────────────────────────────────────

section "Emoji Sequences"
printf '  Basic:     😀 😎 🤖 💀 🎉 🚀\n'
printf '  Flags:     🇺🇸 🇬🇧 🇯🇵 🇩🇪 🇫🇷 🇰🇷\n'
printf '  Skin tone: 👋🏻 👋🏼 👋🏽 👋🏾 👋🏿\n'
printf '  ZWJ:       👨‍💻 👩‍🔬 👨‍👩‍👧‍👦 🏳️‍🌈 👩‍❤️‍👨\n'
printf '  Keycap:    1️⃣ 2️⃣ 3️⃣ #️⃣ *️⃣\n'

# ── 39. RTL / BiDi Text ───────────────────────────────────────────────

section "RTL and BiDi Text"
printf '  Arabic:    مرحبا بالعالم\n'
printf '  Hebrew:    שלום עולם\n'
printf '  Mixed LTR/RTL: Hello مرحبا World عالم End\n'
printf '  Numbers in RTL: العدد 12345 هنا\n'

# ── Done ────────────────────────────────────────────────────────────────────

printf '\n%s━━━ ANSI Stress Test Complete (39 sections) ━━━%s\n' "${CSI}1;32m" "${CSI}0m"
