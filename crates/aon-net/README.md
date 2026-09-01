# AON.Net

AON.Net replaces the retired Druaga Online network services. It supports the Tower and Station clients from game version 1.60 and 1.65.

| Service | Default endpoint |
| --- | --- |
| ALL.Net PowerOn | TCP port 80 |
| Administration with optional security | TCP port 80 or HTTPS port 443 |
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

`http-connection-limit` limits active connections on each HTTP listener. PowerOn and secure administration use separate limits when admin security is enabled. `game-connection-limit` limits the combined active connections on all Tower and Station TCP services. AON.Net stops accepting game connections when this limit is full.

`http-request-timeout-seconds` limits HTTP request processing. It does not close an established administration event stream. `http-body-limit-bytes` limits PowerOn and administration request bodies. `tower-connection-timeout-seconds` limits Tower reads and writes.

A relative `database-path` starts at the server working directory. AON.Net creates this database when it starts. The database stores card data backups, server settings, and changes from the administration interface.

The `[admin-security]` section controls Transport Layer Security (TLS) and authentication for the administration interface. Keep `enabled = false` for local hosting. Set it to `true` before you make the administration interface available on the public Internet. The enabled mode requires `tls-public-cert`, `tls-private-key`, and `admin-token`. The token must contain at least 32 bytes.

`tls-public-cert` must point to a PEM certificate chain. `tls-private-key` must point to its PEM private key. Relative paths start at the server working directory. Restrict access to the configuration file and private key.

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
target/release/aon-net aon-net.toml
```

AON.Net uses the `INFO` log level by default. Set `RUST_LOG` to select a different log filter.

AON.Net adds compatible Stations to a party until the party has four players. If the matching wait expires first, AON.Net starts the current partial party. A Station cannot join a party after it starts.

AON.Net cancels a fixed-roster party if a Station disconnects before the final participant exchange or before all assigned Stations join gameplay. A normal matching close after the final exchange keeps the gameplay assignment valid.

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

Allow TCP ports 80 and 33437 through 33442 through the server firewall. Also allow TCP port 443 when admin security is enabled. The game protocols do not support authentication. Use firewall rules and connection limits when you make the game services available on the public Internet.

## Use the administration interface

When admin security is disabled, open this address after the server starts:

```text
http://SERVER_ADDRESS/admin
```

When admin security is enabled, open this address and enter the configured admin token:

```text
https://SERVER_ADDRESS/admin
```

The interface changes the shop name, quest rotation, quest rewards, and quest bonuses. Its Logs page shows live server logs.

When admin security is enabled, AON.Net keeps authenticated sessions in memory. A server restart ends all sessions. The web app does not put the token in browser storage. The login field supports password managers.

## Configuration notes

- `uri` and `host` supply required PowerOn response fields.
- `gameplay-relay-queue-capacity` limits queued gameplay records for each Station. The default is 4096. AON.Net disconnects a Station if its queue becomes full.
- `gameplay-player-timeout-seconds` limits gameplay reads and writes. The default is 10 seconds. AON.Net disconnects a Station that stops sending data or does not accept a write before the timeout.
- `matching-player-timeout-seconds` limits matching reads, matching writes, and the handoff to gameplay. The default is 10 seconds.
- `http-connection-limit` defaults to 64 connections for each HTTP listener.
- `game-connection-limit` defaults to 256 connections shared by all game TCP listeners.
- `http-request-timeout-seconds` defaults to 10 seconds.
- `http-body-limit-bytes` defaults to 65536 bytes.
- `tower-connection-timeout-seconds` defaults to 30 seconds.
- The region fields and `place-id` supply the cabinet location.
- PowerOn text must be representable in Shift_JIS. It must not contain `&`, a null byte, or a line break.
- AON.Net accepts at most 16 announcements.
- Announcement times use `YYYY-MM-DD HH:MM`.
- Announcement text can contain at most 428 bytes after CP932 encoding.
