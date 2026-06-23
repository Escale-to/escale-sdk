use std::env;
use std::fs;
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
        Some("image") => handle_image(args.collect()),
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
    println!("  image <command> ...                  Inspect, preview, encode, or prepare JXL");
    println!("  generate-test-vectors <dir>          Generate core-format conformance vectors");
    println!("  --version                            Print the EPC version");
}

fn default_pack_output_dir(source_dir: &str) -> PathBuf {
    PathBuf::from(source_dir)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn handle_image(args: Vec<String>) -> ExitCode {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("info") => image_info(args.collect()),
        Some("validate") => image_validate(args.collect()),
        Some("preview") => image_preview(args.collect()),
        Some("encode") => image_encode(args.collect()),
        Some("prepare") => image_prepare(args.collect()),
        Some("--help") | Some("-h") | Some("help") => {
            print_image_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown image command: {command}");
            print_image_help();
            ExitCode::from(2)
        }
        None => {
            print_image_help();
            ExitCode::from(2)
        }
    }
}

fn print_image_help() {
    eprintln!("image commands:");
    eprintln!("  image info <image.jxl> [--kind cover|thumbnail]");
    eprintln!("  image validate <image.jxl> --kind cover|thumbnail");
    eprintln!("  image preview <image.jxl> --out <preview.png> [--max <px>] [--kind cover|thumbnail] [--force]");
    eprintln!("  image encode <input.jpg|png> <output.jxl> --kind cover|thumbnail [--cjxl <path>] [--distance <n>|--quality <n>] [--effort <n>] [--force]");
    eprintln!("  image prepare <input.jpg|png> <draft-dir> [--cjxl <path>] [--distance <n>|--quality <n>] [--effort <n>] [--force]");
}

fn image_info(args: Vec<String>) -> ExitCode {
    let mut path = None;
    let mut kind = epc_image::EpcImageKind::Cover;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--kind" {
            let Some(value) = iter.next() else {
                eprintln!("--kind requires cover or thumbnail");
                return ExitCode::from(2);
            };
            kind = match parse_image_kind(&value) {
                Some(kind) => kind,
                None => return ExitCode::from(2),
            };
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            eprintln!("usage: escale-epc image info <image.jxl> [--kind cover|thumbnail]");
            return ExitCode::from(2);
        }
    }

    let Some(path) = path else {
        eprintln!("usage: escale-epc image info <image.jxl> [--kind cover|thumbnail]");
        return ExitCode::from(2);
    };

    match epc_image::validate_jxl_file(&path, kind) {
        Ok(info) => {
            let size = fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            println!("path: {}", path.display());
            println!("kind: {}", image_kind_name(kind));
            println!("width: {}", info.width);
            println!("height: {}", info.height);
            println!("pixels: {}", info.pixels);
            println!("file_bytes: {size}");
            println!("valid: true");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("invalid JPEG XL: {error:?}");
            ExitCode::FAILURE
        }
    }
}

