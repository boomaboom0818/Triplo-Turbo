# Triplo — Doppio engine

This checkout runs the complete [Doppio](https://github.com/mm-nair/Doppio) bot: its strategy, Open Poker client, simulator, models, and opponent database. `engine/` is a Git submodule pinned to the tested upstream revision. Fetching it requires access to the private Doppio repository.

## Build and run locally

Requires Git, stable Rust, and a C toolchain. From this repository:

```sh
git submodule update --init --recursive
./doppio --help
./doppio sim --strategy v3 --table mixed --hands 1000 --seed 42
./doppio sim --strategy v5 --table real-openpoker --hands 1000 --seed 42
```

The launcher builds the locked release on first use. It forwards arguments to Doppio and locates the shipped models in `engine/models/v4`. V3 is the upstream default; use `--strategy v5` to select V5 explicitly. To run the upstream tests:

```sh
scripts/cargo test --locked --release --manifest-path engine/Cargo.toml --target-dir target/doppio
```

## Live play

One client at a time per Open Poker key. After stopping the previous client for the intended seat, run:

```sh
OPENPOKER_KEY_FILE="$HOME/.config/triplo/openpoker.key" ./doppio play --strategy v3
```

Use the key file for **that seat**, or supply Doppio's native `OPENPOKER_API_KEY`. The default SQLite history is `data/doppio.db` relative to this repository; pass `--db` for a different location. `./doppio play --help` lists buy-in, hand limit, table selection, and other native options.
