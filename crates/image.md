# Guide d'utilisation de `epc-image`

Le crate `epc-image` fournit les fonctions de base pour travailler avec les
images JPEG XL d'un EPC. Son rôle est volontairement centré sur trois besoins :
valider un fichier JXL, produire une image RGBA8 prête à l'affichage, et encoder
une source image vers JXL lorsque la feature d'encodage est activée.

Le contrat d'affichage est simple : les fonctions de rendu retournent un
`RgbaImage`, c'est-à-dire une largeur, une hauteur, et un buffer de pixels
interleavés dans l'ordre `R, G, B, A`. Ce format est facile à adapter ensuite
vers Flutter, une app native, du JavaScript, un viewer desktop ou un export PNG
temporaire.

## Générer la documentation Rust

La documentation publique générée par `rustdoc` se construit avec :

```bash
cargo doc -p epc-image --no-deps
```

Pour inclure aussi les helpers internes documentés dans le fichier source :

```bash
cargo doc -p epc-image --no-deps --document-private-items
```

Si l'on veut inclure l'API d'encodage JXL :

```bash
cargo doc -p epc-image --no-deps --features jxl-encode-libjxl
```

## Choisir le type d'image EPC

La plupart des fonctions prennent un `EpcImageKind`. Ce type indique à la fois
le chemin canonique dans l'EPC et les limites de taille à appliquer.

```rust
use epc_image::EpcImageKind;

let cover = EpcImageKind::Cover;
assert_eq!(cover.core_path(), "media/cover.jxl");

let thumbnail = EpcImageKind::Thumbnail;
assert_eq!(thumbnail.core_path(), "media/thumbnail.jxl");
```

`EpcImageKind::Cover` correspond à `media/cover.jxl`.

`EpcImageKind::Thumbnail` correspond à `media/thumbnail.jxl`.

Pour convertir un chemin EPC canonique en type :

```rust
use epc_image::EpcImageKind;

let kind = EpcImageKind::from_core_path("media/cover.jxl");
assert_eq!(kind, Some(EpcImageKind::Cover));
```

Les méthodes `max_pixels()` et `max_dimension()` exposent les limites applicables
à chaque type d'image. Elles sont surtout utiles pour afficher des diagnostics ou
préparer une interface utilisateur.

## Valider un fichier JXL

Pour vérifier qu'un fichier est bien un JPEG XL décodable et qu'il respecte les
limites EPC :

```rust
use epc_image::{validate_jxl_file, EpcImageKind};

let info = validate_jxl_file("media/cover.jxl", EpcImageKind::Cover)?;

println!("dimensions: {}x{}", info.width, info.height);
println!("pixels: {}", info.pixels);
```

`validate_jxl_file` ne se contente pas de lire un en-tête. La fonction vérifie
les dimensions, le nombre total de pixels, puis tente de rendre la première
frame. Cela permet de rejeter un fichier corrompu avant qu'une app cliente tente
de l'afficher.

Le résultat est un `JxlInfo` :

```rust
pub struct JxlInfo {
    pub width: u32,
    pub height: u32,
    pub pixels: u64,
}
```

Les dimensions tiennent compte de l'orientation JPEG XL.

## Décoder des octets JXL en RGBA8

Si l'application a déjà chargé les octets JXL depuis une base de données, une
archive ou une couche réseau, elle peut appeler `decode_jxl_rgba8` :

```rust
use epc_image::{decode_jxl_rgba8, EpcImageKind, RenderOptions};

let bytes = std::fs::read("media/cover.jxl")?;
let image = decode_jxl_rgba8(
    &bytes,
    EpcImageKind::Cover,
    RenderOptions::fit(1024, 1024),
)?;

assert_eq!(image.pixels.len(), image.expected_len());
```

Le résultat est un `RgbaImage` :

```rust
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
```

Le buffer `pixels` contient exactement `width * height * 4` octets lorsque
l'image est valide. La méthode `expected_len()` permet de le vérifier.

## Décoder un fichier JXL en RGBA8

Pour lire directement un fichier `.jxl` :

```rust
use epc_image::{decode_jxl_file_rgba8, EpcImageKind, RenderOptions};

let image = decode_jxl_file_rgba8(
    "media/thumbnail.jxl",
    EpcImageKind::Thumbnail,
    RenderOptions::fit(256, 256),
)?;

println!("thumbnail: {}x{}", image.width, image.height);
```

Cette fonction est utile pour les outils CLI, les tests, ou les viewers qui
manipulent un fichier image isolé.

## Configurer le redimensionnement

Les fonctions de rendu utilisent `RenderOptions`.

Pour conserver la taille décodée :

```rust
use epc_image::RenderOptions;

let options = RenderOptions::original();
```

Pour adapter l'image dans une boîte maximale :

```rust
use epc_image::RenderOptions;

let options = RenderOptions::fit(1024, 1024);
```

Le mode `fit` conserve le ratio d'origine et ne force pas d'agrandissement si
l'image tient déjà dans la taille demandée. Les dimensions maximales doivent
être strictement supérieures à zéro.

