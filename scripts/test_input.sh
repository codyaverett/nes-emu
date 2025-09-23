#!/bin/bash

echo "Controller Input Test"
echo "===================="
echo ""
echo "This test will show button press/release events in the console."
echo "Try pressing these keys to see if they're detected:"
echo ""
echo "  Z - A button"
echo "  X - B button"
echo "  Arrow Keys - D-pad"
echo "  Enter - Start"
echo "  RShift - Select"
echo ""
echo "Look for 'Button pressed' and 'Button released' messages."
echo ""

RUST_LOG=info cargo run -- roms/Super_mario_brothers.nes 2>&1 | grep -E "Button|controller|Controller"