# Guide d'utilisation de `epc-validate`

Le crate `epc-validate` contient le validateur de référence du profil EPC 1.0
`core-format`. Il produit un rapport structuré, sérialisable en JSON, utilisable
par une CLI, des tests de conformité, une application cliente ou un binding SDK.

Il sait valider deux formes d'un EPC :

- un dossier unpacked, avec `validate_core_directory` ;
- une archive `.epc`, avec `validate_epc_file`.

Le validateur vérifie notamment :

- la structure du conteneur ;
- les chemins autorisés ;
- les limites de ressources ;
- `manifest.json` ;
- `text/message.md` ;
- les images JXL lorsque la feature `jxl` est active ;
- `proof/hashes.json` ;
- `proof/signature.json` si présent.

## Générer la documentation Rust

La documentation publique se génère avec :

```bash
cargo doc -p epc-validate --no-deps
```

Pour inclure les helpers internes du validateur :

```bash
cargo doc -p epc-validate --no-deps --document-private-items
```

## Valider une archive `.epc`

Pour valider un fichier EPC final :

```rust
use epc_validate::validate_epc_file;

let report = validate_epc_file("dist/postcard.epc");

if report.is_valid() {
    println!("EPC valide");
} else {
    println!("EPC invalide");
}
```

`validate_epc_file` lit d'abord les métadonnées ZIP, vérifie les limites du
conteneur, extrait les fichiers attendus dans un dossier temporaire borné, puis
délègue les validations de contenu au validateur de dossier.

Cette fonction est l'entrée recommandée pour une app qui reçoit un fichier
`.epc` complet.

## Valider un dossier unpacked

Pour valider un dossier déjà décompressé :

```rust
use epc_validate::validate_core_directory;

let report = validate_core_directory("drafts/postcard-001");

for issue in &report.issues {
    println!("{}: {}", issue.code, issue.detail);
}
```

Le dossier doit avoir la même structure qu'une archive EPC à sa racine :

```text
manifest.json
media/cover.{jpg|jpeg|png|webp|jxl}
media/thumbnail.jxl
text/message.md
proof/hashes.json
proof/signature.json
```

`proof/signature.json` est optionnel. Les autres fichiers sont requis par le
profil `core-format`.

Cette entrée est pratique pour les workflows d'édition, les tests, et le crate
`epc-pack` avant assemblage ZIP.

## Utiliser `CoreDirectoryValidator`

`CoreDirectoryValidator` est la forme objet de `validate_core_directory`.

```rust
use epc_validate::CoreDirectoryValidator;

let validator = CoreDirectoryValidator::new("drafts/postcard-001");
let report = validator.validate();
```

Cette forme est utile si l'on veut conserver explicitement un validateur associé
à un dossier. Pour la majorité des cas, `validate_core_directory` suffit.

## Lire un `ValidationReport`

Toutes les validations retournent un `ValidationReport`.

```rust
use epc_validate::validate_epc_file;

let report = validate_epc_file("dist/postcard.epc");

println!("valid: {}", report.valid);
println!("profile: {}", report.profile);
println!("epc version: {}", report.epc_version);
println!("manifest status: {:?}", report.manifest.status);
println!("errors: {}", report.summary.error);
println!("warnings: {}", report.summary.warning);
```

Les champs principaux sont :

- `valid` : résultat global ;
- `profile` : profil EPC détecté ou attendu ;
- `epc_version` : version EPC détectée ou attendue ;
- `summary` : nombre d'issues par sévérité ;
- `proofs` : état des preuves d'intégrité et de signature ;
- `manifest` : métadonnées du manifest récupérées pendant la validation ;
- `issues` : liste détaillée des problèmes.

Les types publics qui composent ce rapport sont :

- `ValidationSummary`, pour les compteurs par sévérité ;
- `ProofReport`, pour regrouper les preuves positives ;
- `ManifestReport`, pour les métadonnées du manifest comme `status` ;
- `IntegrityProofReport`, pour l'état de `proof/hashes.json` ;
- `SignatureProofReport`, pour l'état de `proof/signature.json` ;
- `VerifiedSignatureReport`, pour chaque signature vérifiée avec succès.

