# RustRetro Demo Guide - Getting Started with Libretro Cores & ROMs

## Quick Start with Free Cores & Games

This guide helps you find and download free libretro cores and games to test with RustRetro.

## ✅ Recommended Demo Setup

### Best for Learning: NES + Homebrew Game

**Why NES?**
- Easy to emulate
- Many free homebrew games available
- FCEUmm core is well-tested and stable
- Perfect for learning

### Installation Steps

#### 1. Download FCEUmm Core (NES Emulator)

The libretro NES cores are available from the official build bot. You have several options:

**Option A: Download Pre-built Binary**
```bash
# From buildbot.libretro.com/nightly/
# Navigate to: https://buildbot.libretro.com/nightly/

# You'll see cores organized by system:
# - Windows: x86_64-w64-mingw32/
# - macOS: apple/
# - Linux: linux/

# Download: fceumm_libretro.{so|dll|dylib}
```

**Official Core Download Sources:**
- **Primary**: https://buildbot.libretro.com/nightly/
- **Alternative**: https://github.com/libretro/libretro-fceumm/releases

**Platform-Specific Downloads:**
- **macOS**: `fceumm_libretro.dylib`
- **Linux**: `fceumm_libretro.so`
- **Windows**: `fceumm_libretro.dll`

#### 2. Get a Free NES Game (Homebrew)

Several free, open-source NES games are available:

**Option A: Flappy Paratroopa NES** (Flappy Bird clone)
```bash
git clone https://github.com/captain-http/flappy-paratroopa-nes.git
cd flappy-paratroopa-nes
# Build instructions in README
# Produces: flappy-paratroopa-nes.nes
```

**Option B: Other Free NES Homebrew Games**

From https://github.com/topics/nes-game (scroll down for releases):

1. **RoboRun-NES** - Platform game
   - https://github.com/jones-hm/roborun-nes
   - Look for `.nes` ROM in releases or build from source

2. **Petris** - Puzzle game about petting a dog
   - https://github.com/fixermark/petris
   - https://fixermark.itch.io/petris

3. **NES Breakout** - Breakout game
   - https://github.com/zorchenhimer/nes-breakout

4. **NES Runner** - Infinite runner game
   - https://github.com/zorchenhimer/nes-runner

5. **Falling** - Puzzle game
   - https://github.com/xram64/falling-nes

**Option C: Download Ready-Made ROMs**

Many sites host free homebrew games:
- https://itch.io/games/tag-nes
- Search GitHub for "nes-game" with ROM releases

#### 3. Try Your First Game

```bash
# Assuming you have:
# - Core: ~/cores/fceumm_libretro.so (or .dylib/.dll)
# - ROM: ~/games/flappy-paratroopa-nes.nes

./target/release/rustretro \
  --core ~/cores/fceumm_libretro.so \
  --rom ~/games/flappy-paratroopa-nes.nes \
  --scale 2
```

## Alternative Cores to Try

### 2. SNES (Super Nintendo) - snes9x

**Core Download:**
- From buildbot: `snes9x_libretro.{so|dll|dylib}`
- GitHub: https://github.com/libretro/snes9x

**Free Games:**
- Various SNES homebrew games on GitHub (search "snes-game")
- Demos and test ROMs

**Usage:**
```bash
./target/release/rustretro \
  --core ~/cores/snes9x_libretro.so \
  --rom ~/games/snes_game.smc \
  --scale 2
```

### 3. Game Boy Advance - mGBA

**Core Download:**
- From buildbot: `mgba_libretro.{so|dll|dylib}`
- GitHub: https://github.com/libretro/mgba

**Free Games:**
- Search "gba-game" on GitHub
- Many indie/homebrew projects available

**Usage:**
```bash
./target/release/rustretro \
  --core ~/cores/mgba_libretro.so \
  --rom ~/games/gba_game.gba \
  --scale 3
```

### 4. Genesis (Sega Genesis) - Genesis Plus GX

**Core Download:**
- From buildbot: `genesis_plus_gx_libretro.{so|dll|dylib}`
- GitHub: https://github.com/libretro/Genesis-Plus-GX

**Free Games:**
- Sonic homebrew games
- Various demos

**Usage:**
```bash
./target/release/rustretro \
  --core ~/cores/genesis_plus_gx_libretro.so \
  --rom ~/games/genesis_game.gen \
  --scale 2
```

## Directory Structure for Demo

```
~/Games/
├── Cores/
│   ├── fceumm_libretro.so
│   ├── snes9x_libretro.so
│   ├── mgba_libretro.so
│   └── genesis_plus_gx_libretro.so
├── ROMs/
│   ├── NES/
│   │   └── flappy-paratroopa-nes.nes
│   ├── SNES/
│   │   └── snes_game.smc
│   ├── GBA/
│   │   └── gba_game.gba
│   └── Genesis/
│       └── genesis_game.gen
└── Saves/
```

## Quick Download Script

