# Guide d'utilisation de `epc-core`

Le crate `epc-core` contient le vocabulaire commun du format EPC. Il ne lit pas
le filesystem, ne manipule pas les archives ZIP et ne calcule pas les hashes.
Son rôle est de fournir aux autres crates les constantes, les chemins, les
limites et les structures JSON partagées du profil EPC 1.0 `core-format`.

On l'utilise typiquement dans :

- `epc-validate`, pour vérifier qu'un EPC respecte le format attendu ;
- `epc-pack`, pour produire une archive conforme ;
- `epc-image`, pour réutiliser les chemins et limites des médias ;
- `epc-cli`, pour afficher des diagnostics cohérents avec le SDK.

## Générer la documentation Rust

La documentation publique se génère avec :

```bash
cargo doc -p epc-core --no-deps
```

Pour inclure aussi les éventuels helpers privés :

```bash
cargo doc -p epc-core --no-deps --document-private-items
```

## Constantes de version et de profil

`epc-core` expose les noms canoniques utilisés dans les documents EPC.

```rust
use epc_core::{
    CORE_PROFILE,
    EPC_MIME_TYPE,
    EPC_OBJECT_TYPE_POSTCARD,
    EPC_VERSION_1_0,
};

assert_eq!(EPC_VERSION_1_0, "1.0");
assert_eq!(CORE_PROFILE, "core-format");
assert_eq!(EPC_OBJECT_TYPE_POSTCARD, "postcard");
assert_eq!(EPC_MIME_TYPE, "application/vnd.escale.epc+zip");
```

Ces constantes évitent de dupliquer des chaînes sensibles dans les validateurs,
packers et bindings.

Les constantes de domaines cryptographiques sont :

```rust
use epc_core::{CORE_DOMAIN_SEPARATOR, SIGNATURE_DOMAIN_SEPARATOR};

assert_eq!(CORE_DOMAIN_SEPARATOR, "EPC-CORE-V1\n");
assert_eq!(SIGNATURE_DOMAIN_SEPARATOR, "EPC-SIGNATURE-V1\n");
```

Elles doivent être utilisées par les crates responsables du calcul d'intégrité
ou de signature afin d'éviter toute ambiguïté entre plusieurs usages
cryptographiques.

## Chemins canoniques du profil core

Les fichiers attendus dans un EPC `core-format` sont décrits par constantes.

```rust
use epc_core::{
    COVER_PATH,
    HASHES_PATH,
    MANIFEST_PATH,
    MESSAGE_PATH,
    SIGNATURE_PATH,
    THUMBNAIL_PATH,
};

assert_eq!(MANIFEST_PATH, "manifest.json");
assert_eq!(COVER_PATH, "media/cover.jxl");
assert_eq!(THUMBNAIL_PATH, "media/thumbnail.jxl");
assert_eq!(MESSAGE_PATH, "text/message.md");
assert_eq!(HASHES_PATH, "proof/hashes.json");
assert_eq!(SIGNATURE_PATH, "proof/signature.json");
```

`EXPECTED_CORE_FILES` contient les cinq fichiers réguliers obligatoires :

```rust
use epc_core::EXPECTED_CORE_FILES;

for path in EXPECTED_CORE_FILES {
    println!("required: {path}");
}
```

`EXPECTED_HASHED_CORE_FILES` contient les fichiers couverts par
`proof/hashes.json`. Le fichier `proof/hashes.json` lui-même est exclu pour
éviter un hash récursif.

`OPTIONAL_PROOF_FILES` contient les preuves reconnues mais non obligatoires,
actuellement `proof/signature.json`.

`ALLOWED_DIRECTORY_ENTRIES` contient les dossiers tolérés dans l'archive ZIP :
`media`, `text` et `proof`. Ces entrées de dossier sont pratiques pour le
conteneur, mais elles ne font pas partie du coeur immuable.

## Limites de taille

Le profil impose des limites afin de garder les EPC prévisibles sur mobile et
simples à valider.

Les limites globales sont :

