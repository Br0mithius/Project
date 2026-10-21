# MSH Deckbuilder

A deck builder for the **Mulligans Island Commander League (Season 2)**: build a commander deck from your
league pool, with official card images, league rules, a draft helper and trade planning. The card database covers
every set in the Season 2 rules: **Marvel Super Heroes**, **The Hobbit**, **Reality Fracture**, **Foundations** (the
Public Pool starter kit - the booster set plus its Starter Collection / Beginner Box / precon reprints, marked
"product-only" so pack odds ignore them), the league staples and Shop cards, and **Nauctis: The Sunken Realms** once it is released
(Settings > "Update card database").

The repository ships with an **empty pool**. Import yours from Mana Mixer: drag the export file onto the app window, or paste it into the
My Pool tab's import box with Ctrl+V, then press "Add to my pool" or "Replace my pool", or with `import <file> [replace]` in the command-line app. Lines like
`1 Red Hulk (MSH) 149`, `2x Web Up`, two-faced names like `Bilbo Baggins, Burglar // Take a Glance`, and CSV files
with Name and Quantity columns are understood (exports use the same Mana Mixer format, "Front // Back" names included); "replace" keeps a
copy of the old pool in `data/collection.backup.txt`.

## Windows

Download `dist/MSH-Deckbuilder-windows.zip`, unzip it, and double-click **MSH Deckbuilder.exe**.
See `dist/README-windows.txt` for a tour. To rebuild the zip from Linux: `tools/build-windows.sh`.

## The app

