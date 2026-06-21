use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        None | Some("--version") | Some("-V") => {
            println!("escale-epc {}", epc_core::EPC_VERSION_1_0);
            ExitCode::SUCCESS
        }
        Some("validate-dir") => {
            let Some(path) = args.next() else {
                eprintln!("usage: escale-epc validate-dir <unpacked-capsule-dir>");
                return ExitCode::from(2);
            };
            if args.next().is_some() {
                eprintln!("usage: escale-epc validate-dir <unpacked-capsule-dir>");
                return ExitCode::from(2);
            }

            let report = epc_validate::validate_core_directory(PathBuf::from(path));
            match serde_json::to_string_pretty(&report) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    eprintln!("failed to serialize validation report: {error}");
                    return ExitCode::from(2);
                }
            }

            if report.is_valid() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some("validate") => {
            let Some(path) = args.next() else {
                eprintln!("usage: escale-epc validate <capsule.epc>");
                return ExitCode::from(2);
            };
            if args.next().is_some() {
                eprintln!("usage: escale-epc validate <capsule.epc>");
                return ExitCode::from(2);
            }

            let report = epc_validate::validate_epc_file(PathBuf::from(path));
            match serde_json::to_string_pretty(&report) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    eprintln!("failed to serialize validation report: {error}");
                    return ExitCode::from(2);
                }
            }

            if report.is_valid() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some("create") => {
            let mut force = false;
            let mut values = Vec::new();
            for arg in args {
                if arg == "--force" {
                    force = true;
                } else {
                    values.push(arg);
                }
            }
            if values.len() != 2 {
                eprintln!("usage: escale-epc create [--force] <draft-dir> <author-display-name>");
                return ExitCode::from(2);
            }

            let request =
                epc_pack::CreateDraftRequest::new(&values[0], &values[1]).with_force(force);
            match epc_pack::create_draft_directory(request) {
                Ok(draft_dir) => {
                    println!("{}", draft_dir.display());
                    ExitCode::SUCCESS
                }
                Err(epc_pack::PackError::Io(error)) => {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        eprintln!(
                            "failed to create draft: manifest.json already exists; refusing to overwrite an existing draft"
                        );
                    } else {
                        eprintln!("failed to create draft: {error}");
                    }
                    ExitCode::from(2)
                }
                Err(error) => {
                    eprintln!("failed to create draft: {error:?}");
                    ExitCode::from(2)
                }
            }
        }
        Some("pack") => {
            let mut signing_key = None;
            let mut force_signature = false;
            let mut values = Vec::new();
            while let Some(arg) = args.next() {
                if arg == "--sign" {
                    let Some(value) = args.next() else {
                        eprintln!("usage: escale-epc pack [--sign <ssh-ed25519-key>] [--force] <source-dir> [output-dir]");
                        return ExitCode::from(2);
                    };
                    signing_key = Some(PathBuf::from(value));
                } else if arg == "--force" {
                    force_signature = true;
                } else {
                    values.push(arg);
                }
            }
            if values.is_empty() || values.len() > 2 {
                eprintln!(
                    "usage: escale-epc pack [--sign <ssh-ed25519-key>] [--force] <source-dir> [output-dir]"
                );
                return ExitCode::from(2);
            }
            if force_signature && signing_key.is_none() {
                eprintln!("--force can only be used with pack --sign");
                return ExitCode::from(2);
            }
            let source_dir = values[0].clone();
            let output_dir = values
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| default_pack_output_dir(&source_dir));

            let result = match signing_key {
                Some(signing_key) => epc_pack::pack_core_format_to_directory_signed(
                    source_dir,
                    output_dir,
                    signing_key,
                    force_signature,
                ),
                None => epc_pack::pack_core_format_to_directory(source_dir, output_dir),
            };

            match result {
                Ok(output_file) => {
                    println!("{}", output_file.display());
                    ExitCode::SUCCESS
                }
                Err(epc_pack::PackError::InvalidSource(report)) => {
                    match serde_json::to_string_pretty(&report) {
                        Ok(json) => eprintln!("{json}"),
                        Err(error) => eprintln!("failed to serialize validation report: {error}"),
                    }
                    ExitCode::FAILURE
                }
                Err(epc_pack::PackError::Io(error))
                    if error.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    eprintln!(
                        "failed to pack capsule: output file already exists; refusing to overwrite"
                    );
                    ExitCode::from(2)
                }
                Err(error) => {
                    eprintln!("failed to pack capsule: {error:?}");
                    ExitCode::from(2)
                }
            }
        }
        Some("sign") => {
            let mut signing_key = None;
            let mut force = false;
            let mut values = Vec::new();
            while let Some(arg) = args.next() {
                if arg == "--ssh-key" {
                    let Some(value) = args.next() else {
                        eprintln!(
                            "usage: escale-epc sign [--force] --ssh-key <ssh-ed25519-key> <source-dir>"
                        );
                        return ExitCode::from(2);
                    };
                    signing_key = Some(PathBuf::from(value));
                } else if arg == "--force" {
                    force = true;
                } else {
                    values.push(arg);
                }
            }
            if values.len() != 1 || signing_key.is_none() {
                eprintln!(
                    "usage: escale-epc sign [--force] --ssh-key <ssh-ed25519-key> <source-dir>"
                );
                return ExitCode::from(2);
            }

            match epc_pack::sign_core_format_directory_with_ssh_key(
                &values[0],
                signing_key.expect("signing key is present"),
                force,
            ) {
                Ok(signature_file) => {
                    println!("{}", signature_file.display());
                    ExitCode::SUCCESS
                }
                Err(epc_pack::PackError::InvalidSource(report)) => {
                    match serde_json::to_string_pretty(&report) {
                        Ok(json) => eprintln!("{json}"),
                        Err(error) => eprintln!("failed to serialize validation report: {error}"),
                    }
                    ExitCode::FAILURE
                }
                Err(epc_pack::PackError::Io(error))
                    if error.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    eprintln!(
                        "failed to sign capsule: signature proof already exists; use --force to replace it"
                    );
                    ExitCode::from(2)
                }
                Err(error) => {
                    eprintln!("failed to sign capsule: {error:?}");
                    ExitCode::from(2)
                }
            }
        }
        Some("generate-test-vectors") => {
            let Some(output_dir) = args.next() else {
                eprintln!("usage: escale-epc generate-test-vectors <test-vectors/core-format-dir>");
                return ExitCode::from(2);
            };
            if args.next().is_some() {
                eprintln!("usage: escale-epc generate-test-vectors <test-vectors/core-format-dir>");
                return ExitCode::from(2);
            }

            match epc_pack::generate_core_format_test_vectors(PathBuf::from(output_dir)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("failed to generate test vectors: {error:?}");
                    ExitCode::from(2)
                }
            }
        }
        Some("--help") | Some("-h") | Some("help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown command: {command}");
            print_help();
            ExitCode::from(2)
        }
    }
}

fn print_help() {
    println!("escale-epc {}", epc_core::EPC_VERSION_1_0);
    println!();
    println!("commands:");
    println!("  create [--force] <draft-dir> <author>");
    println!("                                      Create or reset an unpacked EPC draft");
    println!("  validate <capsule.epc>               Validate a core-format EPC archive");
    println!("  validate-dir <unpacked-capsule-dir>  Validate an unpacked core-format capsule");
    println!("  pack [--sign <ssh-key>] [--force] <source-dir> [output-dir]");
    println!("                                      Pack, optionally signing with an SSH key");
    println!("  sign [--force] --ssh-key <ssh-key> <source-dir>");
    println!("                                      Write proof/signature.json with Ed25519");
    println!("  generate-test-vectors <dir>          Generate core-format conformance vectors");
    println!("  --version                            Print the EPC version");
}

fn default_pack_output_dir(source_dir: &str) -> PathBuf {
    PathBuf::from(source_dir)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
