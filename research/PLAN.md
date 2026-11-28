# Plan (2026-11-28): everything on win rate

Rule for every step: a change is kept only if paired games (same opponents, seats and
shuffles on both sides) show it wins more, on games it was not selected on. Opponents: the
real table 30%, peer field 70%. Starting life 40.

Order matters: each step changes the numbers the next one uses.

1. Rules first. Card audit against printed text and Scryfall rulings: 424 in-play cards
   done (76 findings), fixes in progress; the other 842 cards after. Gate: test suite and
   the probe snapshot.
2. Bot play, by brute force. Self-review at 40 life with the fixed rules: every option at
   sampled decisions, bot plays on, libraries reshuffled before the decision, every option
   recorded (no selection). Rank kinds of choice by average gain over the bot's own; read
   logs of the largest to find why; one rule change at a time, kept only if `policy_ab`
   gains across our deck and at least three opponents' decks. Time-boxed: what is proven
   goes in, the rest waits.
3. Card power = effect on win rate. Re-measure: (a) every card in our pool precisely
   (12,000+ games, in the tested list's weakest slots); (b) the whole database coarsely
   (800 games). The builder ranks by these; hand-written scores stay only as a first guess
   for unmeasured cards.
4. Synergy and packages. For pairs: win rate with both, minus the two singles (the
   interaction). Candidates from mechanics (payoff + enabler) and from the top cards of the
   pool. Positive pairs are grown into packages of three or more and confirmed on fresh
   games. The builder takes a package as a unit only when its interaction is measured.
5. Deck types. With the new values: which plan wins for each commander at 40 life
   (lock-out control, tokens, ramp, equipment, aggro), the job quotas of each, re-tuned by
   win rate (`archetype_lab`, `quota_lab`), then the peer field rebuilt from them.
6. Lists. Swap pass on the Session 5 and Session 6 lists from the new values; Brom's list
   and Alien Invasion re-tested; gates (`builder_gate`, `builder_bench`) rerun.

Then round again from 2: better bots change values, values change decks.
