# Research log

Results that decisions rest on, with how each was measured. Win-rate points; ± is one
standard error; "paired" = every variant plays the same opponents, seats and shuffles.
The referee (`optimize::Opponents::standard`): the real next table 30%, the peer field 70%.

## Method rules (learned the hard way)

- One referee for everything. Card values used to be measured against the peer field only
  while decks were judged on table 30% / field 70%; the builder ranked on one objective and
  was graded on another.
- Screen on one set of games, confirm on fresh ones. Never ship on the screen.
- A builder change must pass **both** `builder_gate` (auto-build within 0.5 points of the
  playtest-tested deck from the real pool) and `builder_bench` (no worse over 22 commanders
  x 2 pools). "Playtests only, no noise discount" won the benchmark by +3.4 and lost 4.2
  on the real pool: it swapped 40,000-game values for 800-game flukes.
- A value is only as good as its game count. 800 games is ±1.1 points; cards half a point
  apart need 8,000+.
- Tools must load data the way the app does (`db.attach_ratings`, `league::app_rules`).
  Seven tools built decks without measured values and "found" a weak five-colour builder.
- A struct field added without a serde default silently made every saved rules.json fall
  back to default rules. Loading now warns, and a test loads an old file.

## Engine bugs found

- Alien Invasion fired at every player's combat ("attacks each combat if able" matched the
  whole text). Measured +4 to +5 with the bug, −0.4 without. Fixed 2026-11-27; trigger
  phase and whose-turn are read from the trigger clause only.
- Sweep of 316 cards (deck, pool, all opponents) on 2026-11-27: no further bug; commanders
  Ramos, Dwalin and Smaug verified in game logs.

## Findings

- **Removal timing.** Holding instant removal for attackers or end of turn: −0.24 ± 0.14
  (−0.68 vs the table). Killing a creature in response to an Aura/equip/trick: 0.00 ± 0.01;
  in 421 windows over 200 games the seat never had removal and mana together. Both off.
- **Synergy.** Of 29 archetypes' best decks, on-theme packages are worth the sum of their
  cards (interaction within noise) except Plans (+1.4 ± 0.6). Tribal packages: none.
  The Thanos deck's packages: no interaction; its six fliers are worth +21.5 points.
- **Archetype strength** (realistic pools, 3,000 games): stax-lite control 29%, teamwork
  25%, lifegain and reactive control 21%; tribal and engine decks 6-9%. The commander
  matters as much as the archetype (13% to 51% within stax-lite).
- **Copies.** Each copy of Visions of Villainy is worth +1.1 to +1.4, each Web Up +1.4.
- **Colour packs** beat the plain auto-build only for a few commanders (Nick Fury GWU
  +9.3); the optimiser races them and switches only on a win.
- **Thanos, Session 5 list** (league_data/decks): +3.45 ± 0.22 over the earlier plan;
  Alien Invasion out for Rapid Rescue +2.22 ± 0.13; HULK SMASH! + Mountain out for Arbiter
  of Woe + Swamp +0.43 ± 0.17 (twice); Murdock's Crusade stays (cutting it −0.6 to −1.0).
- **Dependable Quinjet** measures +1.3 in the simulator: the bots crew it more freely than
  a person would. A known simulator bias; it is on the never-use list.

## Known weak points (2026-11-27 audit)

1. Nothing is calibrated against real games (2 logged). Log every game.
2. Opponent decks are reconstructions (John and Javi low confidence).
3. 1,546 of 1,649 value rows are 800-game measurements against the old peer field.
4. The gate has one golden deck; it measures consistency with the simulator, not truth.
5. The refine stage of auto-build screens on 800 games: too few to find 1-point swaps.
