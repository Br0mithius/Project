# MSH Deckbuilder

A deck builder for the **Mulligans Island Commander League (Season 2)**: build a commander deck from your
league pool, with official card images, league rules, a draft helper and trade planning. The card database covers
every set in the Season 2 rules: **Marvel Super Heroes**, **The Hobbit**, **Reality Fracture**, **Foundations** (the
Public Pool starter kit), the league staples and Shop cards, and **Nauctis: The Sunken Realms** once it is released
(Settings > "Update card database").

The repository ships with an **empty pool**. Import yours from Mana Mixer: drag the export file onto the app window, or paste it into the
My Pool tab's import box with Ctrl+V, then press "Add to my pool" or "Replace my pool", or with `import <file> [replace]` in the command-line app. Lines like
`1 Red Hulk (MSH) 149`, `2x Web Up`, and CSV files with Name and Quantity columns are understood; "replace" keeps a
copy of the old pool in `data/collection.backup.txt`.

## Windows

Download `dist/MSH-Deckbuilder-windows.zip`, unzip it, and double-click **MSH Deckbuilder.exe**.
See `dist/README-windows.txt` for a tour. To rebuild the zip from Linux: `tools/build-windows.sh`.

## The app

| Tab | What it does |
| --- | --- |
| Deck Builder | Pool browser (filters by colour, type, role, mana value), deck as card stacks or a list, card inspector, auto-build, basic-land controls, suggestions |
| Commanders | Every legendary creature you own, ranked 0-100 by the deck your pool gives it (seven weighted parts you can adjust, including simulated 4-player games), with a preview of the ranking once singleton starts; one-click build, Prismatic Piper option |
| Draft Helper | Type a pack's cards and get them ranked for your deck - or, with no commander yet, for your top-ranked commanders' decks (fit, upgrade over the weakest card, VP value, raw strength); "I took it" adds to your pool |
| Trades & VP | Trade-in plan for the 10 VP minimum (with the rarity floor), VP prices, Skill Tree costs, Shop |
| Upgrades | Cards that would improve your deck: what to claim from the Public Pool now (VP cost, rarity floor, a trade-in plan to pay for it) and what could come in the packs still to be handed out, with the odds that you, your draft table, or anyone in the league opens it |
| Stats & Playtest | Mana curve vs. targets, colour sources, roles, weakest cards, 20,000-hand simulation, sample hands |
| Season & Rules | The Season 2 schedule; picking a session sets deck size (60/75) and singleton |
| My Pool | Import from Mana Mixer, add/remove cards you own, copy your pool for Mana Mixer |