La méthode `is_valid()` retourne le même résultat que `report.valid`.

```rust
if report.is_valid() {
    println!("ok");
}
```

La méthode `has_fatal()` permet de savoir si la validation a rencontré au moins
un problème bloquant de sécurité ou de lisibilité.

```rust
if report.has_fatal() {
    println!("validation interrompue ou partiellement impossible");
}
```

`issues` conserve l'ordre de découverte du validateur. Cet ordre suit les passes
internes de validation et ne doit pas être utilisé comme ordre d'affichage
stable. Pour une interface utilisateur, utiliser `sorted_issues()`.

```rust
for issue in report.sorted_issues() {
    println!("[{:?}] {}", issue.severity, issue.code);
}
```

L'ordre de présentation est : `fatal`, `error`, `warning`, `info`, puis fichier,
JSON Pointer, code, titre, fichier lié et détail.

## Comprendre les sévérités

Les issues utilisent `IssueSeverity`.

```rust
use epc_validate::IssueSeverity;

match IssueSeverity::Error {
    IssueSeverity::Info => println!("information"),
    IssueSeverity::Warning => println!("avertissement"),
    IssueSeverity::Error => println!("erreur"),
    IssueSeverity::Fatal => println!("fatal"),
}
```

Les règles de validité sont simples :

- `info` ne rend pas le rapport invalide ;
- `warning` ne rend pas le rapport invalide ;
- `error` rend le rapport invalide ;
- `fatal` rend le rapport invalide et signale souvent un risque de sécurité ou
  une impossibilité de poursuivre correctement.

Par défaut, le validateur n'émet que les issues actionnables. Les issues
`info` sont réservées au mode verbose.

```rust
use epc_validate::{validate_core_directory_with_options, ValidationOptions};

let report = validate_core_directory_with_options(
    "drafts/postcard-001",
    ValidationOptions::default().verbose(),
);
```

En mode verbose, le validateur émet notamment `EPC_MANIFEST_STATUS`, qui expose
le statut lu dans `manifest.json`. Pour récupérer cette valeur dans un viewer,
préférer `report.manifest.status` à une recherche dans `issues`.

## Lire une issue

Chaque problème est décrit par `ValidationIssue`.

```rust
for issue in &report.issues {
    println!("[{:?}] {}", issue.severity, issue.code);
    println!("{}", issue.title);
    println!("{}", issue.detail);

    if let Some(file) = &issue.file {
        println!("file: {file}");
    }

    if let Some(pointer) = &issue.pointer {
        println!("json pointer: {pointer}");
    }
}
```

Les champs importants sont :

- `severity` : niveau de gravité ;
- `code` : code stable, par exemple `EPC_MANIFEST_INVALID_CARD_ID` ;
- `title` : résumé développeur ;
- `detail` : explication plus précise ;
- `file` : fichier EPC concerné, si applicable ;
- `pointer` : JSON Pointer concerné, si applicable ;
- `related_file` : second fichier lié au problème, si applicable.

Les codes sont pensés pour être stables. Une interface peut donc les utiliser
pour afficher une aide localisée ou une action de correction.

## Créer une issue manuellement

Les types de rapport peuvent aussi servir dans des tests ou des validations
complémentaires.

```rust
use epc_validate::{IssueSeverity, ValidationIssue};

let issue = ValidationIssue::new(
    IssueSeverity::Warning,
    "APP_CUSTOM_WARNING",
    "Custom warning",
    "Application-specific warning.",
)
.with_file("text/message.md")
.with_pointer("#/content/message")
.with_related_file("manifest.json");
```

Pour ajouter une issue à un rapport :

