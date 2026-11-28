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

## 2026-11-27: value-nominated swaps

The pool's precise values (12,000 games per opponent set) nominated four swaps the
optimiser's own search had not found. On the tested Session 5 list, 30,000 games, then
40,000 fresh ones:

- Hero's Downfall -> Attercop +0.95 ± 0.15 (table +0.63, field +1.09)
- Kang -> Panther Pounce +0.40 ± 0.15 (table +0.36, field +0.42)
- Undercover Skrull -> Kree Sentinel +0.51 ± 0.16 (table −0.21, field +0.83)
- Massacre Wurm -> Beorn's Hospitality +0.05 ± 0.15 (table −0.61, field +0.33)
- the first two together, fresh games: **+1.33 ± 0.17** (table +1.18, field +1.40)
- all four, fresh games: +2.44 ± 0.20 (table +0.84, field +3.12)

The list takes the first two: they win against both opponent sets, and against the real
table they beat all four. Lesson for the refine stage: nominate swaps from precise values
and spend the games on those, rather than screening everything on a few hundred games.

Builder gate after the peer field was regenerated: the draft is −1.17 ± 0.29 (Session 5)
and −2.12 ± 0.28 (Session 6) behind the tested lists; the refine stage brings Session 5
to −0.49 but takes 17 minutes. Two attempted draft fixes (trying each bench card in the
upgrade pass; treating precisely measured jobless cards as non-filler) did not close the
gap and were not shipped.

## 2026-11-28: do the bots over-respect blockers?

The attack AI drops attackers that would die to a predicted block and re-plans; a blocker
that kills safely moves to the next attacker, so one good blocker can stop a whole wide
board. Letting groups attack and accept their losses (`exp::WIDE_SWING`) **lost** for the
seat doing it: Drake's tokens −0.16 ± 0.07, Javi's elves −0.13 ± 0.07, Thanos −0.21 ± 0.08
(20,000 paired games). In a four-player game the caution wins. Left off.

With every seat swinging wide (`MSH_AI_EXP=16`, opponents who attack through blockers):
Hero's Downfall -> Attercop is still +0.80 ± 0.15 (table +0.45; with cautious bots +0.95,
table +0.63); both list swaps +1.07 ± 0.19 (table +0.46; cautious +1.33, table +1.18).
Defensive creatures are worth less against players who ignore blockers, but the swaps hold.

Deathtouch stacking: Attercop and Vampire Nighthawk are each worth the same with or
without the other (interaction +0.01 ± 0.21); a fourth deathtoucher (Ravening Warg) is
worth +0.55 to +0.80 because it is a weaker card, not because of overlap.

## 2026-11-28: Session 6 (singleton) list, value-nominated swaps

30,000 games each, then 40,000 fresh for the combinations:
Kang -> Panther Pounce +0.96 ± 0.15 (table +0.47); Super Villain Lockup -> Beorn's
Hospitality +0.91 ± 0.14 (table +0.89); Hero's Downfall -> Attercop +0.83 ± 0.15 (table
+0.69); Undercover Skrull -> Kree Sentinel +0.79 ± 0.15 (table −0.09, field +1.17);
Iron Man -> Mortify −0.54 ± 0.14 (rejected: a higher single-card value did not survive
the playtest). Three swaps together +2.62 ± 0.18 (table +2.29); all four **+3.44 ± 0.20**
(table +2.11, field +4.01). The list takes all four: the fourth is flat at the table
within noise and worth +1.25 against the field. (Rule: a swap is taken unless it is
clearly worse at the real table - more than one standard error.)

## 2026-11-28: Alien Invasion - a card to grow into

- Manual play let "attacks each combat if able" creatures stay home (fixed): the Aliens
  piled up as blockers, which made the card look far stronger at the table than it is.
- Head to head in Rapid Rescue's slot (Session 5 list, 20,000 paired games): -2.36 ± 0.19;
  with every seat casting engines early (EARLY_ENGINE) -2.30 ± 0.19.