Card images (full art, high resolution) come from [Scryfall](https://scryfall.com). Each one is downloaded the first
time it's needed and saved in the `images` folder next to the program (`Card Name (MSH 123).jpg`); after that it is
always read from disk. Grids use smooth reduced-size copies, the full-resolution image loads when you rest the mouse on
a card (hover zoom) or inspect it, and only a few images are sent to the graphics card per frame so scrolling stays
smooth. If an image can't be fetched, the app draws a stand-in card face.

## Online data (downloaded once, re-pulled only on request)

| Source | Used for | When it's downloaded |
| --- | --- | --- |
| [17Lands](https://www.17lands.com) | Games-in-hand win rate per card, a variable in card strength | Once, on first start; then only via Settings > "Update ratings now" |
| [Playgroup.gg](https://playgroup.gg) | Commander win rates in recorded multiplayer games | Same as 17Lands (one gentle request at a time, stops early if the site doesn't answer) |
| [Scryfall](https://scryfall.com) | Card images; new league cards | Each image once, then read from `images/`; cards only via "Update card database" |

Everything downloaded is saved in `data/` (`ratings_17lands.json`, `commander_stats.json`), so the app never asks the
sites for the same data twice unless you press a button.

## Running from source

```bash
cargo run --release                 # the windowed app
cargo run --release --bin msh-cli   # the same tools as a typed command line
```

Command-line examples: `commanders`, `auto madame hydra`, `season 6`, `draft red hulk; web up; truck toss`,
`trade 10 rare`, `export mixer`, `import pool.txt`, `ratings`, `update-cards`, `why red hulk`, `upgrades`,
`publicpool pool.txt`, `help`. `cargo run --release --example rank_timing -- data` times the ranking's parts.

## Files

| File | What it is |
| --- | --- |
| `data/cards.json` | Card database (cost, type, rules text, P/T, rarity, colour identity). |
| `data/collection.txt` | The cards you own, one `count name` per line (empty until you import). Edit by hand or with `own` / `unown`. |
| `data/ratings_17lands.json`, `data/commander_stats.json` | Saved 17Lands and Playgroup.gg data. |
| `data/public_pool.txt` | The league's Public Pool, pasted from Mana Mixer (Upgrades tab or `publicpool <file>`). |
| `data/rules.json` | Format rules, created when you change one with `rules <key> <value>`. |
| `decks/*.txt` | Saved decks (`decks/_current.txt` is the auto-saved working deck). |

## Format rules (defaults - change with `rules`)

| Rule | Default |
| --- | --- |
| `deck_size` | 60 |
| `commander_counts_toward_size` | true (the commander is one of the 60) |
| `singleton` | false until Session 6 |
| `piper_free` | true (The Prismatic Piper needn't be in your pool) |
| `max_copies` | 0 = only limited by what you own |
| `unlimited_basics` | true |
| `enforce_color_identity` | true |
| `enforce_collection` | true |
| `starting_life` | 30 |

## Card data accuracy

Every card's rules text, cost, type and power/toughness is checked against the
[Forge](https://github.com/Card-Forge/forge) card database, with collector numbers and rarities from its set lists.
After adding new cards to `data/cards.json`, re-check them with:

```bash
python3 tools/verify_with_forge.py           # show differences
python3 tools/verify_with_forge.py --write   # apply them
python3 tools/verify_with_forge.py --import-set HOB --write   # add a whole league set
```

A `?` after a card name in the app means its text hasn't been verified (for example, a card typed in by hand with
`newcard`).

## How the advice works

* **Card strength** blends a rarity guess with the card's 17Lands win rate compared with its set's average; the more
  games 17Lands has recorded, the more the real data counts. Roles found in the rules text (removal, card draw, board
  wipes, "each opponent" effects that are better with 4 players...), creature stats for the cost, curve fit and synergy
  with your commander (what its abilities care about) are added on top.
* **Deck plans**: each commander's cost, body and rules text decide what kind of deck it wants - aggro (cheap
  attackers, combat triggers: lower curve, 18-24 creatures), midrange, big mana (expensive payoffs: higher curve, fewer
  small creatures) or spells (instant/sorcery payoffs: 9-14 creatures, more removal and draw). Auto-build, the
  Stats tab's curve targets and the ranking all use that plan.
* **Commander ranking** auto-builds a full deck for each commander (and each mono-colour commander with The Prismatic
  Piper in its two best extra colours) and grades it on seven parts - Cards, Curve, Creatures, Interaction, Mana,
  Commander (17Lands, Playgroup.gg win rate, abilities, cost) and Playtest. Parts are put on one scale (how far above or
  below your middle option, using the median so one freak option can't distort the rest), the sliders set how much each
  part counts, and the total runs 0-100 with 50 = your middle option. Before Session 6 the same ranking is also run
  under singleton rules as a preview. `why <commander>` in the command line shows the breakdown.
* **Playtest** plays each deck in 1,000 simplified 4-player games against decks auto-built from simulated league pools
  (random packs, the same number you have by this point of the season). The simulator understands lands and mana
  rocks, creature combat and keywords, removal, board wipes, card draw, tokens, drains, life gain, anthems and
  Equipment; it ignores counterspells, tricks and unusual abilities, so treat it as a second opinion.
* **Upgrades / pack odds** use Play Booster contents: 7 commons, 3 uncommons, a rare or mythic (mythic about 1 in 7),
  plus a wildcard and a foil of any rarity at typical published rates. "Your table" means the card is opened at your
  4-player draft table (so it's in your face-up draft); "league" means any of the 8 players opens it.
* **Lands**: the league's 25-27 at 60 cards (scaled for 75), low end for cheap curves; basics are split by coloured pips.
* **Simulation** shuffles the deck thousands of times to estimate opening-hand land counts, land drops by turn, and the
  odds of casting your commander on curve.

## Development

```bash
cargo test
```