```rust
use epc_core::{
    MAX_ARCHIVE_SIZE,
    MAX_REGULAR_FILES,
    MAX_TOTAL_UNCOMPRESSED_SIZE,
    MAX_ZIP_ENTRIES,
};

println!("archive max: {MAX_ARCHIVE_SIZE} bytes");
println!("uncompressed max: {MAX_TOTAL_UNCOMPRESSED_SIZE} bytes");
println!("zip entries max: {MAX_ZIP_ENTRIES}");
println!("regular files max: {MAX_REGULAR_FILES}");
```

Les limites de chemins sont :

```rust
use epc_core::{MAX_PATH_BYTES, MAX_PATH_DEPTH};

println!("path bytes max: {MAX_PATH_BYTES}");
println!("path depth max: {MAX_PATH_DEPTH}");
```

Les limites par fichier sont exposées individuellement :

```rust
use epc_core::{
    MAX_COVER_SIZE,
    MAX_HASHES_SIZE,
    MAX_MANIFEST_SIZE,
    MAX_MESSAGE_SIZE,
    MAX_SIGNATURE_SIZE,
    MAX_THUMBNAIL_SIZE,
};

println!("manifest max: {MAX_MANIFEST_SIZE}");
println!("cover max: {MAX_COVER_SIZE}");
println!("thumbnail max: {MAX_THUMBNAIL_SIZE}");
println!("message max: {MAX_MESSAGE_SIZE}");
println!("hashes max: {MAX_HASHES_SIZE}");
println!("signature max: {MAX_SIGNATURE_SIZE}");
```

Pour obtenir la limite à partir d'un chemin EPC, utiliser
`expected_file_size_limit` :

```rust
use epc_core::{expected_file_size_limit, COVER_PATH};

let limit = expected_file_size_limit(COVER_PATH);
assert!(limit.is_some());
```

Un chemin inconnu retourne `None`.

## Limites d'image et Markdown

Les limites d'image sont utilisées notamment par `epc-image`.

```rust
use epc_core::{
    MAX_COVER_DIMENSION,
    MAX_COVER_PIXELS,
    MAX_THUMBNAIL_DIMENSION,
    MAX_THUMBNAIL_PIXELS,
};

println!("cover pixels max: {MAX_COVER_PIXELS}");
println!("cover side max: {MAX_COVER_DIMENSION}");
println!("thumbnail pixels max: {MAX_THUMBNAIL_PIXELS}");
println!("thumbnail side max: {MAX_THUMBNAIL_DIMENSION}");
```

Un thumbnail EPC est dérivé de l'image principale déclarée par
`content.cover.path` (`media/cover.jpg`, `media/cover.jpeg`,
`media/cover.png`, `media/cover.webp` ou `media/cover.jxl`) par
redimensionnement dans une boîte maximale de 256x256 pixels. Le ratio d'origine
est conservé, l'image n'est pas recadrée et elle n'est pas agrandie si elle
tient déjà dans cette boîte. Les limites normatives associées sont donc
1024 pixels par côté et 256 * 256 pixels décodés au maximum.

Les limites Markdown sont utilisées par le validateur du message :

```rust
use epc_core::{MAX_MARKDOWN_LINE_BYTES, MAX_MARKDOWN_LINKS};

println!("links max: {MAX_MARKDOWN_LINKS}");
println!("line bytes max: {MAX_MARKDOWN_LINE_BYTES}");
```

Le profil Markdown attendu est :

```rust
use epc_core::{MARKDOWN_CORE_PROFILE, MARKDOWN_CORE_PROFILE_VERSION};

assert_eq!(MARKDOWN_CORE_PROFILE, "epc-markdown-core");
assert_eq!(MARKDOWN_CORE_PROFILE_VERSION, "1.0");
```

## Vérifier les identifiants de carte

`is_valid_card_id` vérifie la forme canonique d'un identifiant EPC :
`escale:<ULID>`.

```rust
use epc_core::is_valid_card_id;

assert!(is_valid_card_id("escale:01ARZ3NDEKTSV4RRFFQ69G5FAV"));
assert!(!is_valid_card_id("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
assert!(!is_valid_card_id("escale:not-a-ulid"));
```

Le suffixe doit être un ULID de 26 caractères en Crockford Base32 majuscule.
Les lettres ambiguës comme `I`, `L`, `O` et `U` ne sont pas acceptées.

## Vérifier les chemins EPC

`is_safe_core_path` vérifie qu'un chemin relatif au contenu EPC respecte les
règles du profil.

