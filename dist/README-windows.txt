MSH DECKBUILDER (Windows)
=========================

1. Unzip this folder anywhere (Desktop, Documents...). Keep the files together.
2. Double-click "MSH Deckbuilder.exe".
   - If Windows shows "Windows protected your PC", click "More info" then "Run anyway"
     (the program isn't code-signed; that's the only reason for the warning).
3. Card images: the first time a card is needed, its official image (full art,
   high resolution) is downloaded from Scryfall and saved in the "images" folder,
   e.g. images\Red Hulk (MSH 149).jpg. After that it is always read from that folder,
   never downloaded again, so it works offline. Images for your pool start downloading
   as soon as the app opens; before a draft, use Settings > "Download images for the
   whole set".
4. Your pool starts empty. Drag your Mana Mixer export file onto the app window (or
   paste it into the My Pool tab's import box with Ctrl+V), then press "Add to my pool"
   (or "Replace my pool").
5. Rest the mouse on any card to see a big, readable copy of it (Settings lets you
   change the zoom size or turn it off).

TABS
  Deck Builder     Your pool on the left (click = inspect, double-click = add,
                   right-click = more), the deck in the middle, card details on the right.
                   Auto-build, basic land +/- buttons, suggestions along the bottom.
  Commanders       Every legendary creature you own, ranked 0-100 by the deck your pool
                   gives it, including simulated 4-player games. Also shows where each
                   one lands once singleton starts. Sliders set how much each part
                   counts. "Build" makes the deck.
                   Mono-colour commanders can add a colour with The Prismatic Piper.
  Draft Helper     Type the cards in a draft pack; it ranks them for your deck.
                   "I took it" adds the card to your pool.
  Trades & VP      Which spare cards to trade in for the 10 VP minimum, VP prices,
                   Skill Tree costs and the Shop.
  Upgrades         Cards that would improve your deck: Public Pool claims (paste the
                   pool from Mana Mixer) and cards from upcoming packs, with the odds
                   that you, your draft table, or anyone in the league opens them.
  Stats & Playtest Mana curve, colours, weakest cards, opening-hand simulation,
                   sample hands.
  Season & Rules   The Season 2 schedule; "Build for this" switches deck size and
                   singleton for that session. "League starter packs" records who
                   opens Marvel or Hobbit packs (used for pack odds and playtests).
  My Pool          Import from Mana Mixer; add or remove cards; copy your pool for Mana Mixer.

The session picker at the top sets the rules (60 cards, singleton from Session 6,
75 cards from Session 10). Export copies a deck list for Moxfield/Archidekt or Mana Mixer.

ONLINE DATA (downloaded once, then only when you ask)
  Commander win rates from Playgroup.gg, EDHREC ranks and
  commander pages (what real players run with each of your commanders) and Scryfall
  Tagger tags (what each card is good at) are downloaded the first time the app starts
  and saved in the data folder. They are only downloaded again when you press
  Settings > "Update ratings now". New league cards (for example
  Nauctis: The Sunken Realms when it comes out) are added with Settings > "Update card
  database". Sets covered: Marvel Super Heroes, The Hobbit, Reality Fracture,
  Foundations, plus the league staples and Shop cards.

YOUR FILES (keep these when you update the program)
  data\collection.txt   your league pool ("count name" per line).
  data\cards.json       card database (every league set).
  data\commander_stats.json, data\edhrec.json,
  data\scryfall_tags.json      saved ranking data (data\edhrec_log.txt shows how the
                               last EDHREC download went).
  images\               card images (downloaded once, reused forever).
  decks\                saved decks (plain text).

"msh-cli (command line).exe" is the same tool as a typed command line, if you prefer it.
