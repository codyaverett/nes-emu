#!/usr/bin/env bash
# Screenshot harness for the in-window UI (docs/plans/SHARED_OVERLAY_UI.md,
# Phase 0; docs/debugging/UI_FRAMEWORK.md).
#
#   scripts/ui_screenshots.sh ROM OUTDIR
#
# Copies ROM into OUTDIR, pre-creates <rom>.cht and <rom>.s1..s3 next to
# the copy with a fixed content and mtime (so the Cheats and States pages
# are stable), then runs the SDL binary with --no-audio, --ui-script and
# --screenshot to capture the command palette and every tool page as PPM.
# Prints the SHA-256 of every PPM (also written to OUTDIR/hashes.txt).
# Two runs on the same tree must agree; a refactor of the drawing code
# must leave every hash unchanged.
#
# The scripts only view and navigate (they never save a state or add a
# cheat), so nothing in OUTDIR changes between the pre-creation and the
# capture. Frame numbers follow the "+4" rule from UI_FRAMEWORK.md: a page
# is captured four frames after the key that opened it.
#
# NES_EMU_BIN overrides the binary (default: target/debug/nes-emu, built
# here with cargo build so a re-run always tests the current tree).
set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: $0 ROM OUTDIR" >&2
    exit 2
fi
rom_src=$1
out=$2
root=$(cd "$(dirname "$0")/.." && pwd)
bin=${NES_EMU_BIN:-"$root/target/debug/nes-emu"}

if [ -z "${NES_EMU_BIN:-}" ]; then
    (cd "$root" && cargo build --quiet)
fi

mkdir -p "$out"
name=$(basename "$rom_src")
stem=${name%.*}
rom="$out/$name"
cp "$rom_src" "$rom"
rm -f "$out"/*.ppm

# Fixed fixtures: one cheat, three used slots with a fixed size and mtime.
# The States page prints the mtime in UTC, so the touch runs in UTC too.
printf 'SXIOPO\t1\tInfinite lives\n' > "$out/$stem.cht"
for n in 1 2 3; do
    head -c 15098 /dev/zero > "$out/$stem.s$n"
done
TZ=UTC touch -t 202601010000 "$out/$stem.cht" "$out/$stem.s1" "$out/$stem.s2" "$out/$stem.s3"

run() {
    # run NAME SCRIPT SCREENSHOT...
    local label=$1 script=$2
    shift 2
    local args=()
    for spec in "$@"; do
        args+=(--screenshot "$out/$spec")
    done
    echo "== $label" >&2
    (cd "$root" && "$bin" "$rom" --no-audio --ui-script "$script" "${args[@]}" 2>"$out/$label.log")
}

# Palette with "vol" typed, then the Help page (UI_FRAMEWORK.md).
run palette \
    "backquote,v,o,l,,,Escape,backquote,h,e,l,p,Return,,,Escape,Escape" \
    palette.ppm:35 help.ppm:44

# Every page from the title screen: 90 empty frames first (frames 30-119),
# then one page after another. Frame of entry i is 30 + i.
empties=$(printf ',%.0s' $(seq 1 90))              # 90 empties, frames 30..119
pages="${empties}backquote,m,e,m,Space,3,0,0,Return" # 120-128, memory at 132
pages+=",,,,,PageDown"                              # 129-132 empty, PageDown 133, page2 at 137
pages+=",,,,,Escape"                                # 134-137 empty, Escape 138
pages+=",backquote,p,p,u,Return"                    # 139-143, patterns at 147
pages+=",,,,,Right"                                 # 144-147 empty, Right 148, nametables at 152
pages+=",,,,,Right"                                 # 149-152 empty, Right 153, palettes at 157
pages+=",,,,,Escape"                                # 154-157 empty, Escape 158
pages+=",backquote,a,p,u,Return,2,5"                # 159-165, apu at 169
pages+=",,,,,Escape"                                # 166-169 empty, Escape 170
pages+=",backquote,u,n,m,u,t,e,Return"              # 171-178 unmute all
pages+=",backquote,a,p,u,Return"                    # 179-183, apu_unmuted at 187
pages+=",,,,,Escape"                                # 184-187 empty, Escape 188
pages+=",backquote,c,h,e,a,t,s,Return"              # 189-196, cheats at 200
pages+=",,,,,Escape"                                # 197-200 empty, Escape 201
pages+=",backquote,s,t,a,t,e,s,Return"              # 202-209, states at 213
pages+=",,,,,Down"                                  # 210-213 empty, Down 214, states-cursor at 218
pages+=",,,,,Escape"                                # 215-218 empty, Escape 219
pages+=",F7"                                        # 220, toast "Slot 2 (saved)" at 223
pages+=",,,,Backspace*40"                           # 221-223 empty, hold 224-263, rewind at 250
pages+=",F1"                                        # 264, help-rewind at 268
pages+=",,,,,Escape,Escape"                         # 265-268 empty, close 269, quit 270
run pages "$pages" \
    memory.ppm:132 memory_page2.ppm:137 \
    ppu_patterns.ppm:147 ppu_nametables.ppm:152 ppu_palettes.ppm:157 \
    apu.ppm:169 apu_unmuted.ppm:187 \
    cheats.ppm:200 states.ppm:213 states-cursor.ppm:218 \
    toast.ppm:223 rewind.ppm:250 help-rewind.ppm:268

(cd "$out" && shasum -a 256 *.ppm | sort -k2) | tee "$out/hashes.txt"
