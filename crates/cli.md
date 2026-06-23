# Guide d'utilisation de `epc-cli`

Le crate `epc-cli` fournit le binaire `escale-epc`. C'est l'interface en ligne
de commande du SDK pour créer, valider, signer, packer et préparer les images
d'un EPC `core-format`.

Le binaire s'appuie sur les autres crates du workspace :

- `epc-core` pour les constantes et chemins du format ;
- `epc-validate` pour produire les rapports de validation ;
- `epc-pack` pour créer, signer et assembler les EPC ;
- `epc-image` pour valider, prévisualiser et encoder les images JPEG XL.

## Générer la documentation Rust

La documentation du crate se génère avec :

```bash
cargo doc -p epc-cli --no-deps
```

Le crate expose surtout un binaire, donc la documentation utile au quotidien est
principalement la documentation des commandes ci-dessous.

## Afficher l'aide et la version

Pour afficher l'aide :

```bash
escale-epc --help
```

ou :

```bash
escale-epc help
```

Pour afficher la version EPC supportée :

```bash
escale-epc --version
```

Sans argument, le binaire affiche aussi la version.

## Codes de sortie

Les commandes utilisent trois comportements simples :

- `0` : succès ;
- `1` : commande exécutée, mais EPC ou image invalide ;
- `2` : erreur d'usage, fichier déjà existant, erreur d'I/O ou échec technique.

Les commandes de validation écrivent le rapport JSON sur `stdout`. Les erreurs
d'usage et les diagnostics d'échec sont écrits sur `stderr`.

## Créer un brouillon EPC

La commande `create` initialise un dossier EPC unpacked.

```bash
escale-epc create drafts/postcard-001 "Alice"
```

Elle crée les dossiers nécessaires, écrit `manifest.json`, initialise
`text/message.md` si absent, et supprime les preuves générées obsolètes.

Le dossier créé est affiché sur `stdout`.

Pour remplacer un `manifest.json` existant :

```bash
escale-epc create --force drafts/postcard-001 "Alice"
```

Sans `--force`, la commande refuse d'écraser un manifest existant.

Après création, l'application ou l'utilisateur doit encore fournir :

- une image source pour la couverture ;
- `text/message.md`

La commande `image prepare` produit ensuite `media/cover.jxl` et dérive
automatiquement `media/thumbnail.jxl` à partir de cette couverture.

## Valider une archive EPC

Pour valider une archive `.epc` :

```bash
escale-epc validate dist/postcard.epc
```

La commande affiche un `ValidationReport` JSON sur `stdout`.

Si le rapport est valide, le code de sortie est `0`. Si le rapport contient une
erreur ou un fatal, le code de sortie est `1`.

Exemple d'usage dans un script :

```bash
if escale-epc validate dist/postcard.epc > report.json; then
  echo "EPC valide"
else
  echo "EPC invalide, voir report.json"
fi
```

## Valider un dossier unpacked

Pour valider un dossier EPC avant packing :

```bash
escale-epc validate-dir drafts/postcard-001
```

La commande utilise `epc-validate::validate_core_directory` et affiche le même
type de rapport JSON que `validate`.

Elle est utile pendant la construction d'un EPC, avant de produire l'archive
finale.

## Packer un EPC

La commande `pack` assemble un dossier source en archive `.epc`.

```bash
escale-epc pack drafts/postcard-001 dist
```

Le second argument est optionnel. S'il est absent, le dossier parent du dossier
source est utilisé.

```bash
escale-epc pack drafts/postcard-001
```

La commande :

1. scelle le manifest si `sealed_at` est vide ;
2. régénère `proof/hashes.json` ;
3. valide le dossier staging ;
4. écrit l'archive `.epc` avec un nom canonique ;
5. affiche le chemin du fichier généré sur `stdout`.

Le nom final suit la forme :

```text
<TIME6>-<ID10>.epc
```

Si le fichier de sortie existe déjà, la commande refuse de l'écraser.

## Signer pendant le packing

`pack` peut signer avec une clé privée OpenSSH Ed25519 non chiffrée avant
d'écrire l'archive.

```bash
escale-epc pack --sign keys/author_ed25519 drafts/postcard-001 dist
```

Si `proof/signature.json` existe déjà, il est conservé par défaut. Pour le
remplacer :

```bash
escale-epc pack --sign keys/author_ed25519 --force drafts/postcard-001 dist
```

`--force` n'est accepté avec `pack` que si `--sign` est aussi présent.

## Signer un dossier unpacked

Pour écrire uniquement `proof/signature.json` sans packer :

```bash
escale-epc sign --ssh-key keys/author_ed25519 drafts/postcard-001
```

Le chemin du fichier de signature généré est affiché sur `stdout`.

Pour remplacer une signature existante :

```bash
escale-epc sign --force --ssh-key keys/author_ed25519 drafts/postcard-001
```

La clé doit être une clé privée OpenSSH Ed25519 non chiffrée. Les clés chiffrées
ne sont pas encore supportées par ce helper bas niveau.

## Commandes image

Les commandes image sont regroupées sous :

```bash
escale-epc image <command> ...
```

Pour afficher l'aide spécifique :

```bash
escale-epc image help
```

Les sous-commandes disponibles sont :

- `image info`
- `image validate`
- `image preview`
- `image encode`
- `image prepare`

`cover` et `thumbnail` sont les deux valeurs acceptées par `--kind`.
`thumb` est aussi accepté comme alias de `thumbnail`.

## Inspecter une image JXL

`image info` valide une image JXL et affiche ses métadonnées.

```bash
escale-epc image info media/cover.jxl --kind cover
```

`--kind` est optionnel pour `image info` et vaut `cover` par défaut.