```rust
use epc_validate::{IssueSeverity, ValidationIssue, ValidationReport};

let mut report = ValidationReport::new("core-format", "1.0");
report.push(ValidationIssue::new(
    IssueSeverity::Info,
    "APP_NOTE",
    "Application note",
    "Extra validation note.",
));
```

`push` met automatiquement à jour `summary` et `valid`.

## Fusionner deux rapports

`ValidationReport::extend` ajoute les issues d'un autre rapport et reprend ses
métadonnées principales.

```rust
use epc_validate::ValidationReport;

let mut combined = ValidationReport::default();
let extra = ValidationReport::new("core-format", "1.0");

combined.extend(extra);
```

Cette méthode est utile si une application ajoute une validation métier après la
validation EPC standard.

## Sérialiser un rapport en JSON

`ValidationReport`, `ValidationIssue` et les sous-rapports dérivent
`Serialize` et `Deserialize`.

```rust
use epc_validate::validate_epc_file;

let report = validate_epc_file("dist/postcard.epc");
let json = serde_json::to_string_pretty(&report)?;
println!("{json}");
```

Le JSON est adapté à une sortie CLI, à des snapshots de tests ou à un retour
d'API.

## Lire le rapport d'intégrité

`report.proofs.integrity` décrit l'état de `proof/hashes.json`.

```rust
let integrity = &report.proofs.integrity;

println!("present: {}", integrity.present);
println!("checked: {}", integrity.checked);
println!("valid: {}", integrity.valid);

if let Some(algorithm) = &integrity.hash_algorithm {
    println!("hash algorithm: {algorithm}");
}

if let Some(core_digest) = &integrity.core_digest {
    println!("core digest: {core_digest}");
}
```

Les champs ont le sens suivant :

- `present` : le fichier `proof/hashes.json` existe ;
- `checked` : les vérifications de digest ont été tentées ;
- `valid` : la preuve d'intégrité est valide ;
- `hash_algorithm` : algorithme déclaré, attendu à `sha-256` ;
- `core_digest` : digest global vérifié, si disponible.

Une preuve d'intégrité invalide rend l'EPC invalide.

## Lire le rapport de signature

`report.proofs.signature` décrit l'état de `proof/signature.json` quand il est
présent.

```rust
let signature = &report.proofs.signature;

println!("present: {}", signature.present);
println!("checked: {}", signature.checked);
println!("valid: {}", signature.valid);
println!("policy satisfied: {}", signature.policy_satisfied);

for verified in &signature.verified_signatures {
    println!("{} {}", verified.algorithm, verified.key_id);
}
```

Les champs principaux sont :

- `present` : le fichier `proof/signature.json` existe ;
- `checked` : les vérifications de signature ont été tentées ;
- `valid` : la preuve de signature est valide ;
- `policy_satisfied` : la politique déclarée est satisfaite ;
- `policy_mode` : mode de politique, par exemple `all` ou `any` ;
- `signer_display_name` : nom affiché du signataire ;
- `signer_role` : rôle déclaré du signataire ;
- `signed_at` : date déclarée de signature ;
- `verified_signatures` : signatures vérifiées avec succès.

Une signature absente n'est pas une erreur, car `proof/signature.json` est
optionnel dans `core-format`. En revanche, une signature présente mais invalide
rend l'EPC invalide.

## Validation ZIP

`validate_epc_file` vérifie les règles du conteneur avant de valider le contenu.

Le validateur contrôle notamment :

- la taille maximale de l'archive ;
- l'absence de ZIP64 ;
- le nombre maximal d'entrées ZIP ;
- les chemins dangereux ;
- les chemins dupliqués après normalisation ;
- l'absence de symlinks ;
- les méthodes de compression autorisées ;
- le ratio de compression ;
- la taille décompressée totale ;
- la présence uniquement des fichiers autorisés.

Les méthodes de compression acceptées sont `stored` et `deflated`.

## Validation du dossier unpacked

`validate_core_directory` parcourt les fichiers réguliers et les dossiers depuis
la racine fournie.

Le validateur contrôle notamment :

