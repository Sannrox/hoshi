# TUI portrait assets

Original 8×8 pixel portraits and matching ASCII drawings for hoshi. All files
in `assets/portraits/` are original work, copyright the hoshi authors, licensed
Apache-2.0 (see `/LICENSE`).

| File | Slot | Mood | Notes |
| --- | --- | --- | --- |
| `commander-*.txt` / `.rgb` | player commander | neutral, pleased, alarmed | Overview when no raid |
| `admiral-neutral.txt` / `.rgb` | fleet admiral | neutral (other moods fall back) | Shipyard |
| `pirate-*.txt` / `.rgb` | pirate captain | neutral, pleased, alarmed | Overview after a raid event |
| `planet-rocky.txt` / `.rgb` | rocky planet | — | Planet screen |
| `planet-ice.txt` / `.rgb` | ice planet | — | Planet screen |
| `planet-gas_giant.txt` / `.rgb` | gas giant | — | Planet screen |
| `planet-rare.txt` / `.rgb` | rare planet | — | Planet screen when a rare bonus is present |
| `moods.conf` | — | — | Event-log substring → slot mood |

No third-party art is vendored. The pack must stay under 32 KiB, and each file
under 2 KiB (`scripts/check-portrait-assets.sh` and `portraits::tests::asset_budget_holds`).