La sortie contient :

```text
path: media/cover.jxl
kind: cover
width: ...
height: ...
pixels: ...
file_bytes: ...
valid: true
```

La commande échoue avec un code `1` si l'image n'est pas un JXL valide pour le
type demandé.

## Valider une image JXL

`image validate` vérifie qu'un fichier JXL respecte les limites EPC du type
demandé.

```bash
escale-epc image validate media/cover.jxl --kind cover
```

Pour une miniature :

```bash
escale-epc image validate media/thumbnail.jxl --kind thumbnail
```

Ici, `--kind` est obligatoire. La commande utilise les limites de `cover` ou
`thumbnail` définies par `epc-core`.

## Générer une preview PNG

`image preview` décode un JXL en RGBA8, le redimensionne, puis écrit un PNG.

```bash
escale-epc image preview media/cover.jxl --out preview.png
```

La taille maximale par défaut est `1024` pixels.

Pour choisir une autre taille maximale :

```bash
escale-epc image preview media/cover.jxl --out preview.png --max 512
```

Pour préciser le type d'image :

```bash
escale-epc image preview media/thumbnail.jxl --out thumb.png --kind thumbnail --max 256
```

La commande refuse d'écraser le fichier de sortie. Pour remplacer un PNG
existant :

```bash
escale-epc image preview media/cover.jxl --out preview.png --force
```

## Encoder une image en JXL

`image encode` convertit un fichier supporté par `cjxl`, typiquement JPEG ou
PNG, vers JPEG XL, puis valide le résultat comme `cover` ou `thumbnail`.

```bash
escale-epc image encode source.jpg media/cover.jxl --kind cover
```

Pour une miniature :

```bash
escale-epc image encode source.png media/thumbnail.jxl --kind thumbnail
```

Options disponibles :

- `--cjxl <path>` : chemin explicite vers l'exécutable `cjxl` ;
- `--distance <n>` : distance JPEG XL ;
- `--quality <n>` : qualité JPEG XL ;
- `--effort <n>` : effort encodeur ;
- `--force` : autorise l'écrasement du fichier de sortie.

Exemple lossless explicite :

```bash
escale-epc image encode source.jpg media/cover.jxl --kind cover --distance 0 --effort 7
```

Exemple avec chemin `cjxl` explicite :

```bash
escale-epc image encode source.jpg media/cover.jxl --kind cover --cjxl /usr/local/bin/cjxl
```

Si `cjxl` n'est pas disponible dans le `PATH`, installer les outils libjxl ou
passer `--cjxl <path>`.

## Préparer les images d'un brouillon

`image prepare` est la commande la plus pratique pour alimenter un draft EPC à
partir d'une image source.

```bash
escale-epc image prepare photo.jpg drafts/postcard-001
```

La commande :

1. crée `drafts/postcard-001/media` si nécessaire ;
2. encode la source en `media/cover.jxl` ;
3. décode la couverture en RGBA8 ;
4. génère une miniature redimensionnée dans une boîte de 1024x1024 pixels ;
5. encode cette miniature en `media/thumbnail.jxl` ;
6. valide les deux fichiers JXL ;
7. affiche les deux chemins générés sur `stdout`.

La miniature conserve le ratio de la couverture, n'est pas recadrée et n'est
pas agrandie si la couverture tient déjà dans 1024x1024 pixels.

Options disponibles :

- `--cjxl <path>` ;
- `--distance <n>` ;
- `--quality <n>` ;
- `--effort <n>` ;
- `--force`.

Exemple :

```bash
escale-epc image prepare photo.jpg drafts/postcard-001 --distance 0 --effort 7
```

Si `media/cover.jxl` ou `media/thumbnail.jxl` existe déjà, la commande échoue
sauf avec `--force`.

## Générer les vecteurs de conformité

Pour régénérer les archives de conformité du profil `core-format` :

```bash
escale-epc generate-test-vectors test-vectors/core-format
```

La commande écrit les archives valides, invalides, avec warnings, ainsi que les
rapports JSON attendus. Elle est surtout destinée au développement du SDK et aux
autres implémentations du format.

## Workflow complet

Un flux minimal de création ressemble à ceci :

```bash
escale-epc create drafts/postcard-001 "Alice"
escale-epc image prepare photo.jpg drafts/postcard-001
printf "Bonjour depuis Escale.\n" > drafts/postcard-001/text/message.md
escale-epc validate-dir drafts/postcard-001
escale-epc pack drafts/postcard-001 dist
```

Avec signature :

```bash
escale-epc create drafts/postcard-001 "Alice"
escale-epc image prepare photo.jpg drafts/postcard-001
printf "Bonjour depuis Escale.\n" > drafts/postcard-001/text/message.md
escale-epc sign --ssh-key keys/author_ed25519 drafts/postcard-001
escale-epc pack drafts/postcard-001 dist
```

Ou signature intégrée au packing :

```bash
escale-epc pack --sign keys/author_ed25519 drafts/postcard-001 dist
```

## Choisir la bonne commande

Pour créer un dossier de travail, utiliser `create`.

Pour préparer `cover.jxl` et `thumbnail.jxl`, utiliser `image prepare`.

Pour contrôler une image seule, utiliser `image info` ou `image validate`.

Pour générer une preview PNG, utiliser `image preview`.

Pour convertir explicitement un fichier image vers JXL, utiliser `image encode`.

Pour valider un dossier avant packing, utiliser `validate-dir`.

Pour valider une archive finale, utiliser `validate`.

Pour signer sans packer, utiliser `sign`.

Pour produire l'archive `.epc`, utiliser `pack`.

Pour les tests de conformité, utiliser `generate-test-vectors`.
