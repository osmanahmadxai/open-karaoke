# Croonly 🎤

**Open-source karaoke studio for macOS, Windows and Linux.** Search any song, press **Start singing**, and sing over its instrumental with synced lyrics. A low-latency autotune, harmonizer and vocal effects chain processes your voice as you sing.

All Croonly needs is an internet connection: no accounts, no API keys, no extra installs.

## Download

**[⬇ Download Croonly](https://osmanahmadxai.github.io/croonly/)**: one link for every platform. It picks the right installer for your computer.

| macOS (Apple Silicon + Intel) | Windows 10/11 | Linux |
|---|---|---|
| [Croonly-macOS.dmg](https://github.com/osmanahmadxai/croonly/releases/latest/download/Croonly-macOS.dmg) | [Setup.exe](https://github.com/osmanahmadxai/croonly/releases/latest/download/Croonly-Windows-Setup.exe) · [.msi](https://github.com/osmanahmadxai/croonly/releases/latest/download/Croonly-Windows.msi) | [AppImage](https://github.com/osmanahmadxai/croonly/releases/latest/download/Croonly-Linux.AppImage) · [.deb](https://github.com/osmanahmadxai/croonly/releases/latest/download/Croonly-Linux.deb) · [.rpm](https://github.com/osmanahmadxai/croonly/releases/latest/download/Croonly-Linux.rpm) |

The builds are not code-signed yet. On macOS, right-click the app and choose **Open** the first time. On Windows, choose **More info → Run anyway** if SmartScreen appears.

## Features

### Songs & lyrics
- **Search anything.** The song catalogue comes from the iTunes Search API, with Deezer as a fallback.
- **Automatic instrumental finder.** Croonly searches YouTube for instrumental, karaoke and backing-track versions and ranks them. It penalises covers, remixes, "slowed + reverb" edits and versions with the wrong length. Every candidate can be swapped from the **Song** tab.
- **Vocal-remover fallback.** If no clean instrumental exists, Croonly strips the centre-panned lead vocal from the original recording and keeps the bass and kick.
- **Time-synced lyrics** from [LRCLIB](https://lrclib.net), with a smooth word-by-word wipe, a countdown before each verse, and support for word-timed ("enhanced") LRC.
- **Lyric auto-sync.** Instrumentals often start a few seconds before or after the original, so lyrics would drift. Croonly downloads the original in the background and cross-correlates the rhythm of the two recordings to find the exact offset. You can nudge the timing with `[` / `]`.
- **Guide vocal.** Mix in the original singer quietly while you learn a song.
- **Key & tempo change.** Shift the backing track ±6 semitones or play it at 70–130 % speed (Signalsmith Stretch, studio quality). The autotune key follows.

### Real-time autotune (written from scratch in Rust)
- **Pitch tracking:** McLeod Pitch Method with FFT autocorrelation, every ~4 ms, 65 Hz – 1.2 kHz.
- **Pitch shifting:** TD-PSOLA. It preserves formants, so a corrected voice still sounds like you and not a chipmunk. Voice latency is about 10 ms plus your audio buffer.
- **Automatic key detection** of every song (Krumhansl–Kessler), or pick any of 12 scales manually: major, minor, harmonic/melodic minor, modes, pentatonics, blues, chromatic.
- **Retune speed** from instant (the classic hard-tune effect) to slow and natural.
- **Humanize**: sustained notes get looser correction.
- **Flex-tune**: leaves small expressive deviations alone.
- **Note hysteresis**, so wobbles near a boundary don't flip between notes.
- **Correction amount**, **detection threshold**, and **synthetic vibrato** that fades in on held notes.
- **Pitch shift and independent formant shift**, from chipmunk to monster.
- **Diatonic harmonizer** with two voices (3rds, 4ths, 5ths, 6ths, octaves, always in key), stereo spread, and a **doubler**.
- **Presets:** Natural, Pop polish, Hard tune, Choir, Diva, Chipmunk, Monster.

### Vocal channel strip
- Noise gate with hold, low-cut, and a three-band EQ (warmth, presence, air)
- Split-band de-esser and a one-knob compressor
- Stereo ping-pong echo and a Freeverb-style reverb with pre-delay
- Presets: Dry, Studio, Arena, Club echo, Radio

### Voice Lab
- Test your sound before you pick a song: sing live and watch the pitch graph.
- Record a take of up to 30 seconds, then loop it while you tweak the autotune and effects, with an A/B switch to hear the original.
- A **key drone** plays a soft chord in your chosen key to sing against.
- Save any combination of settings to **My presets** and use it in every song. Export the processed take as a WAV.

### Performance
- **Live pitch graph**: your raw voice against the tuned voice, over the scale's note lanes, with a tuner readout.
- **Scoring**: pitch accuracy, points, streaks and a grade at the end of the song.
- **Recording**: 24-bit WAV of the full mix, with automatic latency compensation so your voice lines up with the music. Files are saved to `Music/Croonly`.

## Architecture

```
┌──────────────────────────── React + TypeScript UI ────────────────────────────┐
│ search · stage · lyrics wipe · pitch graph · panels       (src/)              │
└──────────────── Tauri IPC: commands + 30 fps "engine://tick" events ──────────┘
┌──────────────────────────────── Rust core (src-tauri/) ───────────────────────┐
│ audio/engine.rs   cpal input & output streams, lock-free rings, recorder      │
│ audio/chain.rs    gate → pitch tracking → autotune controller → PSOLA voices  │
│                   → EQ → de-esser → compressor → echo → reverb                │
│ audio/pitch.rs    McLeod pitch detector      audio/psola.rs  pitch shifter    │
│ audio/analysis.rs key detection, lyric alignment, vocal remover               │
│ audio/stretch.rs  key/tempo (Signalsmith)    audio/decode.rs Symphonia+rubato │
│ youtube.rs  yt-dlp search/download   meta.rs  iTunes/Deezer/LRCLIB            │
│ tools.rs    first-run download of yt-dlp + Deno from their GitHub releases    │
└───────────────────────────────────────────────────────────────────────────────┘
```

**Why Rust and not a browser audio stack?** A singer hears their own voice through the app. The Web Audio API typically adds 30–50 ms of round-trip delay, which is enough to throw off your timing. Native audio through cpal, with DSP that never locks or allocates on the audio thread, keeps that delay low.

## Running it

Prerequisites (developers only, not end users): [Rust](https://rustup.rs), Node.js 20+, and the [Tauri system dependencies](https://v2.tauri.app/start/prerequisites/). On Linux you also need `libasound2-dev` and `libclang-dev`.

```bash
npm install
npm run app     # development build with hot reload
npm run dist    # installers for your platform → src-tauri/target/release/bundle/
cd src-tauri && cargo test   # DSP unit tests
```

### Releasing
Push a version tag and GitHub Actions builds installers for every platform, publishes the release, and attaches files with fixed names so that the download page always serves the newest version:

```bash
# bump "version" in src-tauri/tauri.conf.json, package.json and src-tauri/Cargo.toml, then:
git tag v0.2.0 && git push origin v0.2.0
```

The download page lives in `docs/` and is served by GitHub Pages.

On first launch Croonly downloads [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [Deno](https://deno.com) (about 70 MB, one time) into its app-data folder, and keeps yt-dlp updated daily.

## Tips
- **Use headphones.** With speakers, the microphone picks up the music and your processed voice. Turn off **Mix → Hear myself** if you sing through speakers.
- Lower **Settings → Buffer size** for less delay. Raise it if you hear crackles.
- Keyboard: `Space` play/pause · `←/→` seek · `[` `]` lyric timing · `F` fullscreen · `Tab` controls · `Esc` back.

## Legal
Croonly is a player. It streams audio from YouTube via yt-dlp and lyrics from LRCLIB at the user's request, and does not host or redistribute any content. Respect the terms of service of these platforms and the copyright laws in your country. Recordings you make are for personal use.

## License
MIT. See [LICENSE](LICENSE).