- 3,000 logged games against Drake, JD and Will (deck wins 67%): cast by round 4, 28 of 31
  won (90%); rounds 5-6 64%; 7-9 69%; never drawn 65%. In hand by round 4 it was castable
  by round 5 in a third of games: {1}{R}{R}{G} against about ten red sources is the limit,
  not the bot's timing. Small sample, and a hand with that mana is a good hand anyway.
- Decision (with Brom): not in the current list; re-test with the red sources when the
  pool gains fixing (Session 5 packs, Dec 3). Refine at 20,000 games per swap did not pass
  the gate either (S5 -0.98, S6 -0.78, 11-14 minutes): stays at 6,000.
- EARLY_ENGINE on our seat, current Session 5 list: +0.20 ± 0.08 (20,000 games); to confirm.
- EARLY_ENGINE confirmation (30,000 games): S5 +0.15 ± 0.07, S6 -0.07 ± 0.06, Drake -0.05,
  Will +0.09, JD +0.02. Not general: stays off.
- Brom's own list against the tested Session 5 list (30,000 paired games): -4.59 ± 0.25
  (table -6.38). Parts: Alien Invasion + two Mountains for Rapid Rescue, Plains, Swamp
  -2.45; Mortify for the third Visions -0.78; Iron Man for Panther Pounce -0.43; HULK
  SMASH! for Attercop -0.36; She-Hulk for Doc Samson -0.18 (table -1.17); the two
  Mountains alone -0.05.

## 2026-11-28: bot self-review by brute force (first run)

`examples/bot_regret`: 300 bot-against-bot games (Session 5 Thanos list against the eight
opponent decks, seats rotated), 8 sampled decisions a game, every option tried with the bot
playing on; screen 24 play-outs, points with a 15-point gap confirmed on 160 others
(`research/data/bot_regret_thanos_300.tsv`). 92 of about 2,400 sampled decisions had an
option clearly better than the bot's (gap >= 10 points and 2 standard errors):
- attacks: sending everything at one player beat the bot's attack 20 times (mean gap 30),
  not attacking 8 times (30);
- main phase: passing first (playing after combat) 20 times (26); another land first 15
  times (24-41);
- targets and yes/no choices 26 times.
One 90-point case checked by hand (game 257, round 8): reproduced, 2 of 60 against 56 of
60. The bot paid for a commander that got countered with its red sources and could not
cast Red Hulk; with the other land it did, kept a blocker home, and survived the next
attack. Positions like this are knife-edge, so single gaps are large; the patterns need
more games and other decks before any rule changes.
- CORRECTION to the first run: the libraries were reshuffled after the decision, so a line
  that drew cards that turn drew the recorded game's cards in every play-out and the
  others random ones; listing only the cases where an alternative beat the bot turned that
  noise into findings ("pass first" above all). The reshuffle now comes right before the
  decision, and `bot_regret` writes every option of every sampled point (`GAP=0`: no
  selection), so a kind of choice is rated by its average over all positions. The table
  above is not to be used; rerun pending.

## 2026-11-28: bot self-review, corrected method, 40 life (first unbiased result)

