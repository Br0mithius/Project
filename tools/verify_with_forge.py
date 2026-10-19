"""Re-verify every card in data/cards.json against Forge's card scripts (exact oracle text).

Usage:  python3 tools/verify_with_forge.py           # show differences only
        python3 tools/verify_with_forge.py --write   # also update data/cards.json

Forge (github.com/Card-Forge/forge) is an open-source MTG engine whose card scripts carry
the official oracle text. Cards you add with `newcard` can be checked the same way.
"""
import json, re, sys, urllib.request, urllib.parse, os
HERE = os.path.dirname(os.path.abspath(__file__))
SP = os.path.join(HERE, ".forge-cache") + os.sep
os.makedirs(SP, exist_ok=True)
BASE = "https://raw.githubusercontent.com/Card-Forge/forge/master/forge-gui/res/"
DB = os.path.join(HERE, "..", "data", "cards.json")

def get(path):
    cache = SP + re.sub(r"[^A-Za-z0-9_.]", "_", path)
    if os.path.exists(cache):
        return open(cache, encoding="utf-8").read()
    try:
        with urllib.request.urlopen(BASE + urllib.parse.quote(path), timeout=30) as r:
            t = r.read().decode("utf-8")
    except Exception:
        return None
    open(cache, "w", encoding="utf-8").write(t)
    return t

# Edition file: collector number + rarity.
ed = get("editions/Marvel Super Heroes.txt")
numbers = {}
section = None
for line in ed.splitlines():
    if line.startswith("["):
        section = line.strip("[]").lower(); continue
    m = re.match(r"^(\d+[a-z]?)\s+([CURMSLT])\s+(.+?)(\s+@.*)?$", line.strip())
    if m and section == "cards":
        num, rar, name = m.group(1), m.group(2), m.group(3).strip()
        if name not in numbers:
            numbers[name] = (num, {"C":"common","U":"uncommon","R":"rare","M":"mythic","L":"common","S":"special","T":"common"}[rar])

def candidates(name):
    base = name.lower()
    a = re.sub(r"[^a-z0-9 _-]", "", base)
    outs = []
    for s in (a, re.sub(r"['.!,]", "", base), re.sub(r"[']", "", base)):
        f = re.sub(r"[^a-z0-9]+", "_", s).strip("_")
        outs.append(f)
    return list(dict.fromkeys(outs))

SUPER = {"Legendary", "Basic", "Snow", "World"}
TYPES = {"Creature","Artifact","Enchantment","Instant","Sorcery","Land","Planeswalker","Battle","Kindred","Tribal"}

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
        if "/" in tok or (len(tok) == 2 and tok.isalpha() and tok.isupper() and tok not in ("XX",)):
            tok = tok if "/" in tok else tok[0] + "/" + tok[1]
        out += "{" + tok + "}"
    return out

def cmc_of(c):
    total = 0
    for s in re.findall(r"\{([^}]+)\}", c):
        if s.isdigit(): total += int(s)
        elif s in ("X", "Y"): pass
        else: total += 1
    return total

def identity(c, text):
    no_reminder = re.sub(r"\([^)]*\)", "", text)
    syms = re.findall(r"\{([^}]+)\}", c + no_reminder)
    out = []
    for col in "WUBRG":
        if any(col in s.split("/") for s in syms):
            out.append(col)
    return out

db = json.load(open(DB))
report = []
for card in db:
    script = None
    for f in candidates(card["name"]):
        script = get(f"cardsfolder/{f[0]}/{f}.txt")
        if script: break
    if not script:
        report.append(f"!! NOT FOUND in Forge: {card['name']}")
        continue
    # Only the front face (Forge scripts can hold several faces after ALTERNATE).
    script = script.split("\nALTERNATE")[0]
    fields = {}
    for line in script.splitlines():
        k, _, v = line.partition(":")
        if k in ("Name","ManaCost","Types","PT","Oracle","Loyalty") and k not in fields:
            fields[k] = v
    new = dict(card)
    new["name"] = fields.get("Name", card["name"])
    new["mana_cost"] = cost(fields.get("ManaCost", ""))
    new["cmc"] = cmc_of(new["mana_cost"])
    new["type_line"] = type_line(fields.get("Types", ""))
    new["oracle_text"] = fields.get("Oracle", "").replace("\\n", "\n").strip()
    # Forge typo on the MSH dual lands.
    new["oracle_text"] = new["oracle_text"].replace("if your control a basic land", "if you control a basic land")
    pt = fields.get("PT")
    new["power"], new["toughness"] = (pt.split("/", 1) if pt else (None, None))
    new["color_identity"] = identity(new["mana_cost"], new["oracle_text"])
    if new["name"] in numbers:
        new["collector_number"], new["rarity"] = numbers[new["name"]]
    else:
        report.append(f"   (no edition entry for {new['name']}, kept number/rarity)")
    diffs = [k for k in ("name","mana_cost","cmc","type_line","oracle_text","power","toughness","rarity","color_identity","collector_number")
             if str(card.get(k)) != str(new.get(k)) and not (k == "cmc" and float(card.get(k) or 0) == float(new.get(k)))]
    if diffs:
        report.append(f"-- {card['name']} [{card['confidence']}] changed: {', '.join(diffs)}")
        for k in diffs:
            if k == "oracle_text":
                report.append(f"     old: {card[k]!r}\n     new: {new[k]!r}")
            else:
                report.append(f"     {k}: {card.get(k)!r} -> {new.get(k)!r}")
    new["confidence"] = "verified"
    new["note"] = "Rules text, cost, type and P/T from the Forge card database (github.com/Card-Forge/forge); number and rarity from its MSH edition list."
    card.clear(); card.update(new)
if "--write" in sys.argv:
    db.sort(key=lambda c: c["name"])
    with open(DB, "w") as f:
        json.dump(db, f, indent=2, ensure_ascii=False); f.write("\n")
print("\n".join(report))
print(f"\n{len(db)} cards checked, {sum(1 for r in report if r.startswith('--'))} with differences, {sum(1 for r in report if r.startswith('!!'))} not found")
