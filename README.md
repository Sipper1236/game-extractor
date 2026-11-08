# game-extractor

Native Linux CLI for extracting FitGirl repacks (Inno Setup + FreeArc with custom CLS codecs) without running the Windows installer.

## Features

- **Hybrid backend architecture**: Native codecs (precomp, zstd, lz4, bzip3, xtool-linux) + Wine backend (`fitgirl-extractor.exe`) for closed-source CLS/Oodle/Razor codecs
- **Multi-threaded extraction**: Wine prefix pool runs N extractions in parallel (configurable via `GAME_EXTRACTOR_JOBS`)
- **CPU limiting**: `GAME_EXTRACTOR_NICE` (nice value) or `GAME_EXTRACTOR_CPU_LIMIT` (cpulimit %)
- **Parallel MD5 verification**: Semaphore-controlled concurrent checksumming (~24 min for 57 GB vs ~35 min sequential)
- **Per-bin resume checkpoints**: State saved to `~/.cache/game-extractor/<hash>.json`
- **Interactive content selection**: Choose main/optional/selective-VO bins via `--select` or interactive menu
- **Proton-ready output**: Extracted structure works directly with Steam/Proton

## Quick Start

```bash
# Build
cargo build --release

# Verify MD5 (parallel, ~24 min for CP2077)
./target/release/game-extractor verify "/path/to/Cyberpunk 2077 [FitGirl Repack]/"

# Extract optional content only (credits + bonus), 1 job, lower CPU priority
GAME_EXTRACTOR_JOBS=1 GAME_EXTRACTOR_NICE=10 \
  ./target/release/game-extractor install \
  --select optional \
  --no-verify \
  --output /mnt/games/Cyberpunk2077 \
  "/path/to/Cyberpunk 2077 [FitGirl Repack]/"

# Extract everything (main + optional + all VO), 2 parallel Wine prefixes
GAME_EXTRACTOR_JOBS=2 \
  ./target/release/game-extractor install \
  --select all \
  --output /mnt/games/Cyberpunk2077 \
  "/path/to/Cyberpunk 2077 [FitGirl Repack]/"

# Resume interrupted extraction
GAME_EXTRACTOR_JOBS=2 \
  ./target/release/game-extractor install \
  --select all \
  --resume \
  --output /mnt/games/Cyberpunk2077 \
  "/path/to/Cyberpunk 2077 [FitGirl Repack]/"
```

## Selection Syntax

| Argument | Meaning |
|----------|---------|
| `--select main` | Main game bins (fg-01.bin .. fg-08.bin) |
| `--select optional` | All optional bins (bonus, credits) |
| `--select optional-bonus` | Specific optional bin |
| `--select selective-english` | English VO only |
| `--select selective-english,selective-russian` | Multiple VO languages |
| `--select all` | Everything |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `GAME_EXTRACTOR_JOBS` | 2 | Max concurrent extractions (Wine prefixes created = this value) |
| `GAME_EXTRACTOR_NICE` | unset | Nice value (-20..19, higher = lower priority) |
| `GAME_EXTRACTOR_CPU_LIMIT` | unset | cpulimit % per process (requires `cpulimit` installed) |

## Requirements

| Tool | Required? | Install |
|------|-----------|---------|
| `wine` | **Yes** (Wine backend) | `pacman -S wine` / `apt install wine` |
| `innoextract` | Native backend only | `pacman -S innoextract` / `apt install innoextract` |
| `fitgirl-extractor.exe` | **Yes** (Wine backend) | **Download separately** (see below) |
| `cpulimit` | Optional (CPU limiting) | `pacman -S cpulimit` / `apt install cpulimit` |

### fitgirl-extractor.exe (Required)

**Not included in this repo** — download from [pisek/fitgirl-extractor releases](https://github.com/pisek/fitgirl-extractor/releases):

```bash
# v0.2.1 (tested)
wget -O /tmp/fitgirl-extractor.exe https://github.com/pisek/fitgirl-extractor/releases/download/v0.2.1/fitgirl-extractor.exe
chmod +x /tmp/fitgirl-extractor.exe
```

The tool expects it at `/tmp/fitgirl-extractor.exe`. To use a different location, edit `src/backend/wine.rs:167` and rebuild.

Place `fitgirl-extractor.exe` at `/tmp/fitgirl-extractor.exe` or update the path in `src/backend/wine.rs`.

## Architecture

```
game-extractor CLI
├── Interactive selection (main/optional/selective-<lang>)
├── MD5 verification (tokio::sync::Semaphore(2))
├── Per-bin resume checkpoints (~/.cache/game-extractor/)
└── Proton-ready output directory
    │
    ├── Native Backend (open-source codecs)
    │   ├── precomp, zstd, lz4, bzip3, xtool-linux
    │   └── Can handle: native-only repacks
    │
    └── Wine Backend (CLS/Oodle/Razor)
        ├── fitgirl-extractor.exe via Wine
        ├── WinePrefixPool (N prefixes = JOBS)
        ├── GPU env vars: DXVK_HUD=0, VKD3D_CONFIG=dxr11
        └── Can handle: FitGirl repacks (CP2077, etc.)
```

## CP2077 FitGirl Repack

**100% Wine backend required** — every bin uses:
- `cls-srep.dll` + `cls-srep_x64.exe` (deduplication)
- `cls-lollypop.dll` + `cls-lollypop_x64.exe` (delta encoding)
- `cls-magic2.dll` + `cls-magic2_x64.exe` (texture/model transforms)
- `oo2reck.exe` (Oodle Kraken)
- `rz-1.03.7-stdio.exe` (Razor)
- `xtool.exe` / `rtool.exe` (game asset transforms)

No GPU acceleration possible — all codecs are CPU-bound, branch-heavy algorithms.

## Output Structure

```
/target/
├── app/                    # Game files
├── _Redist/                # Redistributables (DirectX, VC++)
├── engine/                 # Engine binaries
├── bin/                    # Game executables
├── r6/                     # REDengine 6 data
└── steam_appid.txt         # For Proton/Steam
```

Launch via: `proton run bin/launcher.exe` or add to Steam as non-Steam game.

## License

MIT