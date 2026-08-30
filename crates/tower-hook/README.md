# Druaga Tower hook

The Tower hook adds the runtime services that Druaga Online Tower 1.60 needs on a current Windows or Wine system. It supplies the serial devices, file-backed integrated circuit cards, SRAM storage, network redirection, display handling, and operator controls.

The setup uses two DLLs:

- `sx32w.dll` replaces the required Sentinel SuperPro library and starts the hook.
- `tower-hook.dll` installs the Tower runtime hooks.

Use the hook only with Tower version 1.60.

## Requirements

For a Wine build, install the Rust 32-bit Windows GNU target and an i686 MinGW-w64 linker:

```bash
rustup target add i686-pc-windows-gnu
```

## Build the DLLs

Run this command from the repository root:

```bash
cargo build --release --target i686-pc-windows-gnu \
  -p druaga-sx32w-shim -p druaga-tower-hook
```

Cargo writes these files:

```text
target/i686-pc-windows-gnu/release/sx32w.dll
target/i686-pc-windows-gnu/release/tower_hook.dll
```

## Install the hook

Copy both DLLs to the directory that contains `v324ct.exe`. Rename `tower_hook.dll` during the copy:

```bash
cp target/i686-pc-windows-gnu/release/sx32w.dll /path/to/tower/sx32w.dll
cp target/i686-pc-windows-gnu/release/tower_hook.dll /path/to/tower/tower-hook.dll
```

Copy the example configuration to the same directory:

```bash
cp crates/tower-hook/tower-hook.example.toml /path/to/tower/tower-hook.toml
```

The shim finds `tower-hook.dll` beside `v324ct.exe`. The hook finds `tower-hook.toml` beside its own DLL. If the configuration file does not exist, the hook uses its defaults.

## Connect the Tower to AON.Net

Start AON.Net before you start the Tower. The example configuration maps the Tower service names to `localhost`. If AON.Net runs on another computer, replace the `naominet.jp` and `gameservers.aonnet` targets with its IPv4 address:

```toml
[[network.dns-overrides]]
source = "naominet.jp"
ip = "192.0.2.10"
```

## Start the Tower

Set the working directory to the Tower directory. This location stores `druaga_sram.bin` and the default log.

```bash
cd /path/to/tower
DRUAGA_SX32W_LOG=tower.log wine ./v324ct.exe
```

The hook creates the configured card directory when you select a card that does not exist. It creates a factory-empty card image.

## Operator controls

| Key | Action |
| --- | --- |
| `F1` | Service switch |
| `F2` | Test switch |
| Up and Down | Select an item |
| Space | Confirm an item |
| Backspace | Insert a coin |
| `1` through `5` | Mount `card1.bin` through `card5.bin` in the left reader |
| `6` through `9`, then `0` | Mount the matching card file in the right reader |

## Configuration notes

- `display.mode` accepts `windowed` or `original`.
- Do not set `monitor`, `width`, `height`, or `refresh-hz`. The current hook rejects these reserved fields.
- `startup.skip-notice` skips the ten-second legal notice.
- `startup.skip-logos` skips the Namco and Arika logo sequence.
- A relative `cards.directory` starts at the directory that contains `tower-hook.toml`.
- `cards.reset-usage-count` resets the remaining-use count to 100 before each character-card write.
- `network.adapter = "dynamic"` supplies values from the current host adapter.
- `network.router-checks = "emulated"` avoids the raw ICMP socket requirement under Wine.
- `network.disable-maintenance-window` permits the service during the daily maintenance window.
- Each DNS override must set one `domain` or `ip` target.
- The logging switches select serial frame logging for the input/output board and both card readers.

Set `DRUAGA_TOWER_DISPLAY_MODE` to `windowed` or `original` to override the configured display mode. Set `DRUAGA_SX32W_LOG` to change the log path.

If the shim cannot load or initialize `tower-hook.dll`, it shows an error and writes the reason to the log. Check the DLL names, the target architecture, and `tower-hook.toml` first.
