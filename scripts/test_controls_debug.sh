#!/bin/bash

echo "Testing NES Emulator with Super Mario Bros (Debug Mode)"
echo "========================================================"
echo ""
echo "This will show controller input debug logs"
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
echo "Starting emulator with debug logs..."
echo ""

RUST_LOG=debug cargo run -- roms/Super_mario_brothers.nes