534 bot-against-bot games (Thanos Session 5 list, Drake's and Will's decks as the reviewed
seat, the other decks as opponents), 5,299 decision points, 18,796 options, 30 play-outs
each, libraries reshuffled before the decision, every option kept
(`research/data/bot_review_40life_*.tsv.gz`). Average gain of each kind of alternative over
the bot's own choice, in win-rate points:

  attack: none                         +1.2 ± 0.3   (751 decisions)
  response: pass                       +0.2 ± 0.4
  main: play this land first           -0.1 ± 0.2
  attack: everything at one target     -0.3 ± 0.2
  block: none                          -0.8 ± 0.4
  main: pass (cast after combat)       -0.8 ± 0.2   (the first run's "pass first" was the artifact)
  other targets / yes-no / none        -1.0 to -1.2

So the bot's choices beat every blanket alternative except one: NOT ATTACKING. Where the
bot attacked (675 decisions) staying home was +1.3 ± 0.3:
  by deck:   Thanos +2.3 ± 0.4, Will +2.1 ± 0.7, Drake -2.2 ± 0.7 (the token deck should attack)
  by round:  1-4 0.0 ± 1.1, 5-7 +2.0 ± 0.6, 8-10 +3.4 ± 0.6, 11+ -1.5 ± 0.5
  by size:   1 attacker +1.7 ± 0.5, 2-3 +1.7 ± 0.5, 4+ -0.8 ± 0.6
Reading: slower decks lose games by making small attacks in the middle rounds; going wide,
and attacking late, is right. To do: read logs for why (tapped blockers and the crack-back,
or drawing the table's attacks), then a rule tested with `policy_ab` on several decks.
These games were on the rules before the audit fixes; rerun after them.

## 2026-11-28: first bot rule from the self-review - no small attacks in the middle rounds

Rule: in rounds 5-10 an attack with one to three tapping attackers that can't kill a player
is not made (vigilance and must-attack creatures still go). `policy_ab`, only that seat
changed, 12,000 paired games each (`research/data/hold_mid_policy_ab.txt`):
Thanos list +1.98 ± 0.35 (table +2.64), Will +1.50 ± 0.25, Drake +1.32 ± 0.16, JD
+1.27 ± 0.29, Andrew +1.04 ± 0.14. Tuning on 8,000 games (`hold_mid_tuning.txt`): rounds
4-10 and up to five attackers the same; rounds 5-12, 3-14 and up to two attackers worse;
every round (1-99) -2.4 for the Thanos list. On by default; `ai::exp::NO_HOLD_MID` (64)
turns it off for a seat. Next: with every seat holding, does a seat that stops holding
lose? Then the self-review again on the new bots.

Card audit: 424 in-play cards reviewed against printed text and Scryfall rulings by six
reviewer agents, 76 findings; two fixer agents fixed 67 (suite 274 -> 343 tests), 7 not
fixed (Shuri, Absorbing Man, Fishing Pole, Theorist's Proxy, Ronin, Captain America's
player hexproof, Wiccan's bot choice), 2 not engine bugs (Surveillance Room already
worked; Affectionate Indrik is a bot choice).
- Stability of the hold rule. With every seat holding, a seat that stopped holding GAINED:
  Thanos +1.81 ± 0.35, Will +1.00 ± 0.25 (Drake -0.35, JD -0.15): holding only pays when
  the table punishes tapped creatures. Refinement: a small attack the defender cannot
  block at all still goes. With every seat on the refined rule, stopping no longer gains
  (10,000 paired games): Thanos -0.38 ± 0.34, Will -0.09 ± 0.24, Drake -0.61 ± 0.15, JD
  +0.11 ± 0.27. The refined rule is the default. Lesson for every bot rule from here: test
  it one seat at a time AND with every seat using it.

## 2026-11-28: self-review rerun on the new bots (hold rule on, audited rules, 40 life)

360 games (140 Thanos list, 110 Drake, 110 JD as the reviewed seat), every option at every
decision, 30 play-outs each, no selection (`research/data/bot_review_r3_*.tsv.gz`). Average
gain of each class of alternative over the bot's own choice, win-rate points:
no attack -0.53 ± 0.42 (rounds 5-7 +0.73 ± 0.43, 8-10 +0.71 ± 0.91, 11+ -2.58 ± 0.90; it
was +1.2 ± 0.3 before the rule), a different attack -1.73 ± 0.34, no blocks -2.76 ± 1.04,
pass in main phase -0.96 ± 0.20, a different land/spell first -0.03 ± 0.15, "none" on an
optional choice -1.31 ± 0.53, a different pick on a choice -0.88 ± 0.14, pass instead of
responding +0.06 ± 0.06. No class of alternative beats the bot any more; the hold rule
used up the one pattern this coarse split could see. The next patterns need finer
features (which attack, which target) or far more games per class.