fn image_validate(args: Vec<String>) -> ExitCode {
    let mut path = None;
    let mut kind = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--kind" {
            let Some(value) = iter.next() else {
                eprintln!("--kind requires cover or thumbnail");
                return ExitCode::from(2);
            };
            kind = parse_image_kind(&value);
            if kind.is_none() {
                return ExitCode::from(2);
            }
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        } else {
            eprintln!("usage: escale-epc image validate <image.jxl> --kind cover|thumbnail");
            return ExitCode::from(2);
        }
    }

    let (Some(path), Some(kind)) = (path, kind) else {
        eprintln!("usage: escale-epc image validate <image.jxl> --kind cover|thumbnail");
        return ExitCode::from(2);
    };

    match epc_image::validate_jxl_file(&path, kind) {
        Ok(info) => {
            println!(
                "valid {} as {} ({}x{}, {} pixels)",
                path.display(),
                image_kind_name(kind),
                info.width,
                info.height,
                info.pixels
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("invalid {}: {error:?}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn image_preview(args: Vec<String>) -> ExitCode {
    let mut input = None;
    let mut output = None;
    let mut max = 1024_u32;
    let mut force = false;
    let mut kind = epc_image::EpcImageKind::Cover;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => {
                let Some(value) = iter.next() else {
                    eprintln!("--out requires a path");
                    return ExitCode::from(2);
                };
                output = Some(PathBuf::from(value));
            }
            "--max" => {
                let Some(value) = iter.next() else {
                    eprintln!("--max requires a positive integer");
                    return ExitCode::from(2);
                };
                max = match value.parse::<u32>() {
                    Ok(value) if value > 0 => value,
                    _ => {
                        eprintln!("--max requires a positive integer");
                        return ExitCode::from(2);
                    }
                };
            }
            "--kind" => {
                let Some(value) = iter.next() else {
                    eprintln!("--kind requires cover or thumbnail");
                    return ExitCode::from(2);
                };
                kind = match parse_image_kind(&value) {
                    Some(kind) => kind,
                    None => return ExitCode::from(2),
                };
            }
            "--force" => force = true,
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("usage: escale-epc image preview <image.jxl> --out <preview.png> [--max <px>] [--kind cover|thumbnail] [--force]");
                return ExitCode::from(2);
            }
        }
    }

    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("usage: escale-epc image preview <image.jxl> --out <preview.png> [--max <px>] [--kind cover|thumbnail] [--force]");
        return ExitCode::from(2);
    };
    if !force && output.exists() {
        eprintln!("output already exists: {}; use --force", output.display());
        return ExitCode::from(2);
    }

    let image = match epc_image::decode_jxl_file_rgba8(
        &input,
        kind,
        epc_image::RenderOptions::fit(max, max),
    ) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("failed to decode preview image: {error:?}");
            return ExitCode::from(2);
        }
    };

    match epc_image::write_rgba_png_file(&image, &output) {
        Ok(()) => {
            println!("{}", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to write preview: {error:?}");
            ExitCode::from(2)
        }
    }
}

fn image_encode(args: Vec<String>) -> ExitCode {
    let mut values = Vec::new();
    let mut kind = None;
    let mut force = false;
    let mut options = epc_image::EncodeOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--kind" => {
                let Some(value) = iter.next() else {
                    eprintln!("--kind requires cover or thumbnail");
                    return ExitCode::from(2);
                };
                kind = parse_image_kind(&value);
                if kind.is_none() {
                    return ExitCode::from(2);
                }
            }
            "--cjxl" => {
                let Some(value) = iter.next() else {
                    eprintln!("--cjxl requires a path");
                    return ExitCode::from(2);
                };
                options = options.with_cjxl_path(value);
            }
            "--distance" => {
                let Some(value) = iter.next() else {
                    eprintln!("--distance requires a number");
                    return ExitCode::from(2);
                };
                let Ok(value) = value.parse::<f32>() else {
                    eprintln!("--distance requires a number");
                    return ExitCode::from(2);
                };
                options = options.with_distance(value);
            }
            "--quality" => {
                let Some(value) = iter.next() else {
                    eprintln!("--quality requires a number");
                    return ExitCode::from(2);
                };
                let Ok(value) = value.parse::<f32>() else {
                    eprintln!("--quality requires a number");
                    return ExitCode::from(2);
                };
                options = options.with_quality(value);
            }
            "--effort" => {
                let Some(value) = iter.next() else {
                    eprintln!("--effort requires an integer");
                    return ExitCode::from(2);
                };
                let Ok(value) = value.parse::<u8>() else {
                    eprintln!("--effort requires an integer");
                    return ExitCode::from(2);
                };
                options = options.with_effort(value);
            }
            "--force" => force = true,
            _ => values.push(arg),
        }
    }

    if values.len() != 2 || kind.is_none() {
        eprintln!("usage: escale-epc image encode <input.jpg|png> <output.jxl> --kind cover|thumbnail [--force]");
        return ExitCode::from(2);
    }
    let input = PathBuf::from(&values[0]);
    let output = PathBuf::from(&values[1]);
    let kind = kind.expect("kind checked");
    if !force && output.exists() {
        eprintln!("output already exists: {}; use --force", output.display());
        return ExitCode::from(2);
    }

    match encode_and_validate_file(&input, &output, kind, &options) {
        Ok(()) => {
            println!("{}", output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn image_prepare(args: Vec<String>) -> ExitCode {
    let mut values = Vec::new();
    let mut force = false;
    let mut options = epc_image::EncodeOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cjxl" => {
                let Some(value) = iter.next() else {
                    eprintln!("--cjxl requires a path");
                    return ExitCode::from(2);
                };
                options = options.with_cjxl_path(value);
            }
            "--distance" => {
                let Some(value) = iter.next() else {
                    eprintln!("--distance requires a number");
                    return ExitCode::from(2);
                };
                let Ok(value) = value.parse::<f32>() else {
                    eprintln!("--distance requires a number");
                    return ExitCode::from(2);
                };
                options = options.with_distance(value);
            }
            "--quality" => {
                let Some(value) = iter.next() else {
                    eprintln!("--quality requires a number");
                    return ExitCode::from(2);
                };
                let Ok(value) = value.parse::<f32>() else {
                    eprintln!("--quality requires a number");
                    return ExitCode::from(2);
                };
                options = options.with_quality(value);
            }
            "--effort" => {
                let Some(value) = iter.next() else {
                    eprintln!("--effort requires an integer");
                    return ExitCode::from(2);
                };
                let Ok(value) = value.parse::<u8>() else {
                    eprintln!("--effort requires an integer");
                    return ExitCode::from(2);
                };
                options = options.with_effort(value);
            }
            "--force" => force = true,
            _ => values.push(arg),
        }
    }

    if values.len() != 2 {
        eprintln!("usage: escale-epc image prepare <input.jpg|png> <draft-dir> [--force]");
        return ExitCode::from(2);
    }

    let input = PathBuf::from(&values[0]);
    let draft = PathBuf::from(&values[1]);
    let media_dir = draft.join("media");
    if let Err(error) = fs::create_dir_all(&media_dir) {
        eprintln!("failed to create media directory: {error}");
        return ExitCode::from(2);
    }
    let cover = draft.join(epc_core::COVER_PATH);
    let thumbnail = draft.join(epc_core::THUMBNAIL_PATH);
    if !force && (cover.exists() || thumbnail.exists()) {
        eprintln!("cover or thumbnail already exists; use --force");
        return ExitCode::from(2);
    }

    if let Err(error) =
        encode_and_validate_file(&input, &cover, epc_image::EpcImageKind::Cover, &options)
    {
        eprintln!("{error}");
        return ExitCode::from(2);
    }

    match epc_image::encode_thumbnail_from_cover_jxl_file(&cover, &thumbnail, &options) {
        Ok(()) => {
            println!("{}", cover.display());
            println!("{}", thumbnail.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "failed to create thumbnail: {}",
                format_thumbnail_error(error)
            );
            ExitCode::from(2)
        }
    }
}

