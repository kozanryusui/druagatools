# Druaga Tools

Druaga Tools contains server and runtime compatibility software for Druaga Online. The project supports local installations on current Windows systems and on Linux systems with Wine.

This repository does not contain the original game executables. You must supply the required files from your own installation.

## Start here

- [Set up AON.Net](crates/aon-net/README.md) to run the local network services for Station versions 1.60 and 1.65 and Tower version 1.60.
- [Set up the Tower hook](crates/tower-hook/README.md) to run Tower version 1.60 on a current Windows or Wine system.

Start AON.Net before you start the Tower or Station clients.

## Player guide and databases

The [Druaga Online guide and database](crates/aon-net/html/index.html) contains the hidden treasure chest guide, item database, crafting database, enemy database, quest item sources, and Tower item sources.

Clone or download the repository, then open `crates/aon-net/html/index.html` in a web browser.

## Components

| Component | Purpose |
| --- | --- |
| `aon-net` | PowerOn, database, matching, relay, gameplay, and administration services |
| `tower-hook` | Tower 1.60 runtime compatibility, device emulation, storage, input, display, and network hooks |
| `sx32w-shim` | Sentinel SuperPro compatibility and Tower hook bootstrap |
| `tower-board` | Tower input/output board protocol and state machine |
| `utils` | Game data inspection, extraction, and site generation tools |

The code is version-specific. Do not use a component with another game version unless its README lists that version.

## License

This project uses the [GNU Affero General Public License version 3 only](LICENSE).

Druaga Online names and game assets belong to their respective rights holders. This project is not affiliated with or endorsed by those rights holders.
