MSH DECKBUILDER (Windows)
=========================

1. Unzip this folder anywhere (Desktop, Documents...). Keep the files together.
2. Double-click "MSH Deckbuilder.exe".
   - If Windows shows "Windows protected your PC", click "More info" then "Run anyway"
     (the program isn't code-signed; that's the only reason for the warning).
3. The first time it runs it downloads the official card images (full art) for your
   pool from Scryfall in the background and saves them in data\images, so they work
   offline afterwards. Before a draft, use Settings > "Download images for the whole set".

TABS
  Deck Builder     Your pool on the left (click = inspect, double-click = add,
                   right-click = more), the deck in the middle, card details on the right.
                   Auto-build, basic land +/- buttons, suggestions along the bottom.
  Commanders       Every legendary creature you own, ranked. "Build" makes a full deck.
                   Mono-colour commanders can add a colour with The Prismatic Piper.
  Draft Helper     Type the cards in a draft pack; it ranks them for your deck.
                   "I took it" adds the card to your pool.
  Trades & VP      Which spare cards to trade in for the 10 VP minimum, VP prices,
                   Skill Tree costs and the Shop.
  Stats & Playtest Mana curve, colours, weakest cards, opening-hand simulation,
                   sample hands.
  Season & Rules   The Season 2 schedule; "Build for this" switches deck size and
                   singleton for that session.
  My Pool          Add or remove cards you own; copy your pool for Mana Mixer.

The session picker at the top sets the rules (60 cards, singleton from Session 6,
75 cards from Session 10). Export copies a deck list for Moxfield/Archidekt or Mana Mixer.

YOUR FILES (keep these when you update the program)
  data\collection.txt   your league pool ("count name" per line).
  data\cards.json       card database (the whole Marvel Super Heroes set).
  data\images\          downloaded card images.
  decks\                saved decks (plain text).

"msh-cli (command line).exe" is the same tool as a typed command line, if you prefer it.
