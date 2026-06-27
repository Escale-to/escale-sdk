# Guide d'utilisation de `epc-pack`

Le crate `epc-pack` contient les primitives d'écriture du format EPC
`core-format`. Il sert à créer un dossier de brouillon, régénérer
`proof/hashes.json`, signer éventuellement le contenu, puis assembler une
archive `.epc` valide.

Il s'appuie sur :

- `epc-core` pour les chemins, constantes et modèles JSON ;
- `epc-validate` pour refuser un dossier invalide avant l'écriture finale ;
- `zip` pour produire l'archive `.epc` ;
- `ed25519-dalek` pour les signatures Ed25519.

## Générer la documentation Rust

La documentation publique se génère avec :

```bash
cargo doc -p epc-pack --no-deps
```

Pour inclure les helpers internes du pipeline de packing :

```bash
cargo doc -p epc-pack --no-deps --document-private-items
```

## Cycle recommandé

Le flux normal de création d'un EPC est :

1. Créer un dossier de brouillon avec `create_draft_directory`.
2. Copier l'image source acceptée sans modification dans `media/cover.*`.
3. Dériver `media/thumbnail.jxl` depuis cette couverture avec la règle EPC.
4. Ajouter ou modifier `text/message.md`.
5. Signer le dossier avant tout scellement final.
6. Packer le dossier en `.epc`.

La dérivation du thumbnail est fournie par `epc-image` : elle redimensionne la
couverture dans une boîte de 256x256 pixels, conserve le ratio, ne recadre pas
et n'agrandit pas l'image.

`epc-pack` régénère `proof/hashes.json` lors de la signature et du packing.
Il ne faut donc pas écrire ce fichier à la main dans une application cliente.

## Cycle de vie et noms de fichiers

Le manifest porte un champ `status` qui décrit l'état d'écriture de la carte :

| Statut | Forme | Nom | Responsable de l'écriture |
| --- | --- | --- | --- |
| `draft` | dossier unpacked | `escale-<TIME6>-<RAND2>/` | app, CLI ou SDK public |
| `issued` | archive `.epc` | `escale-<ID10>.epc` | SDK public, pour remise à l'infrastructure de voyage |
| `sealed` | archive `.epc` | `<TIME6>-<ID10>.epc` | SDK public pour une carte qui ne voyage pas, ou infrastructure de voyage après délivrance |

Un brouillon n'est pas une archive `.epc` publique : c'est un dossier unpacked.
Le format `escale-<ID10>.epc` est réservé à une archive `issued`.

Une source `issued` ou `sealed` est verrouillée par le SDK public. Les fonctions
qui écrivent dans le dossier source, comme `create_draft_directory`,
`refresh_manifest_image_metadata` ou `sign_core_format_directory`, retournent
`PackError::SealedSource`. Le même verrou empêche le SDK public de transformer
une source `issued` en `sealed` : cette finalisation appartient à
l'infrastructure de voyage.

## Créer un dossier de brouillon

`CreateDraftRequest` décrit la création d'un dossier EPC unpacked.

```rust
use epc_pack::{create_draft_directory, CreateDraftRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let request = CreateDraftRequest::new("drafts/postcard-001", "Alice");
    let root = create_draft_directory(request)?;

    println!("draft created at {}", root.display());
    Ok(())
}
```

Cette opération crée les dossiers `media/` et `text/`, écrit un
`manifest.json`, crée `text/message.md` s'il n'existe pas déjà, et supprime les
preuves générées obsolètes `proof/hashes.json` et `proof/signature.json`.

Elle ne crée pas automatiquement les images. L'application doit ensuite écrire :

- une image principale supportée : `media/cover.jpg`, `media/cover.jpeg`,
  `media/cover.png`, `media/cover.webp` ou `media/cover.jxl`
- `media/thumbnail.jxl`
- `text/message.md`

Le manifest reçoit automatiquement :