fn encode_and_validate_file(
    input: &std::path::Path,
    output: &std::path::Path,
    kind: epc_image::EpcImageKind,
    options: &epc_image::EncodeOptions,
) -> Result<(), String> {
    epc_image::encode_file_to_jxl_file(input, output, options).map_err(|error| {
        format!(
            "failed to encode {}: {}",
            output.display(),
            format_encode_error(error)
        )
    })?;
    epc_image::validate_jxl_file(output, kind)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "encoded image is invalid as {}: {error:?}",
                image_kind_name(kind)
            )
        })
}

fn format_encode_error(error: epc_image::EncodeError) -> String {
    match error {
        epc_image::EncodeError::CjxlUnavailable { path, source } => {
            format!(
                "cannot start cjxl at {:?}: {source}. Install libjxl tools or pass --cjxl <path>.",
                path
            )
        }
        other => format!("{other:?}"),
    }
}

fn format_thumbnail_error(error: epc_image::ThumbnailError) -> String {
    match error {
        epc_image::ThumbnailError::Display(error) => format!("failed to render cover: {error:?}"),
        epc_image::ThumbnailError::Encode(error) => format_encode_error(error),
        epc_image::ThumbnailError::Validation(error) => {
            format!("encoded thumbnail is invalid: {error:?}")
        }
    }
}

fn parse_image_kind(value: &str) -> Option<epc_image::EpcImageKind> {
    match value {
        "cover" => Some(epc_image::EpcImageKind::Cover),
        "thumbnail" | "thumb" => Some(epc_image::EpcImageKind::Thumbnail),
        _ => {
            eprintln!("invalid image kind: {value}; expected cover or thumbnail");
            None
        }
    }
}

fn image_kind_name(kind: epc_image::EpcImageKind) -> &'static str {
    match kind {
        epc_image::EpcImageKind::Cover => "cover",
        epc_image::EpcImageKind::Thumbnail => "thumbnail",
    }
}