```bash
#!/bin/bash

# Setup directories
mkdir -p ~/Games/{Cores,ROMs,Saves}

# Download cores (adjust URLs for your OS)
echo "Downloading cores..."
# For macOS
curl -o ~/Games/Cores/fceumm_libretro.dylib \
  https://buildbot.libretro.com/nightly/macos/fceumm_libretro.dylib

# For Linux (x86_64)
# curl -o ~/Games/Cores/fceumm_libretro.so \
#   https://buildbot.libretro.com/nightly/linux/fceumm_libretro.so

echo "Cloning free NES game..."
cd ~/Games/ROMs/NES
git clone https://github.com/captain-http/flappy-paratroopa-nes.git
cd flappy-paratroopa-nes
# Build instructions (may require cc65 toolchain)
# make
# This produces: flappy-paratroopa-nes.nes

echo "Done! Cores in ~/Games/Cores/, ROMs in ~/Games/ROMs/"
```

## Testing Your Setup

### Test 1: Core Loads
```bash
./target/release/rustretro \
  --core ~/Games/Cores/fceumm_libretro.so \
  --rom ~/Games/ROMs/NES/flappy-paratroopa-nes.nes

# Should print:
# - Core: FCEUmm
# - ROM: path to NES file
# - Starting emulation...
```

### Test 2: Input Works
Once running, you should be able to:
- Press arrow keys (D-pad)
- Press Z, X, A, S (buttons)
- Press Enter (Start)
- Press ESC (Quit)

### Test 3: Frame Rate
The game should run at proper NES speed (~60 FPS).

## Troubleshooting

### "Core API version mismatch"
- Ensure core version matches libretro API v1
- Download a newer core build from buildbot

### "Cannot load ROM"
- Verify ROM file exists: `ls -la ~/Games/ROMs/NES/`
- Use absolute paths: `/absolute/path/to/rom.nes`

### "SDL2 error"
- Install SDL2: `brew install sdl2` (macOS)
- Or: `apt-get install libsdl2-dev` (Linux)

### No video output
- This is expected! Video rendering is stubbed in current version
- The core is running, just not displaying to window
- This is a known limitation in the README

### No audio output
- Use `--no-audio` flag to suppress audio errors
- Audio integration is incomplete in current version

## Building NES Games from Source

### Prerequisites
```bash
# macOS
brew install cc65

# Linux
sudo apt-get install cc65

# Then clone and build
cd flappy-paratroopa-nes
make
```

## Free Game Collections

### GitHub NES Game Collections
1. Search: https://github.com/search?q=topic:nes-game
2. Filter by "Releases" to find compiled `.nes` files
3. Or download source and build

### itch.io NES Games
- https://itch.io/games/tag-nes
- Many free/open-source games available

### RetroArch Content Directory
- Many cores come with example content
- Available through RetroArch's downloader

## Next Steps

1. **Get a core**: Download from buildbot or GitHub
2. **Get a ROM**: Clone or download a free homebrew game
3. **Run RustRetro**: Follow the command examples
4. **Report issues**: Note that video/audio are incomplete features

## Legal Notes

- Libretro cores are open-source (MIT license)
- Homebrew games are free/open-source
- No copyrighted content needed for testing
- This guide uses only legal, freely-available software

## Resources

### Download Cores
- https://buildbot.libretro.com/nightly/
- https://github.com/libretro/ (search for specific cores)

### Find Free Games
- https://github.com/topics/nes-game
- https://itch.io/games/tag-nes
- Specific game repositories with releases

### Build Homebrew Games
- cc65 toolchain: https://cc65.github.io/
- NES development: https://wiki.nesdev.org/

### Libretro Documentation
- https://docs.libretro.com/
- https://www.libretro.com/

## Example: Complete Setup in 5 Minutes

```bash
# 1. Create directories
mkdir -p ~/Games/{Cores,ROMs/NES,Saves}

# 2. Download core (macOS example)
curl -L -o ~/Games/Cores/fceumm_libretro.dylib \
  "https://buildbot.libretro.com/nightly/macos/fceumm_libretro.dylib"

# 3. Get a free game
cd ~/Games/ROMs/NES
git clone https://github.com/captain-http/flappy-paratroopa-nes.git

# 4. Build the game (if needed, requires cc65)
cd flappy-paratroopa-nes
# Assuming Makefile exists, run: make

# 5. Run RustRetro
cd ~/path/to/rustretro
./target/release/rustretro \
  --core ~/Games/Cores/fceumm_libretro.dylib \
  --rom ~/Games/ROMs/NES/flappy-paratroopa-nes/flappy-paratroopa-nes.nes
```

## FAQ

**Q: Where do I get libretro cores?**
A: https://buildbot.libretro.com/nightly/ or GitHub releases from https://github.com/libretro/

**Q: Are there legal free ROMs?**
A: Yes! Homebrew games (created by fans) are free and legal. Many are on GitHub with releases.

**Q: Why no video output?**
A: Video rendering in SDL2 is stubbed out (incomplete feature). Core runs fine, just not displayed.

**Q: Why no audio?**
A: Audio playback is a known limitation. Use `--no-audio` flag.

**Q: Can I use commercial ROMs?**
A: We recommend using free homebrew games instead. For commercial games, check local laws.

**Q: What if my ROM won't load?**
A: Different cores support different ROM formats. Verify format matches the core's supported extensions.

---

**Happy retro gaming! 🎮**
