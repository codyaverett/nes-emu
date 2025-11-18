# Testing Mario START Button

To test if the START button works in Super Mario Bros:

1. Run the emulator with info logging:
```bash
RUST_LOG=info cargo run --release --bin nes-emu roms/mario.nes
```

2. Once the game window opens and you see the Mario title screen, press the Enter/Return key to start the game.

3. Watch the console output for:
   - "START button pressed!" message
   - "CPU reading controller $4016" messages showing the game is polling input
   - "CPU writing controller $4016" messages showing the strobe pattern

4. The game should transition from the title screen to gameplay after pressing START.

## Controls:
- Enter/Return: START button
- Right Shift: SELECT button  
- Arrow Keys: D-pad (Up, Down, Left, Right)
- Z: A button (Jump)
- X: B button (Run/Fire)
- R: Reset the NES
- Escape: Exit emulator

## What to look for in logs:
When you press Enter, you should see:
1. "Button pressed: START, state: 00010000"
2. "START button pressed!" warning
3. Multiple "CPU reading controller" messages as the game polls the controller
4. The game should respond by starting

If the game doesn't start, check if:
- Controller reads are happening (the game is polling $4016)
- The START bit (0x10) is set when reading