Pour générer le thumbnail canonique d'un EPC, partir de `media/cover.jxl` :

```rust
use epc_image::{
    derive_thumbnail_rgba8_from_cover_jxl_file,
    encode_rgba8_to_jxl_file,
    EncodeOptions,
};

let thumbnail = derive_thumbnail_rgba8_from_cover_jxl_file("media/cover.jxl")?;
encode_rgba8_to_jxl_file(
    &thumbnail,
    "media/thumbnail.jxl",
    &EncodeOptions::default(),
)?;
```

Cette fonction applique la règle EPC : boîte maximale de 1024x1024 pixels,
ratio de la couverture conservé, aucun recadrage et aucun agrandissement si
l'image est déjà plus petite. Le résultat est ensuite encodé en
`media/thumbnail.jxl`.

Avec la feature `jxl-encode-libjxl`, `encode_thumbnail_from_cover_jxl_file`
combine ces étapes :

```rust
use epc_image::{encode_thumbnail_from_cover_jxl_file, EncodeOptions};

encode_thumbnail_from_cover_jxl_file(
    "media/cover.jxl",
    "media/thumbnail.jxl",
    &EncodeOptions::default(),
)?;
```

## Rendre une image depuis un dossier EPC unpacked

Quand un EPC a déjà été décompressé dans un dossier, on peut lire directement
les fichiers `media/cover.jxl` ou `media/thumbnail.jxl`.

Pour la couverture :

```rust
use epc_image::{render_cover_from_directory_rgba8, RenderOptions};

let cover = render_cover_from_directory_rgba8(
    "album.epc.unpacked",
    RenderOptions::fit(1024, 1024),
)?;
```

Pour la miniature :

```rust
use epc_image::{render_thumbnail_from_directory_rgba8, RenderOptions};

let thumbnail = render_thumbnail_from_directory_rgba8(
    "album.epc.unpacked",
    RenderOptions::fit(256, 256),
)?;
```

Pour choisir dynamiquement le type d'image :

```rust
use epc_image::{render_image_from_directory_rgba8, EpcImageKind, RenderOptions};

let image = render_image_from_directory_rgba8(
    "album.epc.unpacked",
    EpcImageKind::Cover,
    RenderOptions::fit(1024, 1024),
)?;
```

Ces fonctions lisent uniquement l'image demandée. Elles ne valident pas tout le
contenu de l'EPC.

## Rendre une image depuis une archive `.epc`

Quand l'EPC est encore sous forme d'archive ZIP `.epc`, les fonctions suivantes
extraient l'entrée image demandée puis la décodent en RGBA8.

Pour la couverture :

```rust
use epc_image::{render_cover_from_epc_rgba8, RenderOptions};

let cover = render_cover_from_epc_rgba8(
    "album.epc",
    RenderOptions::fit(1024, 1024),
)?;
```

Pour la miniature :

```rust
use epc_image::{render_thumbnail_from_epc_rgba8, RenderOptions};

let thumbnail = render_thumbnail_from_epc_rgba8(
    "album.epc",
    RenderOptions::fit(256, 256),
)?;
```

Pour choisir dynamiquement le type d'image :

```rust
use epc_image::{render_image_from_epc_rgba8, EpcImageKind, RenderOptions};

let image = render_image_from_epc_rgba8(
    "album.epc",
    EpcImageKind::Thumbnail,
    RenderOptions::fit(256, 256),
)?;
```

Ces fonctions sont pensées pour l'affichage rapide. La validation complète du
manifest, des preuves ou des hashes reste du ressort des autres crates du SDK.

## Encoder vers JXL

L'API d'encodage est disponible uniquement avec la feature Cargo
`jxl-encode-libjxl`.

Elle s'appuie sur l'exécutable `cjxl` fourni par libjxl. L'objectif est de ne pas
réimplémenter l'encodage JPEG XL dans le SDK Rust.

Exemple de compilation :

```bash
cargo build -p epc-image --features jxl-encode-libjxl
```

### Configurer l'encodage

Les options d'encodage sont portées par `EncodeOptions`.

```rust
use epc_image::EncodeOptions;

let options = EncodeOptions::default()
    .with_distance(0.0)
    .with_effort(7);
```

Par défaut, le SDK cherche `cjxl` dans le `PATH`. Pour fournir un chemin
explicite :

```rust
use epc_image::EncodeOptions;

let options = EncodeOptions::default()
    .with_cjxl_path("/usr/local/bin/cjxl");
```

Pour piloter l'encodage avec une qualité plutôt qu'une distance :

```rust
use epc_image::EncodeOptions;

let options = EncodeOptions::default()
    .with_quality(100.0)
    .with_effort(7);
```

`with_quality` désactive la distance configurée auparavant.

### Encoder un JPEG ou un PNG

Pour convertir un JPEG en JXL :

```rust
use epc_image::{encode_jpeg_file_to_jxl_file, EncodeOptions};

let options = EncodeOptions::default();
encode_jpeg_file_to_jxl_file("photo.jpg", "media/cover.jxl", &options)?;
```

