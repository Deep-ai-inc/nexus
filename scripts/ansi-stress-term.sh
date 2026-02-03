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

# ── Done ────────────────────────────────────────────────────────────────────

printf '\n%s━━━ ANSI Stress Test Complete ━━━%s\n' "${CSI}1;32m" "${CSI}0m"
