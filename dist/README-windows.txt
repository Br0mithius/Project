MSH DECKBUILDER (Windows)
=========================

1. Unzip this folder anywhere (Desktop, Documents...). Keep the files together.
2. Double-click msh-deckbuilder.exe. A console window opens.
   - If Windows shows "Windows protected your PC", click "More info" then "Run anyway"
     (the program isn't code-signed; that's the only reason for the warning).
3. Type commands and press Enter. Good first steps:

     commanders           rank every legendary creature you own as a commander
     auto madame hydra    build a full legal 60-card deck instantly
     stats                mana curve, colours, roles, land advice
     suggest / cut        best cards to add / weakest cards in the deck
     sim                  opening-hand and land-drop odds
     check                make sure the deck is legal
     save hydra           save the deck (goes in the decks folder)
     export               decklist you can paste into Moxfield / Archidekt
     help                 every command
     quit                 save and exit

   Press TAB to auto-complete card names. Names can be partial ("add 2 roxxon").

YOUR FILES
  data\collection.txt   the cards in your league pool ("count name" per line).
                        Edit it in Notepad, or use the `own` / `unown` commands.
  data\cards.json       card database (rules text etc.).
  decks\                your saved decks (plain text).

When you get new cards, either add them with `own <card>` or replace data\cards.json
and data\collection.txt with updated copies.