- un identifiant `escale:<ULID>` ;
- `epc_version = "1.0"` ;
- `profile = "core-format"` ;
- `type = "postcard"` ;
- `created_at` en UTC ;
- `status = "draft"` ;
- `sealed_at` vide ;
- les chemins de contenu canoniques.

## Fournir le contexte local de création

Par défaut, `CreateDraftRequest::new` appelle
`detect_device_created_local_time`. Pour une application mobile ou desktop, il
est préférable de fournir directement les valeurs lues depuis l'OS au moment de
la création.

```rust
use epc_core::CreatedLocalTime;
use epc_pack::{create_draft_directory, CreateDraftRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let local_time = CreatedLocalTime {
        time_zone: "Europe/Paris".to_string(),
        utc_offset: "+02:00".to_string(),
    };

    let request = CreateDraftRequest::new("drafts/postcard-001", "Alice")
        .with_created_local_time(local_time);

    create_draft_directory(request)?;
    Ok(())
}
```

`with_force(true)` autorise le remplacement d'un `manifest.json` existant :

```rust
use epc_pack::{create_draft_directory, CreateDraftRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let request = CreateDraftRequest::new("drafts/postcard-001", "Alice")
        .with_force(true);

    create_draft_directory(request)?;
    Ok(())
}
```

Sans `force`, la création échoue si `manifest.json` existe déjà.

## Obtenir l'identifiant court d'un brouillon

`draft_filename_from_directory` lit `manifest.json` et produit le nom historique
`escale-<ID10>.epc`. Ce format est désormais réservé aux fichiers EPC `issued` ;
un brouillon public reste un dossier unpacked, pas une archive `.epc`.

```rust
use epc_pack::draft_filename_from_directory;

fn main() -> Result<(), epc_pack::PackError> {
    let filename = draft_filename_from_directory("drafts/postcard-001")?;
    println!("{filename}");
    Ok(())
}
```

Ce helper reste disponible pour compatibilité, mais une nouvelle interface ne
devrait pas l'utiliser pour nommer une archive de brouillon. Le nom final d'un
EPC scellé est produit par `pack_core_format_to_directory`.

## Packer vers un chemin explicite

`PackRequest` décrit le packing d'un dossier source vers un fichier `.epc`
précis.

```rust
use epc_pack::{pack_core_format, PackRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let request = PackRequest::new(
        "drafts/postcard-001",
        "dist/postcard-001.epc",
    );

    pack_core_format(request)?;
    Ok(())
}
```

`pack_core_format` copie le dossier source dans un staging temporaire,
prépare le statut demandé par `PackRequest::mode`, régénère `proof/hashes.json`,
valide le staging avec `epc-validate`, puis écrit le ZIP final.

Le dossier source doit contenir au minimum :

- `manifest.json`
- une image principale supportée : `media/cover.jpg`, `media/cover.jpeg`,
  `media/cover.png`, `media/cover.webp` ou `media/cover.jxl`
- `media/thumbnail.jxl`
- `text/message.md`

Pour produire un staging `sealed`, `proof/signature.json` doit exister et être
valide. S'il existe, il est copié dans le staging et inclus dans l'archive.

Attention : cette fonction écrit le fichier de sortie avec `create_new`. Elle
échoue si le fichier `.epc` existe déjà.

`PackRequest::new` choisit `PackMode::Sealed` par défaut. Pour produire une
archive de remise au voyage, utiliser `with_mode(PackMode::Issued)`.

## Packer dans un dossier de sortie

`pack_core_format_to_directory` est l'entrée la plus pratique pour produire un
nom de fichier EPC canonique.

```rust
use epc_pack::pack_core_format_to_directory;

fn main() -> Result<(), epc_pack::PackError> {
    let output_file = pack_core_format_to_directory(
        "drafts/postcard-001",
        "dist",
    )?;

    println!("written {}", output_file.display());
    Ok(())
}
```

