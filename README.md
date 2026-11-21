# Triplo Turbo

Triplo Turbo is a Rust poker bot for Open Poker, with an offline simulator and a persistent opponent database. The strategy and models come from a pinned private engine dependency. Its source and URL are not included in the public repository.

## Build and run locally

Requires Git, Python 3.12+, stable Rust, and a C toolchain. From a fresh checkout, configure the private engine Git URL provided to authorized collaborators:

```sh
TRIPLO_TURBO_ENGINE_URL='<private engine Git URL>' ./triplo-turbo setup
./triplo-turbo --help
./triplo-turbo sim --strategy v3 --table mixed --hands 1000 --seed 42
./triplo-turbo sim --strategy v5 --table real-openpoker --hands 1000 --seed 42
```

Setup saves the URL in local Git configuration only. If the engine is already checked out, run `./triplo-turbo setup` to reuse its configured remote. Run setup again when the pinned engine revision changes. The launcher builds a local Triplo Turbo release without editing the private checkout and defaults to the bundled models. V3 is the default; use `--strategy v5` to select V5 explicitly. Run the engine tests with:

```sh
./triplo-turbo test
```

## Live play

One client at a time per Open Poker key. After stopping the previous client for the intended seat, run:

```sh
OPENPOKER_KEY_FILE="$HOME/.config/triplo/openpoker.key" ./triplo-turbo play --strategy v3
```

Use the key file for **that seat**, or supply `OPENPOKER_API_KEY`. The default SQLite history is `data/triplo-turbo.db` relative to this repository. To keep an existing engine database, pass its path with `--db`; the launcher does not move or overwrite it. `./triplo-turbo play --help` lists buy-in, hand limit, table selection, and other options.