- les fichiers obligatoires ;
- les fichiers inattendus ;
- les dossiers inattendus ;
- les symlinks ;
- les chemins dangereux ;
- les tailles par fichier ;
- la taille totale ;
- les fichiers de contenu vides, qui produisent des warnings.

Une cover vide supportée, `media/thumbnail.jxl` et `text/message.md`
produisent des warnings, pas des erreurs, lorsque leur présence est autrement
conforme.

## Validation du manifest

Le validateur lit `manifest.json` et vérifie notamment :

- `epc_version = "1.0"` ;
- `profile = "core-format"` ;
- `type = "postcard"` ;
- `id` au format `escale:<ULID>` ;
- `status` parmi `draft`, `issued` ou `sealed` ;
- la cohérence entre `status` et `sealed_at` ;
- `created_local_time.time_zone` non vide et sûr ;
- `created_local_time.utc_offset` au format `+HH:MM` ou `-HH:MM` ;
- `author.display_name` non vide ;
- les chemins et MIME types de `cover`, `thumbnail` et `message` ;
- le profil Markdown `epc-markdown-core` en version `1.0`.

Les erreurs de manifest incluent généralement un `pointer` JSON pour aider une
interface ou une CLI à localiser le champ concerné.

Pour le cycle de vie :

- `draft` et `issued` doivent garder `sealed_at` vide ;
- `sealed` doit renseigner `sealed_at` ;
- les anciens manifests sans champ `status` restent lisibles : le validateur
  les interprète comme `sealed` si `sealed_at` est renseigné, sinon comme
  `draft`.

## Validation Markdown

Le message Markdown est vérifié selon les limites du profil core.

Le validateur contrôle notamment :

- la taille maximale du fichier ;
- la longueur maximale d'une ligne ;
- le nombre maximal de liens ;
- le profil déclaré dans le manifest.

Le profil Markdown est volontairement minimal dans cette phase du SDK.

## Validation JXL

Quand la feature `jxl` est active, `epc-validate` délègue la validation des
images à `epc-image`.

Les fichiers concernés sont :

- `media/cover.jxl`, uniquement lorsque la cover déclarée est JPEG XL ;
- `media/thumbnail.jxl`.

Le validateur vérifie alors la signature et la décodabilité du JPEG XL, ainsi
que les limites de dimensions et de pixels du profil EPC.

Si la feature `jxl` est désactivée, cette validation d'image approfondie n'est
pas compilée.

## Validation des hashes

`proof/hashes.json` doit décrire les fichiers immuables du coeur EPC :

- `manifest.json` avec transform `jcs` ;
- la cover déclarée par `manifest.content.cover.path` avec transform `identity` ;
- `media/thumbnail.jxl` avec transform `identity` ;
- `text/message.md` avec transform `identity`.

Le validateur recalcule les digests, vérifie l'algorithme `sha-256`, puis
recalcule `core_digest` avec le domaine `EPC-CORE-V1\n`.

## Validation des signatures

Si `proof/signature.json` est présent, le validateur vérifie que la signature
est cohérente avec :

- l'identifiant du manifest ;
- la version EPC ;
- le `core_digest` ;
- l'algorithme de hash ;
- la politique de signature ;
- les clés requises ;
- les signatures Ed25519.

La signature est vérifiée sur :

```text
UTF8("EPC-SIGNATURE-V1\n") || JCS(payload)
```

Les signatures valides sont listées dans
`report.proofs.signature.verified_signatures`.

## Choisir la bonne fonction

Pour valider une archive finale reçue par une app ou une CLI, utiliser
`validate_epc_file`.

Pour valider un dossier de travail avant packing, utiliser
`validate_core_directory`.

Pour garder une instance explicite associée à une racine, utiliser
`CoreDirectoryValidator::new(...).validate()`.

Pour inspecter le résultat global, utiliser `report.is_valid()`,
`report.summary` et `report.issues`.

Pour produire une sortie machine-readable, sérialiser `ValidationReport` avec
`serde_json`.