Cette fonction met le manifest en statut `sealed` si nécessaire, mais le
validateur refuse le résultat si aucune signature valide n'est présente. Pour un
brouillon non signé, utiliser `pack_core_format_to_directory_signed`. Si
`sealed_at` est vide, le timestamp scellé est écrit dans le `manifest.json` du
dossier source, puis utilisé pour calculer le nom final.

Le nom généré suit la forme :

```text
<TIME6>-<ID10>.epc
```

`TIME6` est dérivé de `sealed_at`. `ID10` correspond aux 10 derniers caractères
de l'identifiant EPC.

Si le manifest est déjà scellé et signé, son `sealed_at` existant est conservé.
Cela permet de repacker le même contenu avec un nom stable.

Pour préparer une carte destinée à l'infrastructure de voyage, utiliser
`pack_core_format_to_directory_issued`. Le manifest passe alors en statut
`issued`, `sealed_at` reste vide, et le fichier produit suit la forme
`escale-<ID10>.epc`. Une source `issued` est verrouillée côté SDK public et ne
doit être finalisée que par l'infrastructure de voyage.

## Signer avec un seed Ed25519

`SignRequest` permet de créer `proof/signature.json` à partir d'un seed Ed25519
encodé en Base64URL sans padding.

```rust
use epc_pack::{sign_core_format_directory, SignRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let request = SignRequest::new(
        "drafts/postcard-001",
        "base64url-ed25519-seed",
        "Alice",
    );

    let signature_path = sign_core_format_directory(request)?;
    println!("signature written at {}", signature_path.display());
    Ok(())
}
```

La signature couvre :

```text
UTF8("EPC-SIGNATURE-V1\n") || JCS(payload)
```

Avant d'écrire la signature, le dossier est scellé si nécessaire,
`proof/hashes.json` est régénéré, puis le dossier est validé. La signature
refuse les sources `issued` et `sealed`.

Par défaut, le rôle du signataire est `author`. Pour le changer :

```rust
use epc_pack::{sign_core_format_directory, SignRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let request = SignRequest::new(
        "drafts/postcard-001",
        "base64url-ed25519-seed",
        "Alice",
    )
    .with_signer_role("publisher");

    sign_core_format_directory(request)?;
    Ok(())
}
```

Si `proof/signature.json` existe déjà, la signature échoue par défaut. Utiliser
`with_force(true)` pour la remplacer :

```rust
use epc_pack::{sign_core_format_directory, SignRequest};

fn main() -> Result<(), epc_pack::PackError> {
    let request = SignRequest::new(
        "drafts/postcard-001",
        "base64url-ed25519-seed",
        "Alice",
    )
    .with_force(true);

    sign_core_format_directory(request)?;
    Ok(())
}
```

## Signer avec une clé OpenSSH Ed25519

`sign_core_format_directory_with_ssh_key` lit une clé privée OpenSSH Ed25519 non
chiffrée, extrait son seed, puis écrit `proof/signature.json`.

```rust
use epc_pack::sign_core_format_directory_with_ssh_key;

fn main() -> Result<(), epc_pack::PackError> {
    sign_core_format_directory_with_ssh_key(
        "drafts/postcard-001",
        "keys/author_ed25519",
        false,
    )?;

    Ok(())
}
```

Le nom affiché du signataire est repris depuis `manifest.json`
`author.display_name`.

Les clés OpenSSH chiffrées ne sont pas supportées par ce helper bas niveau.

## Signer puis packer

`pack_core_format_to_directory_signed` combine signature par clé OpenSSH et
packing dans un dossier de sortie.

```rust
use epc_pack::pack_core_format_to_directory_signed;

fn main() -> Result<(), epc_pack::PackError> {
    let output_file = pack_core_format_to_directory_signed(
        "drafts/postcard-001",
        "dist",
        "keys/author_ed25519",
        false,
    )?;

    println!("written {}", output_file.display());
    Ok(())
}
```

