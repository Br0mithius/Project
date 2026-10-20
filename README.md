# MSH Deckbuilder

A deck builder for the **Mulligans Island Commander League (Season 2)**: build a commander deck from your
**Marvel Super Heroes** league pool, with official card images, league rules, a draft helper and trade planning.

## Windows

Download `dist/MSH-Deckbuilder-windows.zip`, unzip it, and double-click **MSH Deckbuilder.exe**.
See `dist/README-windows.txt` for a tour. To rebuild the zip from Linux: `tools/build-windows.sh`.

## The app

| Tab | What it does |
| --- | --- |
| Deck Builder | Pool browser (filters by colour, type, role, mana value), deck as card stacks or a list, card inspector, auto-build, basic-land controls, suggestions |
| Commanders | Every legendary creature you own, ranked by how strong a deck your pool gives it; one-click build, Prismatic Piper option |
| Draft Helper | Type a pack's cards and get them ranked for your deck (fit, VP value, raw strength); "I took it" adds to your pool |
| Trades & VP | Trade-in plan for the 10 VP minimum (with the rarity floor), VP prices, Skill Tree costs, Shop |
| Stats & Playtest | Mana curve vs. targets, colour sources, roles, weakest cards, 20,000-hand simulation, sample hands |
| Season & Rules | The Season 2 schedule; picking a session sets deck size (60/75) and singleton |
| My Pool | Add/remove cards you own; copy your pool for Mana Mixer |

Card images (full art, high resolution) come from [Scryfall](https://scryfall.com). Each one is downloaded the first
time it's needed and saved in the `images` folder next to the program (`Card Name (MSH 123).jpg`); after that it is
always read from disk. Grids use smooth reduced-size copies, the full-resolution image loads when you rest the mouse on
a card (hover zoom) or inspect it, and only a few images are sent to the graphics card per frame so scrolling stays
smooth. If an image can't be fetched, the app draws a stand-in card face.

## Running from source

```bash
cargo run --release                 # the windowed app
cargo run --release --bin msh-cli   # the same tools as a typed command line
```

Command-line examples: `commanders`, `auto madame hydra`, `season 6`, `draft red hulk; web up; truck toss`,
`trade 10 rare`, `export mixer`, `help`.

## Files

| File | What it is |
| --- | --- |
| `data/cards.json` | Card database (cost, type, rules text, P/T, rarity, colour identity). |
| `data/collection.txt` | The cards you own, one `count name` per line. Edit by hand or with `own` / `unown`. |
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
[Forge](https://github.com/Card-Forge/forge) card database, with collector numbers and rarities from its MSH set list.
After adding new cards to `data/cards.json`, re-check them with:

```bash
python3 tools/verify_with_forge.py           # show differences
python3 tools/verify_with_forge.py --write   # apply them
```

A `?` after a card name in the app means its text hasn't been verified (for example, a card typed in by hand with
`newcard`).

## How the advice works

* **Scores** (`suggest`, `cut`, `auto`) add up rarity, roles found in the rules text (removal, card draw, counters,
  tokens, evasion...), creature stats for the mana cost, how well the card fills the mana curve, and synergy with your
  commander (for example, Villain cards for Madame Hydra). They are a heuristic to sort options, not a rule.
* **Lands**: the league's 25-27 at 60 cards (scaled for 75), low end for cheap curves; basics are split by coloured pips.
* **Simulation** shuffles the deck thousands of times to estimate opening-hand land counts, land drops by turn, and the
  odds of casting your commander on curve.

## Development

```bash
cargo test
```