Pour convertir un PNG en JXL :

```rust
use epc_image::{encode_png_file_to_jxl_file, EncodeOptions};

let options = EncodeOptions::default();
encode_png_file_to_jxl_file("cover.png", "media/cover.jxl", &options)?;
```

Pour laisser `cjxl` gérer un type de fichier supporté :

```rust
use epc_image::{encode_file_to_jxl_file, EncodeOptions};

let options = EncodeOptions::default();
encode_file_to_jxl_file("source.png", "media/cover.jxl", &options)?;
```

Ces fonctions ne redimensionnent pas l'image. Si une app veut imposer une taille
maximale avant encodage, elle doit le faire avant d'appeler le SDK.

### Encoder une image RGBA8

Pour encoder des pixels RGBA8 déjà en mémoire :

```rust
use epc_image::{encode_rgba8_to_jxl_file, EncodeOptions, RgbaImage};

let image = RgbaImage {
    width: 2,
    height: 2,
    pixels: vec![
        255, 0, 0, 255,
        0, 255, 0, 255,
        0, 0, 255, 255,
        255, 255, 255, 255,
    ],
};

let options = EncodeOptions::default();
encode_rgba8_to_jxl_file(&image, "media/thumbnail.jxl", &options)?;
```

Pour récupérer directement les octets JXL :

```rust
use epc_image::{encode_rgba8_to_jxl_bytes, EncodeOptions, RgbaImage};

let image = RgbaImage {
    width: 1,
    height: 1,
    pixels: vec![255, 255, 255, 255],
};

let options = EncodeOptions::default();
let bytes = encode_rgba8_to_jxl_bytes(&image, &options)?;
```

En interne, l'encodage RGBA8 passe par un PNG temporaire afin de fournir une
source lossless à `cjxl`.

### Écrire un PNG depuis RGBA8

`write_rgba_png_file` permet d'écrire un `RgbaImage` en PNG :

```rust
use epc_image::{write_rgba_png_file, RgbaImage};

let image = RgbaImage {
    width: 1,
    height: 1,
    pixels: vec![0, 0, 0, 255],
};

write_rgba_png_file(&image, "preview.png")?;
```

Cette fonction est pratique pour le debug, les previews temporaires ou les
tests. Le format image canonique de l'EPC reste le JPEG XL.

## Gérer les erreurs

La validation JXL retourne `JxlValidationError`.

Les cas importants sont :

- `Io` : le fichier ne peut pas être ouvert ou lu ;
- `InvalidBitstream` : le contenu n'est pas un JPEG XL valide ;
- `DimensionsExceeded` : une dimension dépasse la limite autorisée ;
- `PixelsExceeded` : le nombre total de pixels dépasse la limite autorisée ;
- `DecodeFailed` : le fichier semble lisible mais la première frame ne peut pas
  être rendue.

Les fonctions d'affichage retournent `DisplayError`.

Les cas importants sont :

- `Io` : erreur de lecture ;
- `InvalidZip` : l'archive `.epc` n'est pas un ZIP valide ;
- `MissingImage` : `media/cover.jxl` ou `media/thumbnail.jxl` est absent ;
- `Jxl` : erreur de validation ou de décodage JPEG XL ;
- `ImageTooLarge` : l'allocation nécessaire serait trop grande ;
- `UnsupportedChannelCount` : le décodeur a retourné un layout de canaux non
  supporté ;
- `InvalidOptions` : les options de rendu sont invalides.

Les fonctions d'encodage retournent `EncodeError`.

Les cas importants sont :

- `Io` : erreur de fichier ou de processus ;
- `InvalidRgba` : le buffer RGBA8 n'a pas la taille attendue ;
- `InvalidOptions` : les options d'encodage sont invalides ;
- `Png` : l'écriture du PNG temporaire a échoué ;
- `CjxlFailed` : `cjxl` a démarré mais a échoué ;
- `CjxlUnavailable` : l'exécutable `cjxl` n'a pas pu être lancé.

## Choisir la bonne fonction

Pour valider un fichier image isolé, utiliser `validate_jxl_file`.

Pour afficher des octets JXL déjà en mémoire, utiliser `decode_jxl_rgba8`.

Pour afficher un fichier JXL isolé, utiliser `decode_jxl_file_rgba8`.

Pour afficher une couverture ou une miniature depuis un dossier EPC unpacked,
utiliser `render_cover_from_directory_rgba8` ou
`render_thumbnail_from_directory_rgba8`.

Pour afficher une couverture ou une miniature depuis une archive `.epc`, utiliser
`render_cover_from_epc_rgba8` ou `render_thumbnail_from_epc_rgba8`.

Pour encoder un JPEG ou PNG en JXL avec libjxl, activer `jxl-encode-libjxl` et
utiliser `encode_jpeg_file_to_jxl_file`, `encode_png_file_to_jxl_file` ou
`encode_file_to_jxl_file`.

Pour encoder des pixels RGBA8, utiliser `encode_rgba8_to_jxl_file` ou
`encode_rgba8_to_jxl_bytes`.