```rust
use epc_core::is_safe_core_path;

assert!(is_safe_core_path("media/cover.jxl"));
assert!(is_safe_core_path("manifest.json"));

assert!(!is_safe_core_path("/media/cover.jxl"));
assert!(!is_safe_core_path("../secret.txt"));
assert!(!is_safe_core_path("media\\cover.jxl"));
assert!(!is_safe_core_path("media//cover.jxl"));
```

La fonction rejette notamment :

- les chemins vides ;
- les chemins absolus ;
- les backslashes ;
- les octets NUL ;
- les segments vides ;
- les segments `.` et `..` ;
- les chemins trop longs ;
- les chemins trop profonds.

## Savoir si un fichier est attendu ou autorisé

Pour savoir si un chemin correspond à un fichier obligatoire :

```rust
use epc_core::{is_expected_core_file, COVER_PATH, SIGNATURE_PATH};

assert!(is_expected_core_file(COVER_PATH));
assert!(!is_expected_core_file(SIGNATURE_PATH));
```

Pour savoir si un fichier régulier est autorisé, y compris les preuves
optionnelles :

```rust
use epc_core::{is_allowed_regular_file, SIGNATURE_PATH};

assert!(is_allowed_regular_file(SIGNATURE_PATH));
assert!(!is_allowed_regular_file("media/extra.jxl"));
```

Pour savoir si un fichier doit être couvert par `proof/hashes.json` :

```rust
use epc_core::{is_expected_hashed_core_file, HASHES_PATH, MANIFEST_PATH};

assert!(is_expected_hashed_core_file(MANIFEST_PATH));
assert!(!is_expected_hashed_core_file(HASHES_PATH));
```

## Construire un manifest

`Manifest` représente le contenu attendu de `manifest.json`.

```rust
use epc_core::{
    Author,
    Content,
    CreatedLocalTime,
    Manifest,
    ManifestStatus,
    MediaContent,
    MessageContent,
    CORE_PROFILE,
    COVER_PATH,
    EPC_OBJECT_TYPE_POSTCARD,
    EPC_VERSION_1_0,
    MARKDOWN_CORE_PROFILE,
    MARKDOWN_CORE_PROFILE_VERSION,
    MESSAGE_PATH,
    THUMBNAIL_PATH,
};

let manifest = Manifest {
    epc_version: EPC_VERSION_1_0.to_string(),
    profile: CORE_PROFILE.to_string(),
    object_type: EPC_OBJECT_TYPE_POSTCARD.to_string(),
    id: "escale:01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
    created_at: "2026-06-23T10:00:00Z".to_string(),
    created_local_time: CreatedLocalTime {
        time_zone: "Europe/Paris".to_string(),
        utc_offset: "+02:00".to_string(),
    },
    status: ManifestStatus::Sealed,
    sealed_at: "2026-06-23T10:05:00Z".to_string(),
    author: Author {
        display_name: "Alice".to_string(),
    },
    content: Content {
        cover: MediaContent {
            path: COVER_PATH.to_string(),
            mime: "image/jxl".to_string(),
        },
        thumbnail: MediaContent {
            path: THUMBNAIL_PATH.to_string(),
            mime: "image/jxl".to_string(),
        },
        message: MessageContent {
            path: MESSAGE_PATH.to_string(),
            mime: "text/markdown".to_string(),
            markdown_profile: MARKDOWN_CORE_PROFILE.to_string(),
            markdown_profile_version: MARKDOWN_CORE_PROFILE_VERSION.to_string(),
        },
    },
};
```

Le crate dérive `Serialize` et `Deserialize` sur les modèles publics. On peut
donc sérialiser le manifest avec `serde_json` :

```rust
let json = serde_json::to_string_pretty(&manifest)?;
```

Dans le JSON, le champ Rust `object_type` est sérialisé sous le nom `type`.

## Statut du manifest

`ManifestStatus` modélise le cycle de vie public d'une carte :

- `Draft` : dossier unpacked encore modifiable par une application, la CLI ou
  le SDK public ;
- `Issued` : archive `.epc` remise à l'infrastructure de voyage Escale ;
- `Sealed` : archive `.epc` finale et immutable.

La relation avec `sealed_at` est volontairement stricte :

