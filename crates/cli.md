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

La commande `create` initialise un dossier EPC unpacked à partir d'un dossier
préparé ou d'une image de couverture.

```bash
escale-epc create photo.jpg "Alice" "Bonjour depuis Escale."
```

Avec une image `.jpg`, `.jpeg`, `.png`, `.webp` ou `.jxl`, elle crée un dossier
frère nommé `escale-TTTTTT-RR`, copie l'image sans modification dans
`media/cover.*`, dérive `media/thumbnail.jxl`, écrit `manifest.json`, initialise
`text/message.md` si absent, et supprime les preuves générées obsolètes.

`TTTTTT` correspond au `HHMMSS` local du device encodé en base36 sur six
caractères ; `RR` est un suffixe court anti-collision.

Avec un dossier, celui-ci doit déjà exister et contenir une image principale
supportée : `media/cover.jpg`, `media/cover.jpeg`, `media/cover.png`,
`media/cover.webp` ou `media/cover.jxl`.

Si un message optionnel est fourni, il est écrit dans `text/message.md`. Sinon,
le fichier est créé vide et peut être modifié avant le packing.

Les options d'encodage de `image prepare` sont aussi acceptées quand `create`
reçoit une image, par exemple `--quality <n>`, `--distance <n>` ou `--effort <n>`.

Le dossier créé est affiché sur `stdout`.

Pour remplacer un `manifest.json` existant :

```bash
escale-epc create --force drafts/postcard-001 "Alice"
```

Sans `--force`, la commande refuse d'écraser un manifest existant.

Après création, l'application ou l'utilisateur doit encore fournir :

- `text/message.md`, uniquement si le message n'a pas été fourni à `create`

La commande `image prepare` reste disponible pour copier une couverture
supportée sans modification et dériver automatiquement `media/thumbnail.jxl`
dans un dossier déjà existant.

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

Par défaut, la commande :

1. met le manifest en statut `sealed` et renseigne `sealed_at` si nécessaire ;
2. régénère `proof/hashes.json` ;
3. valide le dossier staging ;
4. écrit l'archive `.epc` avec un nom canonique ;
5. affiche le chemin du fichier généré sur `stdout`.

Le nom final suit la forme :

```text
<TIME6>-<ID10>.epc
```

Si le fichier de sortie existe déjà, la commande refuse de l'écraser.

Pour produire une carte de voyage à remettre à l'infrastructure Escale :

```bash
escale-epc pack --issued drafts/postcard-001 dist
```

Dans ce mode, le manifest passe en statut `issued`, `sealed_at` reste vide, et
le fichier généré suit la forme `escale-<ID10>.epc`. `--issued` ne peut pas être
combiné avec `--sign`, car la signature actuelle scelle la carte.

`pack` attend toujours un dossier EPC unpacked comme source. Une archive `.epc`
déjà produite, qu'elle soit `issued` ou `sealed`, doit être validée avec
`validate`, pas repackée par la CLI publique.

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

Ce flux produit une carte `sealed`. Il ne s'applique pas à une carte `issued`,
dont la finalisation appartient à l'infrastructure de voyage Escale.

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

`image encode` convertit un JPEG, PNG ou WebP vers JPEG XL avec l'encodeur Rust
du SDK, puis valide le résultat comme `cover` ou `thumbnail`.

```bash
escale-epc image encode source.jpg media/cover.jxl --kind cover
```

Pour une miniature :

```bash
escale-epc image encode source.png media/thumbnail.jxl --kind thumbnail
```

Options disponibles :

- `--distance <n>` : distance JPEG XL ;
- `--quality <n>` : qualité JPEG XL ;
- `--effort <n>` : option réservée pour compatibilité CLI ;
- `--force` : autorise l'écrasement du fichier de sortie.

Exemple lossless explicite :

```bash
escale-epc image encode source.jpg media/cover.jxl --kind cover --distance 0 --effort 7
```

## Préparer les images d'un brouillon

`image prepare` est la commande la plus pratique pour alimenter un draft EPC à
partir d'une image source.

```bash
escale-epc image prepare photo.jpg drafts/postcard-001
```

La commande :

1. crée `drafts/postcard-001/media` si nécessaire ;
2. copie la source sans modification dans `media/cover.*` ;
3. décode la couverture copiée en RGBA8 ;
4. génère une miniature redimensionnée dans une boîte de 256x256 pixels ;
5. encode cette miniature en `media/thumbnail.jxl` ;
6. valide le thumbnail JXL ;
7. affiche les deux chemins générés sur `stdout`.

La miniature conserve le ratio de la couverture, n'est pas recadrée et n'est
pas agrandie si la couverture tient déjà dans 256x256 pixels.
Par défaut, elle est encodée en qualité 80 plutôt qu'en lossless afin de rester
nettement plus légère que la couverture.

Options disponibles :

- `--distance <n>` ;
- `--quality <n>` ;
- `--effort <n>` : option réservée pour compatibilité CLI ;
- `--force`.

Exemple :

```bash
escale-epc image prepare photo.jpg drafts/postcard-001 --distance 0 --effort 7
```

Si une image principale supportée `media/cover.*` ou `media/thumbnail.jxl`
existe déjà, la commande échoue sauf avec `--force`.

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
draft_dir="$(escale-epc create photo.jpg "Alice" "Bonjour depuis Escale.")"
escale-epc validate-dir "$draft_dir"
escale-epc pack "$draft_dir" dist
```

Avec signature :

```bash
draft_dir="$(escale-epc create photo.jpg "Alice" "Bonjour depuis Escale.")"
escale-epc sign --ssh-key keys/author_ed25519 "$draft_dir"
escale-epc pack "$draft_dir" dist
```

Ou signature intégrée au packing :

```bash
escale-epc pack --sign keys/author_ed25519 drafts/postcard-001 dist
```

## Choisir la bonne commande

Pour créer un dossier de travail, utiliser `create`.

Pour préparer une cover acceptée et `thumbnail.jxl`, utiliser `image prepare`.

Pour contrôler une image seule, utiliser `image info` ou `image validate`.

Pour générer une preview PNG, utiliser `image preview`.

Pour convertir explicitement un fichier image vers JXL, utiliser `image encode`.

Pour valider un dossier avant packing, utiliser `validate-dir`.

Pour valider une archive finale, utiliser `validate`.

Pour signer sans packer, utiliser `sign`.

Pour produire l'archive `.epc`, utiliser `pack`.

Pour les tests de conformité, utiliser `generate-test-vectors`.
