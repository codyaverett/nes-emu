#!/bin/bash

echo "Simple Button Test"
echo "=================="
echo ""
echo "This test shows ONLY button press/release events."
echo "Press and hold the Z key for a few seconds to test."
echo ""

timeout 10 bash -c 'RUST_LOG=info cargo run -- roms/Super_mario_brothers.nes 2>&1 | grep "Button"'