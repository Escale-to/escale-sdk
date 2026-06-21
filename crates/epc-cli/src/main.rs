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
        Some("pack-dir") => {
            let Some(source_dir) = args.next() else {
                eprintln!("usage: escale-epc pack-dir <source-dir> <output.epc>");
                return ExitCode::from(2);
            };
            let Some(output_file) = args.next() else {
                eprintln!("usage: escale-epc pack-dir <source-dir> <output.epc>");
                return ExitCode::from(2);
            };
            if args.next().is_some() {
                eprintln!("usage: escale-epc pack-dir <source-dir> <output.epc>");
                return ExitCode::from(2);
            }

            let request = epc_pack::PackRequest::new(source_dir, output_file);
            match epc_pack::pack_core_format(request) {
                Ok(()) => ExitCode::SUCCESS,
                Err(epc_pack::PackError::InvalidSource(report)) => {
                    match serde_json::to_string_pretty(&report) {
                        Ok(json) => eprintln!("{json}"),
                        Err(error) => eprintln!("failed to serialize validation report: {error}"),
                    }
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("failed to pack capsule: {error:?}");
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
    println!("  validate <capsule.epc>               Validate a core-format EPC archive");
    println!("  validate-dir <unpacked-capsule-dir>  Validate an unpacked core-format capsule");
    println!("  pack-dir <source-dir> <output.epc>   Pack a source directory as core-format EPC");
    println!("  generate-test-vectors <dir>          Generate core-format conformance vectors");
    println!("  --version                            Print the EPC version");
}
