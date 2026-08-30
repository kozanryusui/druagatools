# AON.Net Game Database Site

The input databases are in `data/`. The generated item icons are in `item-icons/`. The generated quest minimaps are in `minimaps/`.

The source game files are in `work/resources/1.60/`. The decompiled quest scripts and the main controller area database are in `work/analysis/scpt/`. The complete extracted minimap set is in `work/analysis/minimaps/`.

Run these commands from the repository root.

```sh
cargo run -p druaga-utils --bin tower-source-builder -- \
  work/resources/1.60/tower/item/lottery.dat \
  work/resources/1.60/tower/item/present.dat \
  crates/aon-net/html/data/tower_sources.json

cargo run -p druaga-utils --bin sol-source-builder -- \
  work/analysis/scpt/decompiled \
  work/analysis/scpt/mainctrl-quest-areas.json \
  work/resources/1.60/station/map/mapname.dat \
  work/analysis/minimaps \
  crates/aon-net/html/data/items.json \
  crates/aon-net/html/data/chests.json \
  crates/aon-net/html/data/quest_sources.json \
  crates/aon-net/html/minimaps

cargo run -p druaga-utils --bin site-builder -- \
  crates/aon-net/html/data/items.json \
  crates/aon-net/html/data/alchemy.json \
  crates/aon-net/html/data/chests.json \
  crates/aon-net/html/data/enemies.json \
  crates/aon-net/html/data/quest_sources.json \
  crates/aon-net/html/data/tower_sources.json \
  crates/aon-net/html
```

Before you run `sol-source-builder`, extract each Station `map/*.gsm` file with `gsm2-extract`. Use the GSM base name for the PNG base name.