Si `force_signature` vaut `true`, la signature est régénérée. Si
`force_signature` vaut `false`, la signature existante est conservée quand
`proof/signature.json` existe déjà.

## Détecter le contexte local de création

`detect_device_created_local_time` retourne une valeur `CreatedLocalTime` basée
sur le contexte du processus.

```rust
use epc_pack::detect_device_created_local_time;

let local_time = detect_device_created_local_time();
println!("{} {}", local_time.time_zone, local_time.utc_offset);
```

La fonction cherche d'abord :

- `ESCALE_DEVICE_TIME_ZONE` pour le fuseau horaire ;
- `TZ` si `ESCALE_DEVICE_TIME_ZONE` est absent ;
- `/etc/localtime` si disponible ;
- `Etc/UTC` en fallback.

Pour l'offset UTC, elle cherche :

- `ESCALE_DEVICE_UTC_OFFSET` ;
- la commande système `date +%z` ;
- `+00:00` en fallback.

Pour les apps iOS, Android, macOS ou Windows, il vaut mieux transmettre les
valeurs exactes obtenues via les APIs natives plutôt que dépendre de cette
détection best-effort.

## Générer les vecteurs de conformité

`generate_core_format_test_vectors` produit les archives de référence du profil
`core-format`.

```rust
use epc_pack::generate_core_format_test_vectors;

fn main() -> Result<(), epc_pack::PackError> {
    generate_core_format_test_vectors("test-vectors/core-format")?;
    Ok(())
}
```

La fonction écrit :

- des archives valides dans `valid/` ;
- des archives avec warnings dans `warning/` ;
- des archives invalides dans `invalid/` ;
- les rapports attendus dans `reports/warning/` et `reports/invalid/`.

Cette fonction est surtout destinée aux tests de conformité du SDK et aux autres
implémentations du format.

## Gérer les erreurs

Toutes les fonctions publiques de packing retournent `PackError`.

Les cas importants sont :

- `InvalidSource` : le dossier staging a échoué à la validation
  `epc-validate` ;
- `InvalidFilenameMetadata` : le manifest ne permet pas de produire un nom EPC
  canonique ;
- `InvalidSignatureMetadata` : la demande de signature, le seed ou la clé sont
  invalides ;
- `SealedSource` : l'opération écrirait dans une source déjà `issued` ou
  `sealed` ;
- `Io` : erreur filesystem ou processus ;
- `Json` : erreur de sérialisation ou désérialisation JSON ;
- `Zip` : erreur d'écriture ZIP.

Pour diagnostiquer `InvalidSource`, inspecter le `ValidationReport` contenu dans
l'erreur.

## Choisir la bonne fonction

Pour initialiser un dossier de brouillon, utiliser `create_draft_directory`.

Pour obtenir un nom de fichier temporaire basé sur l'identifiant, utiliser
`draft_filename_from_directory`.

Pour packer vers un fichier précis, utiliser `pack_core_format` avec
`PackRequest`. Un packing `sealed` exige une signature valide.

Pour packer vers un dossier avec nom canonique scellé depuis une source déjà
signée, utiliser `pack_core_format_to_directory`.

Pour produire une archive `issued` destinée à l'infrastructure de voyage,
utiliser `pack_core_format_to_directory_issued` ou `PackRequest::with_mode` avec
`PackMode::Issued`.

Pour signer avec un seed Base64URL, utiliser `sign_core_format_directory`.

Pour signer avec une clé OpenSSH Ed25519 non chiffrée, utiliser
`sign_core_format_directory_with_ssh_key`.

Pour signer puis packer un brouillon dans un seul appel, utiliser
`pack_core_format_to_directory_signed`.

Pour générer les archives de conformité, utiliser
`generate_core_format_test_vectors`.

Pour remplir `created_local_time` automatiquement dans un contexte serveur ou
CLI, utiliser `detect_device_created_local_time`.
