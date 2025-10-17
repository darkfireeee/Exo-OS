#!/usr/bin/env python3
# create_exo_tree_fixed.py
# Usage:
#   python3 create_exo_tree_fixed.py "arborescence d’Exo-OS .txt"
#   python3 create_exo_tree_fixed.py "arborescence d’Exo-OS .txt" --dry

import os, sys, re, codecs, argparse

parser = argparse.ArgumentParser(description="Créer arborescence depuis un ascii-tree (robuste)")
parser.add_argument("infile", help="Fichier ASCII-tree")
parser.add_argument("--dry", action="store_true", help="Simuler (ne crée rien)")
parser.add_argument("--maxlen", type=int, default=120, help="Longueur max d'un nom valide (défaut 120)")
args = parser.parse_args()

MAX_NAME_LEN = args.maxlen
INFILE = args.infile
DRY = args.dry

INVALID_CHARS_RE = re.compile(r'[<>:"\\|?*\x00-\x1f]')  # caractères interdits sur Windows (et bas niveau)

def clean_name(s):
    s = re.sub(r'\s+#.*$', '', s)   # supprime commentaires inline
    s = s.strip()
    # supprime guillemets d'encadrement éventuels
    if (s.startswith('"') and s.endswith('"')) or (s.startswith("'") and s.endswith("'")):
        s = s[1:-1].strip()
    return s

def extract_name_and_prefix(line):
    m = re.search(r'──\s*(.+)$', line)
    if m:
        name = m.group(1)
        prefix = line[:m.start()]
    else:
        prefix_match = re.match(r'^([ \t│└├─]*)', line)
        prefix = prefix_match.group(1) if prefix_match else ''
        name = line[len(prefix):]
    return clean_name(name), prefix

def depth_from_prefix(prefix):
    # approximation simple : nombre de blocs d'indentation (4 espaces) ou barres verticales
    d = prefix.count('│') + prefix.count('\t') + prefix.count('    ')
    return d

skipped = []
created_dirs = set()
created_files = set()

with codecs.open(INFILE, 'r', 'utf-8', errors='ignore') as f:
    stack = []
    for raw in f:
        line = raw.rstrip('\n\r')
        if not line.strip():
            continue
        if re.match(r'^[#=-]{2,}', line.strip()):
            continue
        name, prefix = extract_name_and_prefix(line)
        if not name:
            continue

        # heuristique : ignorer les lignes trop longues (probablement des descriptions)
        if len(name) > MAX_NAME_LEN:
            skipped.append((name, "trop long"))
            continue

        # si la "ligne" ressemble plus à une phrase (beaucoup de mots) => ignorer
        words = name.split()
        if len(words) > 12 and any(len(w) > 20 for w in words[:5]):
            skipped.append((name, "probablement phrase descriptive"))
            continue

        depth = depth_from_prefix(prefix)
        if depth < len(stack):
            stack = stack[:depth]
        stack.append(name)

        # construire chemin - on nettoie les éléments
        parts = [p for p in stack if p]
        # si un élément contient '/', l'interpréter comme suffixe dossier si c'est final
        # mais on ne veut pas laisser '/' dans un nom réel (sanitisation)
        sanitized_parts = []
        for p in parts:
            is_dir_marker = p.endswith('/')
            pn = p.rstrip('/')
            # remplacer caractères interdits
            pn = INVALID_CHARS_RE.sub('_', pn)
            pn = pn.strip()
            if not pn:
                continue
            sanitized_parts.append(pn + ('/' if is_dir_marker else ''))

        # retransformer en chemin filesystem (supprime le '/' final pour os.path.join)
        # si dernier élément a '/' c'est un dossier
        last_is_dir = sanitized_parts and sanitized_parts[-1].endswith('/')
        sanitized_parts[-1] = sanitized_parts[-1].rstrip('/')

        path = os.path.join(*sanitized_parts) if sanitized_parts else name

        if last_is_dir or name.endswith('/'):
            # dossier
            if DRY:
                print("[DRY] mkdir -p", path)
            else:
                os.makedirs(path, exist_ok=True)
            created_dirs.add(path)
        else:
            parent = os.path.dirname(path)
            if parent:
                if DRY:
                    print("[DRY] mkdir -p", parent)
                else:
                    os.makedirs(parent, exist_ok=True)
                created_dirs.add(parent)
            if DRY:
                print("[DRY] touch", path)
            else:
                try:
                    open(path, 'a', encoding='utf-8').close()
                except Exception as e:
                    skipped.append((path, f"erreur création: {e}"))
                    continue
            created_files.add(path)

# résumé
print("\n=== Résumé ===")
print("Dossiers créés :", len(created_dirs))
print("Fichiers créés :", len(created_files))
if skipped:
    print("\nLignes ignorées ou problèmes (extrait) :")
    for s, reason in skipped[:15]:
        print(" -", reason, ":", (s[:120] + '...') if len(s) > 120 else s)
    if len(skipped) > 15:
        print("  ...", len(skipped)-15, "autres ignorés")
