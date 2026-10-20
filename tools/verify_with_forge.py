"""Build and verify data/cards.json from Forge's card scripts (exact oracle text).

Forge (github.com/Card-Forge/forge) is an open-source MTG engine whose card scripts carry
the official oracle text; its edition files give collector numbers and rarities.

Usage:
    python3 tools/verify_with_forge.py                         # show differences only
    python3 tools/verify_with_forge.py --write                 # also update data/cards.json
    python3 tools/verify_with_forge.py --import-set HOB --write   # add a whole league set
    python3 tools/verify_with_forge.py --add "Sol Ring" --set LEAGUE --write

League sets: MSH (Marvel Super Heroes), HOB (The Hobbit), FRA (Reality Fracture),
FDN (Foundations). Nauctis isn't in Forge yet - the app's "Update card database" button
fetches it from Scryfall once it's released.
"""
import json, re, sys, urllib.request, urllib.parse, os, unicodedata

HERE = os.path.dirname(os.path.abspath(__file__))
CACHE = os.path.join(HERE, ".forge-cache") + os.sep
os.makedirs(CACHE, exist_ok=True)
FORGE = "https://raw.githubusercontent.com/Card-Forge/forge/master/forge-gui/res/"
XMAGE = "https://raw.githubusercontent.com/magefree/mage/master/Mage.Sets/src/mage/"
DB = os.path.join(HERE, "..", "data", "cards.json")

# Set code -> (Forge edition name, XMage set class). First listed wins for reprints.
EDITIONS = {
    "MSH": ("Marvel Super Heroes", "MarvelSuperHeroes"),
    "HOB": ("The Hobbit", "TheHobbit"),
    "FRA": ("Reality Fracture", "RealityFracture"),
    "FDN": ("Foundations", "Foundations"),
}
RARITY = {"C": "common", "U": "uncommon", "R": "rare", "M": "mythic", "L": "common", "S": "special", "T": "common"}
BASICS = {"Plains", "Island", "Swamp", "Mountain", "Forest", "Wastes"}
SUPER = {"Legendary", "Basic", "Snow", "World"}
TYPES = {"Creature", "Artifact", "Enchantment", "Instant", "Sorcery", "Land", "Planeswalker", "Battle", "Kindred", "Tribal"}
# Double-faced cards Forge files as "front_back" whose back names are known.
DFC_BACK = {
    "Bruce Banner": "The Incredible Hulk",
    "Tony Stark": "The Invincible Iron Man",
    "Jennifer Walters": "The Sensational She-Hulk",
    "King T'Challa": "Black Panther, Hope Enduring",
    "Monica Rambeau": "Photon, Living Light",
}


def get(url, cache_name):
    path = CACHE + re.sub(r"[^A-Za-z0-9_.]", "_", cache_name)
    if os.path.exists(path):
        text = open(path, encoding="utf-8").read()
        return text or None
    try:
        with urllib.request.urlopen(url, timeout=30) as r:
            text = r.read().decode("utf-8")
    except Exception:
        text = ""
    open(path, "w", encoding="utf-8").write(text)
    return text or None


def forge(path):
    return get(FORGE + urllib.parse.quote(path), path)


def edition_cards(code):
    """[(number, rarity, name)] from a Forge edition file's [cards] section."""
    text = forge(f"editions/{EDITIONS[code][0]}.txt") or ""
    out, section = [], None
    for line in text.splitlines():
        if line.startswith("["):
            section = line.strip("[]").lower()
            continue
        m = re.match(r"^(\d+[a-z]?)\s+([CURMSLT])\s+(.+?)(\s+@.*)?$", line.strip())
        if m and section == "cards":
            out.append((m.group(1), RARITY[m.group(2)], m.group(3).strip()))
    return out


NUMBERS = {}  # name -> {code: (number, rarity)}
for code in EDITIONS:
    for num, rar, name in edition_cards(code):
        NUMBERS.setdefault(name, {}).setdefault(code, (num, rar))


def xmage_back_name(name, code):
    """For double-faced cards, read the back face's name from XMage's implementation."""
    cls_file = EDITIONS.get(code, (None, None))[1]
    if not cls_file:
        return None
    sets = get(XMAGE + f"sets/{cls_file}.java", f"xmage_{cls_file}.java") or ""
    m = re.search(r'SetCardInfo\("' + re.escape(name) + r'",[^;]*?mage\.cards\.(\w)\.(\w+)\.class', sets)
    if not m:
        return None
    src = get(XMAGE + f"cards/{m.group(1)}/{m.group(2)}.java", f"xmage_{m.group(2)}.java") or ""
    b = re.search(r'"\{[^"]*\}",\s*"([^"]+)"', src)
    return b.group(1) if b else None


def candidates(name, code=None):
    plain = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    names = [plain]
    back = DFC_BACK.get(name) or (xmage_back_name(name, code) if code else None)
    if back:
        names.insert(0, plain + " " + unicodedata.normalize("NFKD", back).encode("ascii", "ignore").decode())
    outs = []
    for n in names:
        base = n.lower()
        for s in (re.sub(r"[^a-z0-9 _-]", "", base), re.sub(r"['.!,]", "", base), re.sub(r"[']", "", base)):
            outs.append(re.sub(r"[^a-z0-9]+", "_", s).strip("_"))
    return list(dict.fromkeys(outs))


