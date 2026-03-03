# chiaki

Safe Rust bindings for [chiaki-ng](https://github.com/streetpea/chiaki-ng) — an open-source PlayStation Remote Play client.

This crate wraps the chiaki-ng C core library and exposes a safe, idiomatic Rust API for:

- **LAN discovery** of PS4/PS5 consoles
- **Registration** (PIN-based credential exchange)
- **Streaming sessions** (H.264/H.265 video, Opus audio)
- **Controller input** (DualShock 4 / DualSense state)
- **Microphone** encoding and streaming
- **DualSense haptics** (audio path + rumble fallback) *(feature: `sdl-controller`)*
- **PSN OAuth2 authentication** *(feature: `psn-auth`)*

The low-level FFI layer is in the companion crate [`chiaki-sys`](https://crates.io/crates/chiaki-sys).

---

## Feature Flags

| Feature | Enables |
|---------|---------|
| *(default)* | Core: discovery, regist, session, controller, mic, stats |
| `sdl-controller` | SDL2 gamepad integration, haptics, `StreamController` orchestrator |
| `psn-auth` | PSN OAuth2 token exchange via `reqwest` |

---

## Quick Start

```toml
[dependencies]
chiaki = "0.1"
```

```rust
use chiaki::{Log, LogLevel, DiscoveryService, DiscoveryOptions};

let log = Log::new_default(LogLevel::ALL);
let svc = DiscoveryService::start(DiscoveryOptions::default(), log, |host| {
    println!("Found: {:?} @ {:?}", host.state, host.host_addr);
})?;
// Service stops when `svc` is dropped
```

---

## Building

The `chiaki-sys` build script compiles the chiaki-ng C core library via CMake.
All C library dependencies must be present before running `cargo build`.

### Linux

```bash
# Debian/Ubuntu
apt install libopus-dev libssl-dev libjson-c-dev libminiupnpc-dev libssh2-dev \
            cmake ninja-build pkg-config

cargo build -p chiaki
```

### macOS

```bash
brew install opus openssl@3 json-c miniupnpc libssh2 cmake ninja pkg-config

cargo build -p chiaki
```

### Windows

The build uses **MSYS2/MinGW-w64** (gcc + Ninja), matching the official CI workflow.
All steps below must be run inside the **MSYS2 MinGW 64-bit** shell.

**1. Install MSYS2**

Download and install from [msys2.org](https://www.msys2.org/).

> **Important:** All commands below must be run in the **"MSYS2 MinGW x64"** shell
> (prompt shows `MINGW64`), **not** the plain "MSYS2" shell (prompt shows `MSYS`).

**2. Install C library dependencies**

```bash
pacman -Syu
pacman -S mingw-w64-x86_64-gcc mingw-w64-x86_64-cmake mingw-w64-x86_64-ninja \
          mingw-w64-x86_64-openssl mingw-w64-x86_64-opus mingw-w64-x86_64-json-c \
          mingw-w64-x86_64-miniupnpc mingw-w64-x86_64-libssh2 mingw-w64-x86_64-libidn2 \
          mingw-w64-x86_64-pkgconf mingw-w64-x86_64-python-protobuf
```

**3. Install Rust with the MinGW target**

```bash
# Install rustup if not already present
pacman -S mingw-w64-x86_64-rust

# Or, if using a system-wide rustup installation, add the GNU target:
rustup target add x86_64-pc-windows-gnu
rustup default stable-x86_64-pc-windows-gnu
```

**4. Build**

```bash
git submodule update --init --recursive   # from repo root
cd bindings/rust
cargo build -p chiaki --target x86_64-pc-windows-gnu
```

> **First build** compiles the C core library via CMake (gcc + Ninja) and
> takes 5–20 minutes. Subsequent builds use Cargo's incremental cache.

---

## Examples

```bash
cargo run -p chiaki --example discovery
cargo run -p chiaki --example regist
cargo run -p chiaki --example session

# SDL2 controller features (on Windows: pacman -S mingw-w64-x86_64-SDL2)
cargo run -p chiaki --features sdl-controller --example controllermanager
cargo run -p chiaki --features sdl-controller --example stream_and_control
cargo run -p chiaki --features sdl-controller --example dualsense_control
```

---

## Architecture

```
chiaki/src/
├── lib.rs            # Module declarations & re-exports
├── error.rs          # Error enum + ffi_result() helper
├── types.rs          # Target, Codec, QuitReason, DualSenseEffectIntensity
├── log.rs            # Log + LogLevel (closure-based, thread-safe)
├── discovery.rs      # LAN console discovery (UDP multicast)
├── regist.rs         # Registration protocol (PIN-based)
├── session.rs        # Streaming session, events, AudioSink trait
├── controller.rs     # ControllerState, ControllerButtons, Touch
├── mic.rs            # MicEncoder (Opus PCM → network)
├── stats.rs          # StreamStats (bitrate, packet_loss)
│
│   # Feature-gated: --features sdl-controller
├── controllermanager.rs  # SDL2 gamepad integration & hotplug
├── feedback.rs           # Session Event → FeedbackCmd mapping
├── haptics.rs            # DualSense haptics
└── stream.rs             # StreamController high-level orchestrator
```

---

## License

MIT — see [LICENSE](https://github.com/streetpea/chiaki-ng/blob/main/LICENSE) in the chiaki-ng repository.
