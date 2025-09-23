#!/bin/bash

echo "Testing NES Emulator with Super Mario Bros"
echo "==========================================="
echo ""
echo "Controls:"
echo "  Z        - A button (Jump/Confirm)"
echo "  X        - B button (Run/Cancel)"  
echo "  Arrow Keys - D-Pad (Movement)"
echo "  Enter    - Start"
echo "  RShift   - Select"
echo "  R        - Reset"
echo "  Escape   - Quit"
echo ""
echo "Starting emulator..."
echo ""

RUST_LOG=info cargo run -- roms/Super_mario_brothers.nes