def type_line(t):
    words = t.split()
    head = [w for w in words if w in SUPER or w in TYPES]
    sub = [w for w in words if w not in SUPER and w not in TYPES]
    return " ".join(head) + (" — " + " ".join(sub) if sub else "")


def cost(mc):
    if not mc or mc.strip() == "no cost":
        return ""
    out = ""
    for tok in mc.split():
        if "/" in tok or (len(tok) == 2 and tok.isalpha() and tok.isupper()):
            tok = tok if "/" in tok else tok[0] + "/" + tok[1]
        out += "{" + tok + "}"
    return out


def cmc_of(c):
    total = 0
    for s in re.findall(r"\{([^}]+)\}", c):
        if s.isdigit():
            total += int(s)
        elif s not in ("X", "Y"):
            total += 1
    return total


def identity(c, text):
    no_reminder = re.sub(r"\([^)]*\)", "", text)
    syms = re.findall(r"\{([^}]+)\}", c + no_reminder)
    return [col for col in "WUBRG" if any(col in s.split("/") for s in syms)]


def parse(face):
    fields = {}
    for line in face.splitlines():
        k, _, v = line.partition(":")
        if k in ("Name", "ManaCost", "Types", "PT", "Oracle", "Loyalty") and k not in fields:
            fields[k] = v
    return fields


def build(card):
    """Card dict from Forge, or None if Forge doesn't have it."""
    script = None
    for f in candidates(card["name"], card.get("set") or None):
        script = forge(f"cardsfolder/{f[0]}/{f}.txt")
        if script:
            break
    if not script:
        return None
    faces = script.split("\nALTERNATE")
    front = parse(faces[0])
    back = parse(faces[1]) if len(faces) > 1 else None
    new = dict(card)
    new["name"] = front.get("Name", card["name"])
    new["mana_cost"] = cost(front.get("ManaCost", ""))
    new["cmc"] = cmc_of(new["mana_cost"])
    new["type_line"] = type_line(front.get("Types", ""))
    text = front.get("Oracle", "").replace("\\n", "\n").strip()
    text = text.replace("if your control a basic land", "if you control a basic land")  # Forge typo
    if back:
        bcost = cost(back.get("ManaCost", ""))
        btext = back.get("Oracle", "").replace("\\n", "\n").strip()
        bpt = (" " + back["PT"]) if back.get("PT") else ""
        text += f"\n\n// Back face: {back.get('Name', '')} {bcost}\n{type_line(back.get('Types', ''))}{bpt}\n{btext}"
    new["oracle_text"] = text
    pt = front.get("PT")
    new["power"], new["toughness"] = (pt.split("/", 1) if pt else (None, None))
    new["color_identity"] = identity(new["mana_cost"] + (cost(back.get("ManaCost", "")) if back else ""), text)
    entries = NUMBERS.get(new["name"], {})
    code = card.get("set") if card.get("set") in entries else (next(iter(entries)) if entries else None)
    if code:
        new["set"] = code
        new["collector_number"], new["rarity"] = entries[code]
    new["confidence"] = "verified"
    new["note"] = "Rules text, cost, type and P/T from the Forge card database (github.com/Card-Forge/forge)" + (
        f"; number and rarity from its {EDITIONS[code][0]} edition list." if code else "."
    )
    return new


def main():
    db = json.load(open(DB, encoding="utf-8"))
    have = {c["name"] for c in db}
    report = []
    args = sys.argv[1:]
    set_for_added = args[args.index("--set") + 1] if "--set" in args else ""
    for i, a in enumerate(args):
        if a == "--import-set":
            code = args[i + 1].upper()
            for num, rar, name in edition_cards(code):
                if name not in BASICS and name not in have:
                    db.append({"name": name, "set": code, "confidence": "new"})
                    have.add(name)
        if a == "--add":
            name = args[i + 1]
            if name not in have:
                db.append({"name": name, "set": set_for_added, "confidence": "new"})
                have.add(name)
    order = ["name", "mana_cost", "cmc", "type_line", "oracle_text", "power", "toughness", "rarity",
             "color_identity", "set", "collector_number", "confidence", "note"]
    out, missing, changed, added = [], [], 0, 0
    for card in db:
        if card.get("confidence") == "partial" and "XMage" in card.get("note", ""):
            out.append(card)  # hand-made from XMage; Forge doesn't have it
            continue
        new = build(card)
        if not new:
            if card.get("confidence") == "new":
                missing.append(f"{card['name']} ({card.get('set')})")
            else:
                out.append(card)
                report.append(f"!! NOT FOUND in Forge: {card['name']}")
            continue
        if card.get("confidence") == "new":
            added += 1
        else:
            diffs = [k for k in ("mana_cost", "type_line", "oracle_text", "power", "toughness", "rarity", "color_identity", "collector_number")
                     if str(card.get(k)) != str(new.get(k))]
            if diffs:
                changed += 1
                report.append(f"-- {card['name']} changed: {', '.join(diffs)}")
        new.setdefault("rarity", card.get("rarity", "special"))
        new.setdefault("set", card.get("set", ""))
        out.append({k: new.get(k) for k in order})
    if "--write" in args:
        out.sort(key=lambda c: c["name"])
        with open(DB, "w", encoding="utf-8") as f:
            json.dump(out, f, indent=2, ensure_ascii=False)
            f.write("\n")
    print("\n".join(report))
    if missing:
        print("Not in Forge yet (the app's Scryfall update fills these):", ", ".join(missing))
    print(f"{len(out)} cards; {added} added, {changed} changed, {len(missing)} missing")


main()
