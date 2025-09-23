#!/bin/bash

echo "Comprehensive Controller Test"
echo "============================="
echo ""
echo "This test shows ALL controller-related activity."
echo ""
echo "Test each key one at a time:"
echo "  Z - A button (should make Mario jump)"
echo "  X - B button (hold to run)"
echo "  Arrow Keys - Move left/right"
echo "  Enter - Start (pause game)"
echo ""
echo "Watch for these messages:"
echo "1. 'Button pressed/released' - confirms SDL input is working"
echo "2. 'Controller strobe' - confirms game is polling controller"
echo "3. 'Controller read' - confirms game is reading button states"
echo ""

RUST_LOG=debug cargo run -- roms/Super_mario_brothers.nes 2>&1 | grep -E "Button|controller|Controller|strobe|4016" | head -100