- `Draft` et `Issued` gardent `sealed_at` vide ;
- `Sealed` doit renseigner `sealed_at`.

Les noms de fichiers suivent la même séparation :

- `draft` : dossier, par exemple `escale-<TIME6>-<RAND2>/` ;
- `issued` : archive `escale-<ID10>.epc` ;
- `sealed` : archive `<TIME6>-<ID10>.epc`.

## Construire un fichier `proof/hashes.json`

`Hashes` représente le descripteur d'intégrité EPC.

```rust
use epc_core::{
    HashEntry,
    HashTransform,
    Hashes,
    HASH_ALGORITHM_SHA256,
    INTEGRITY_VERSION_1,
    MANIFEST_PATH,
};

let hashes = Hashes {
    integrity_version: INTEGRITY_VERSION_1.to_string(),
    hash_algorithm: HASH_ALGORITHM_SHA256.to_string(),
    entries: vec![
        HashEntry {
            path: MANIFEST_PATH.to_string(),
            transform: HashTransform::Jcs,
            digest: "base64url-sha256-digest".to_string(),
        },
    ],
    core_digest: "base64url-core-digest".to_string(),
};
```

`HashTransform::Jcs` indique que l'entrée est normalisée par Canonical JSON
Serialization avant hash.

`HashTransform::Identity` indique un hash byte-for-byte, utile pour les fichiers
binaires comme les images JXL ou le Markdown déjà figé.

Le hash algorithm attendu pour EPC 1.0 est `sha-256`.

## Construire un fichier `proof/signature.json`

`SignatureProof` représente une preuve d'authenticité. Elle est requise par la
politique SDK pour les manifests `sealed`, et optionnelle pour les états de
travail `draft` ou de remise `issued`.

```rust
use epc_core::{
    SignatureEntry,
    SignaturePayload,
    SignaturePolicy,
    SignatureProof,
    SignaturePublicKey,
    SignatureRequiredKey,
    SignatureSigner,
    HASH_ALGORITHM_SHA256,
    SIGNATURE_DOMAIN_SEPARATOR,
};

let key = SignatureRequiredKey {
    algorithm: "Ed25519".to_string(),
    key_id: "base64url-jwk-thumbprint".to_string(),
};

let proof = SignatureProof {
    signature_version: "1".to_string(),
    payload: SignaturePayload {
        context: SIGNATURE_DOMAIN_SEPARATOR.trim_end().to_string(),
        card_id: "escale:01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        epc_version: "1.0".to_string(),
        core_digest: "base64url-core-digest".to_string(),
        hash_algorithm: HASH_ALGORITHM_SHA256.to_string(),
        signed_at: "2026-06-23T10:10:00Z".to_string(),
        signer: SignatureSigner {
            display_name: "Alice".to_string(),
            role: "author".to_string(),
        },
        policy: SignaturePolicy {
            mode: "all".to_string(),
            required_keys: vec![key.clone()],
        },
    },
    signatures: vec![
        SignatureEntry {
            algorithm: key.algorithm,
            key_id: key.key_id,
            public_key: SignaturePublicKey {
                kty: "OKP".to_string(),
                crv: "Ed25519".to_string(),
                x: "base64url-public-key".to_string(),
            },
            value: "base64url-signature".to_string(),
        },
    ],
};
```

Le modèle de signature est volontairement déclaratif. Le crate `epc-core` ne
vérifie pas les signatures lui-même ; il fournit seulement les structures et les
constantes nécessaires aux couches de validation ou de signature.

## Choisir le bon helper

Pour vérifier un identifiant EPC, utiliser `is_valid_card_id`.

Pour vérifier qu'un chemin est sûr avant de l'accepter depuis une archive,
utiliser `is_safe_core_path`.

Pour connaître la limite de taille d'un fichier attendu, utiliser
`expected_file_size_limit`.

Pour savoir si un fichier est obligatoire, utiliser `is_expected_core_file`.

Pour savoir si un fichier régulier est autorisé, utiliser
`is_allowed_regular_file`.

Pour savoir si un fichier doit être listé dans `proof/hashes.json`, utiliser
`is_expected_hashed_core_file`.

Pour construire ou lire les documents JSON du format EPC, utiliser les modèles
`Manifest`, `Hashes` et `SignatureProof`.
