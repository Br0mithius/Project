# MSH Deckbuilder

An interactive terminal deck builder, written in Rust, for building a **60-card, non-singleton commander deck** out of a
**Marvel Super Heroes (MSH)** booster pool.

## Quick start

```bash
cargo run --release
```

Then try:

```
commanders            # rank every legendary creature you own as a commander
show madame hydra     # read a card (names can be partial; TAB completes)
auto madame hydra     # build a full legal 60-card deck instantly
auto thanos rb        # 3+ colour commanders: choose which colours to actually play
stats                 # curve, colours, roles, land count advice
cut                   # weakest cards in the deck
suggest               # best cards you could still add
sim                   # opening-hand / land-drop odds
check                 # validate against the format rules and your collection
save hydra            # writes decks/hydra.txt
export                # paste-able list for Moxfield / Archidekt / Arena
```

Type `help` for every command, or `help pool` for the filter syntax
(`pool c:r t:creature mv<=3 role:removal fits avail sort:score`).

You can also run commands without the interactive prompt:

```bash
cargo run -- -c "auto red hulk" -c stats -c check
printf 'auto thanos\nsim\n' | cargo run
```

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
| `commander_counts_toward_size` | false (60 cards + the commander) |
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
* **Lands**: about 40% of the library, adjusted by average mana value; basics are split by coloured pips.
* **Simulation** shuffles the deck thousands of times to estimate opening-hand land counts, land drops by turn, and the
  odds of casting your commander on curve.

## Development

```bash
cargo test
```
