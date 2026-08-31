# AON.Net

AON.Net replaces the retired Druaga Online network services. It supports the Tower and Station clients from game version 1.60 and 1.65.

| Service | Default endpoint |
| --- | --- |
| ALL.Net PowerOn and administration | TCP port 80 |
| Database service | TCP port 33437 |
| Matching service | TCP port 33438 |
| Relay services | TCP ports 33439 through 33441 |
| Station gameplay relay | TCP port 33442 |

## Requirements

Install a current Rust toolchain. Add the WebAssembly target and install Trunk. The build uses Trunk to compile the administration interface.

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
```

## Create the configuration

Run this command from the repository root:

```bash
cp crates/aon-net/aon-net.example.toml aon-net.toml
```

Edit `aon-net.toml` before you make the server available on a network.

The `bind-ip` field selects the local Internet Protocol (IP) address for all services. The `http-port`, `game-port`, `matching-port`, `relay-ports`, and `gameplay-port` fields select their TCP ports. The `gameplay-advertise-host` and `gameplay-advertise-port` fields select the address that AON.Net sends to matched Stations. Stations must be able to resolve and reach this advertised address.

A relative `database-path` starts at the server working directory. AON.Net creates this database when it starts. The database stores card data backups, server settings, and changes from the administration interface.

## Build and start AON.Net

Build the release executable:

```bash
cargo build --release -p aon-net
```

The Station uses the fixed PowerOn port 80. On Linux, give the executable permission to bind this port:

```bash
sudo setcap cap_net_bind_service+ep target/release/aon-net
```

Start the server from the directory that contains `aon-net.toml`:

```bash
RUST_LOG=aon_net=info target/release/aon-net aon-net.toml
```

## Configure client discovery

The Tower hook can redirect the required host names. In `tower-hook.toml`, map these names to the AON.Net host:

```toml
[[network.dns-overrides]]
source = "naominet.jp"
domain = "localhost"

[[network.dns-overrides]]
source = "gameservers.aonnet"
domain = "localhost"
```

Use `ip` instead of `domain` when the server has a fixed IPv4 address.

The Station does not use the Tower hook. Add `naominet.jp` and `gameservers.aonnet` to the PCSX2 per-game DEV9 host list. Map both names to the AON.Net address. Set the first DEV9 DNS mode to `Internal`.

Allow TCP ports 80 and 33437 through 33442 through the server firewall. Do not expose these services to the public Internet without an additional access-control layer.

## Use the administration interface

Open this address after the server starts:

```text
http://SERVER_ADDRESS/admin
```

The interface changes the shop name, quest rotation, quest rewards, and quest bonuses. Its Logs page shows live server logs.

The administration interface has no login. Permit access only from a trusted network.

## Configuration notes

- `matching-player-count` accepts values from 2 through 4.
- `uri` and `host` supply required PowerOn response fields.
- The region fields and `place-id` supply the cabinet location.
- PowerOn text must be representable in Shift_JIS. It must not contain `&`, a null byte, or a line break.
- AON.Net accepts at most 16 announcements.
- Announcement times use `YYYY-MM-DD HH:MM`.
- Announcement text can contain at most 428 bytes after CP932 encoding.
