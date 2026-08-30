# Druaga `sx32w.dll` Compatibility Library

This stub DLL supplies the Sentinel SuperPro functions that the Tower needs to run. It also runs tower-hook.dll.

## Build

Install the 32-bit Windows GNU target:

```text
rustup target add i686-pc-windows-gnu
```

Build the library:

```text
cargo build --release --target i686-pc-windows-gnu -p druaga-sx32w-shim
```

The output file is:

```text
target/i686-pc-windows-gnu/release/sx32w.dll
```

Copy `sx32w.dll` to the directory that contains `v324ct.exe` before you start the Tower.