| Tab | What it does |
| --- | --- |
| Deck Builder | Pool browser (filters by colour, type, role, mana value), deck as card stacks or a list, card inspector, auto-build, basic-land controls, suggestions |
| Playtest Lab | Starts by itself when you choose a commander or auto-build: playtests what each card in the deck is worth, the best additions from your pool, one land more or fewer and the other colour sets, then shows - separately from your deck - the best combination of swaps the games support, with an "Apply" button (undoable) |
| Commanders | Every legendary creature you own, ranked 0-100 by the deck your pool gives it (eight weighted parts you can adjust, including simulated 4-player games and EDHREC), with a preview of the ranking once singleton starts; one-click build, Prismatic Piper option |
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
| [Playgroup.gg](https://playgroup.gg) | Commander win rates in recorded multiplayer games | Once, on first start; then only via Settings > "Update ratings now" (one gentle request at a time, stops early if the site doesn't answer) |
| [EDHREC](https://edhrec.com) | Each card's EDHREC rank (read from Scryfall, which publishes it, 75 cards per request) and, for each commander you own, the cards EDHREC players run with it | Only via "Update ratings now" (one request per commander, a second apart; commanders without a page aren't asked again until the next update). If EDHREC's shield refuses the JSON address, the app retries once with a plain user agent, then reads the same data from the commander's edhrec.com page, and follows EDHREC's renamed-page redirects |
| [Scryfall Tagger](https://scryfall.com/docs/tagger-tags) | Community tags for what cards do (removal, ramp, sacrifice outlet...) | Only via "Update ratings now": the tag list page, then one search per useful tag, limited to the league sets |
| [Scryfall](https://scryfall.com) | Card images; new league cards | Each image once, then read from `images/`; cards only via "Update card database" |

Everything downloaded is saved in `data/` (`commander_stats.json`, `edhrec.json`,
`scryfall_tags.json`), so the app never asks the sites for the same data twice unless you press a button.

## Running from source

```bash
cargo run --release                 # the windowed app
cargo run --release --bin msh-cli   # the same tools as a typed command line
```

Command-line examples: `commanders`, `auto madame hydra`, `lab` / `lab thorough` (then `lab apply`), `season 6`, `draft red hulk; web up; truck toss`,
`trade 10 rare`, `export mixer`, `import pool.txt`, `ratings`, `update-cards`, `why red hulk`, `upgrades`,
`publicpool pool.txt`, `roster`, `help`. `cargo run --release --example rank_timing -- data` times the ranking's parts;
`cargo run --release --example weight_audit -- data` measures each ranking part's spread, noise and overlap on your pool;
`compare_builds -- data "Thanos, the Mad Titan" WBR WUBRG` playtests one commander in several colour sets (win rate,
mana sources, opening hands); `score_cards -- data "<commander>" "<card>"...` shows how cards score for that deck;
`review_cards -- data <list.txt>` prints each card's rules text beside the app's reading, for checking misreads;
`probe_cards -- data <list.txt>` fires every ability of each card on a standard three-player board in the real
simulator and lists exactly what changed (who lost what, counters, tokens, life, cards), which is the quickest way to
see whether a card plays the way its text says. Add `COMPARE_GAMES=4800` before `compare_builds` for closer calls.

## Files

| File | What it is |
| --- | --- |
| `data/cards.json` | Card database (cost, type, rules text, P/T, rarity, colour identity). |
| `data/collection.txt` | The cards you own, one `count name` per line (empty until you import). Edit by hand or with `own` / `unown`. |
| `data/commander_stats.json` | Saved Playgroup.gg data. |
| `data/edhrec.json`, `data/scryfall_tags.json` | Saved EDHREC ranks and commander pages, and Scryfall Tagger tags. `data/edhrec_log.txt` lists every EDHREC request of the last update and how it went. |
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

* **Card reading** (`src/abilities.rs`): every card's rules text is read into structured abilities following the
  Comprehensive Rules' ability types - keywords (702), static (604), triggered "when/whenever/at" with intervening
  "if" clauses (603), activated "[cost]: [effect]" (602), loyalty abilities, Saga chapters, modal "choose" spells, and
  the league sets' mechanics: Power-up (702.193 - once, cheaper the turn it enters), Teamwork (702.194), Plans,
  Storied (702.195), Recruit, Amass, Empower Jace, prepared (722) and Adventures. Each ability records when it
  happens, what it does and what it can hit, so removal is graded by what it actually answers (unconditional exile >
  conditional > damage-based > fight > lockdown > bounce; "for each opponent" removal counts extra), board wipes are
  told apart from single-target spells, once-only and sacrifice abilities aren't counted as repeatable, and tokens
  given to an opponent (Beast Within) aren't counted as yours. Every rules line in the card database (2,329 lines) is
  read; one-off rule changes ("can't be countered", replacement effects with "instead") are named rather than skipped.
  The card inspector's "How the app reads this card" (and `show <card>`) shows the reading and marks anything it
  couldn't read with "?". `cargo run --release --example parse_report -- data` reports coverage (`--card "Name"`,
  `--removal`, `--roles`, `--synergy "Commander"` for details).

* **Card strength** starts every card as average, nudged a little by rarity (mythic +0.5 over common). 17Lands draft
  win rates were used here before; they come from 2-player 40-card draft games, so they've been dropped (and any
  saved copy is deleted). Roles from the card reading (graded removal, card draw, board wipes, "each opponent"
  effects that are better with 4 players, repeatable engines...), curve fit and synergy are added on top.
* **Creatures** are rated by value for their mana cost (`src/value.rs`): body, keywords and abilities on one scale
  (1 point is about 1 power), compared with what a fair creature of that cost gives. Keywords count for more when the
  creature can use them (flying on a 4-power creature), and abilities that matter in 4-player games (removal, card
  draw, repeating engines, effects on every opponent) count for more than ones that don't. Mana abilities count as
  ramp, and returning only itself from the graveyard as resilience. The card's "why" shows e.g. "156% of fair value
  for 3 mana (2/3 body, 2 damage to any target)"; `cargo run --release --example creature_values -- data --pool`
  lists your creatures this way.
* **EDHREC** adds two things. A card's EDHREC rank, compared with the rest of its set, moves it up to ±0.4 - less
  for new sets, which haven't been in many decks yet (half weight 90 days after release). And when its commander has
  an EDHREC page, cards real players run with that commander get up to +1.5 (from inclusion rate and synergy),
  trusted more the more decks the page is built on (half at 200 decks). The card inspector shows both.
* **Scryfall Tagger tags** are a second opinion next to the card reading: a tag adds a role the reading missed
  (a "removal" tag on a card the reader sees as bounce stays bounce), and tags like "sacrifice-outlet" or "self-mill"
  count a card as an enabler for a commander that needs one.
* **Synergy** pairs what a commander rewards with what a card provides, about 25 mechanics read from both cards:
  tokens, +1/+1 counters, deaths and sacrifice, graveyard, instants/sorceries, artifacts, Equipment, Vehicles, Plans,
  landfall, life gain, drawing a second card, discard, attacking, enters-the-battlefield, Storied, planeswalkers/Jace,
  Teamwork, noncombat damage, big creatures, mana for expensive abilities, Enrage and Armies - plus the creature types
  the commander cares about. The same check runs against the rest of the deck being built: a card scores higher when
  it feeds payoffs already in the deck, when the deck's cards feed its own payoff, or when other cards in the deck
  care about its creature type.
* **Deck plans**: each commander's cost, body and rules text decide what kind of deck it wants - aggro (cheap
  attackers, combat triggers: lower curve, 18-24 creatures), midrange, big mana (expensive payoffs: higher curve, fewer
  small creatures) or spells (instant/sorcery payoffs: 9-14 creatures, more removal and draw). Auto-build, the
  Stats tab's curve targets and the ranking all use that plan.
* **Commander ranking** auto-builds a full deck for each commander (and each mono-colour commander with The Prismatic
  Piper in its two best extra colours) and grades it on eight parts - Cards, Curve, Creatures, Interaction, Mana,
  Commander (Playgroup.gg win rate, abilities, cost), Playtest and EDHREC (how much of the deck is what EDHREC
  players run with that commander; middle when there's no page or one built on under 20 decks). Parts are put on one
  scale (how far above or below your middle option, using the median so one freak option can't distort the rest), the
  sliders set how much each part counts, and the total runs 0-100 with 50 = your middle option.
  The default weights come from measuring each part on a real 64-option pool (`weight_audit`):

  | Part | Weight | Why |
  | --- | --- | --- |
  | Cards | 1.0 | Broadest, most data-backed measure |
  | Curve | 0.5 | Follows Cards closely (rank correlation 0.82) - a deep pool fills the curve too |
  | Creatures | 0.5 | Mostly a penalty for too few creatures (8 distinct values across 64 options) |
  | Interaction | 0.6 | Also follows Cards (0.81) |
  | Mana | 0.6 | Opening-hand checks; the full game simulator pays coloured mana exactly and disagrees with it (-0.62), finding three-colour decks win more than this part says |
  | Commander | 0.8 | Independent, but partly heuristics and small samples |
  | Playtest | 1.5 | The only part that plays full 4-player games against league-like decks (noise ±0.017 at 600 games, reliability 0.97) |
  | EDHREC | 0.4 | Real decks, but 100-card constructed with every card available, and thin for new sets |

  Playgroup.gg win rates count for less than before (half trust at 300 games, at most ±0.6), and EDHREC's commander
  popularity isn't used (it tracks fans of the character more than strength in a league pool). Settings still on an
  earlier version's defaults move to these automatically; weights you changed are kept. A commander's ability only counts as much as its
  deck can feed it: each mechanic it depends on (Equipment, token makers, instants and sorceries, Villains...) is
  counted in its built deck against how many it wants, and abilities that do little without them ("if he's
  equipped", "whenever a token enters") are scaled down - the "Why?" notes say e.g. "needs Equipment: 1 in this deck
  (wants ~6) - ability rarely works". The same check marks other payoff cards in a deck as weak when their enablers
  are missing, and in the playtest "if equipped" abilities only work when Equipment is attached. Before Session 6 the same ranking is also run
  under singleton rules as a preview. `why <commander>` in the command line shows the breakdown.
* **Playtest** (`src/playtest/`) plays each deck in 600 full 4-player Commander games against decks auto-built from two
  independent sets of simulated league pools (random packs, the same number you have by this point of the season), so
  one lucky or unlucky draw of the other players' packs can't swing it. Every deck faces the same opponents and
  shuffles.
  - **The game:** library, hand, graveyard, exile and command zone; colour-exact mana from each land and rock;
    commander tax and 21 commander damage; 30 life; The Prismatic Piper as a second commander; attackable
    planeswalkers; Sagas, Plans (and taking over an opponent's turn), Equipment, Auras, Vehicles; hand size 7;
    London mulligan with a free first mulligan.
  - **The cards:** every effect the card reader understands is played - 1,888 of the 2,106 effects in the database
    exactly, 206 approximated (copies, free spells, doublers, unusual counters...) and 12 with no effect in a
    simplified game (naming a card, redirecting a spell...). Every trigger fires, every keyword works (flash, ward,
    protection, toxic, cycling, crew, convoke, extort, flashback, kicker...) and the league mechanics work: Power-up,
    Teamwork, Storied, Recruit, Amass, Empower Jace, prepared, Adventures. Conditions ("if you attacked this turn",
    "as long as you have an enduring story") are checked in the game; the 44 it can't check are decided by chance.
    `cargo run --release --example sim_coverage -- data` lists how every effect, trigger, keyword and rule is played.
  - **The AI** plays each seat like an above-average player: it works out who the biggest threat is (and remembers who
    attacked it), plans its whole turn's mana, holds up counters and instant-speed answers when that's worth more than
    tapping out, counters by threat, aims removal at the most dangerous thing on the table, wipes the board when it's
    behind, attacks as a group after predicting the defenders' blocks (keeping enough back to survive the crack-back,
    going for lethal when it's there, hitting planeswalkers), blocks for value (kill-and-survive, good trades, double
    blocks, chump only against lethal, wary of open mana), and uses combat tricks, Fogs, flash blockers and protection
    in response to removal. It also equips, crews, picks loyalty abilities, cycles dead cards and chooses well for
    scry, discard, sacrifice and tutors.
  - **Checks:** an all-basic-lands deck wins under 2%; a deck against copies of itself lands near 25%; a deck with half
    its spells swapped for lands wins clearly less. One game takes about 2.5 ms; a full ranking with the singleton
    preview takes under a minute on 4 cores. `sim_bench`, `sim_log` (one game, move by move) and `sim_decks` are in
    `examples/`.
* **Playtest lab** (`src/lab.rs`, Playtest Lab tab, `lab` in the command line). Auto-build picks cards by their
  score, and a score can be wrong: a card can look strong and still lose games (and fitting it in costs the cards it
  pushes out). The lab checks the deck by playing it instead. It never changes your deck by itself; its suggestion is
  shown separately and applied only when you press "Apply best combination" (`lab apply`; Undo takes it back).
  - **Paired playtests** (`src/playtest/lab.rs`): every game has its own seed, which fixes the opponents, your seat
    and every shuffle. A changed deck keeps the library's layout and only changes which card one library slot is, so it
    plays exactly the same games as your deck until the swapped card turns up. Comparing the two game by game ("did
    the change win a game the deck lost, or lose one it won?") cancels most of the luck that separate win rates
    carry: the same accuracy needs several times fewer games. Results are the same on any number of CPU cores.
  - **What each card is worth:** each nonland card is replaced by the best card left on your bench (by score), and
    the win rate lost is the card's worth. When the best bench card is strong, swapping almost anything for it helps,
    so "dead weight" marks the cards whose swap helped clearly more (two standard errors) than the deck's typical card.
  - **Additions:** the best-scoring bench cards, each tried in place of the cards measured least valuable; and one
    land more (the weakest spell becomes a basic) or fewer (a basic becomes the best bench card).
  - **Best combination:** the swaps that measured well are combined, biggest first (each card cut or added only as
    often as you have it). The combination is then **confirmed on fresh games** - different seeds, several times as
    many - and each swap is checked by playing the combination without it; swaps that don't hold up are dropped (and
    what's left is measured again on new games). Choosing the best of many noisy measurements picks lucky ones (the
    winner's curse); measuring the winner again on games that didn't choose it gives an honest number. The
    combination is reported only if its confirmed gain is more than two standard errors; otherwise the lab says no
    change beats the current build.
  - **Colour sets:** for commanders with a choice of colours (Thanos: WBR, WUBR, WBRG, WUBRG), the deck is rebuilt in
    each and played against the same opponents and seats ("Use this build" switches to one).
  - **Reading the numbers:** win-rate points (+2.0 = wins 2% more of its games; 25% is even in a 4-player game) ±
    one standard error. Within about ±2 standard errors is noise; green and red mark changes clearly better or worse.
    Card worths and additions are screening numbers (fewer games); the best combination's numbers are confirmed ones.
  - **Effort:** *quick* (about 1 minute on 4 cores: 800 games per card, 600 per addition, 3,000 to confirm, 2,000 per
    colour set) or *thorough* (about 5 minutes: 3,000 / 2,400 / 10,000 / 6,000, more additions and cut candidates).
    The app runs a thorough lab in the background when you choose a commander (the Playtest Lab tab can switch this
    to quick or off) and stops it if you change commander.
* **Upgrades / pack odds** use Play Booster contents: 7 commons, 3 uncommons, a rare or mythic (mythic about 1 in 7),
  plus a wildcard and a foil of any rarity at typical published rates. "Your table" means the card is opened at your
  4-player draft table (so it's in your face-up draft) - you plus 3 of the other 7 players, any seating equally likely;
  "league" means any of the 8 players opens it. Who opens Marvel or Hobbit packs comes from **League starter packs**
  (Season & Rules tab, or `roster` in the command line), set by default to the Season 2 mix: you and one other player
  all Marvel, one player 3 Marvel / 6 Hobbit at kickoff, five all Hobbit. That's why Hobbit cards can show up as
  upgrades for a Marvel player: they can be drafted from a table-mate's pack. The same mix shapes the simulated
  opponents in playtests (one deck per other player, from their own packs).
* **Lands**: the league's 25-27 at 60 cards (scaled for 75), low end for cheap curves; basics are split by coloured pips.
* **Simulation** shuffles the deck thousands of times to estimate opening-hand land counts, land drops by turn, and the
  odds of casting your commander on curve.

## Development

```bash
cargo test --release
```

`data/probe_snapshot.txt` pins what the simulator does with every card in the database (the `probe_cards` output).
If a change makes any card play differently, the `probe_snapshot` test fails and shows each card's old and new
behaviour. Check every listed card against its rules text, then accept the changes with
`UPDATE_PROBE_SNAPSHOT=1 cargo test --release probe_snapshot`.

Card-reading accuracy is measured with blind audits: a random 200 league cards are probed and every result is
compared with the card's rules text. October 2026: 48.5% of cards had some misplay and 10.5% a serious one; after
three rounds of fixes, 4.5% and 1%.
