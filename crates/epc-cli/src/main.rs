use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DRAFT_DIR_RANDOM_COUNTER: AtomicUsize = AtomicUsize::new(0);

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
            let mut options = epc_image::EncodeOptions::default();
            let mut values = Vec::new();
            let mut iter = args;
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--force" => force = true,
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
                    _ => values.push(arg),
                }
            }
            if !(2..=3).contains(&values.len()) {
                eprintln!("usage: escale-epc create [--force] <draft-dir|cover.jpg|cover.jpeg|cover.png|cover.webp|cover.jxl> <author-display-name> [<message>] [--distance <n>|--quality <n>] [--effort <n>]");
                return ExitCode::from(2);
            }

            create_draft(
                &values[0],
                &values[1],
                values.get(2).map(String::as_str),
                force,
                &options,
            )
        }
        Some("pack") => {
            let mut signing_key = None;
            let mut force_signature = false;
            let mut issued = false;
            let mut values = Vec::new();
            while let Some(arg) = args.next() {
                if arg == "--sign" {
                    let Some(value) = args.next() else {
                        eprintln!("usage: escale-epc pack [--issued] [--sign <ssh-ed25519-key>] [--force] <source-dir> [output-dir]");
                        return ExitCode::from(2);
                    };
                    signing_key = Some(PathBuf::from(value));
                } else if arg == "--force" {
                    force_signature = true;
                } else if arg == "--issued" {
                    issued = true;
                } else {
                    values.push(arg);
                }
            }
            if values.is_empty() || values.len() > 2 {
                eprintln!(
                    "usage: escale-epc pack [--issued] [--sign <ssh-ed25519-key>] [--force] <source-dir> [output-dir]"
                );
                return ExitCode::from(2);
            }
            if force_signature && signing_key.is_none() {
                eprintln!("--force can only be used with pack --sign");
                return ExitCode::from(2);
            }
            if issued && signing_key.is_some() {
                eprintln!("--issued cannot be used with pack --sign");
                return ExitCode::from(2);
            }
            let source_dir = values[0].clone();
            let source_path = Path::new(&source_dir);
            if !source_path.is_dir() {
                if is_epc_archive_path(source_path) {
                    eprintln!(
                        "pack expects an unpacked EPC source directory, not a .epc archive: {}",
                        source_path.display()
                    );
                } else {
                    eprintln!(
                        "pack source must be an unpacked EPC directory: {}",
                        source_path.display()
                    );
                }
                return ExitCode::from(2);
            }
            let output_dir = values
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| default_pack_output_dir(&source_dir));

            let result = if issued {
                epc_pack::pack_core_format_to_directory_issued(source_dir, output_dir)
            } else {
                match signing_key {
                    Some(signing_key) => epc_pack::pack_core_format_to_directory_signed(
                        source_dir,
                        output_dir,
                        signing_key,
                        force_signature,
                    ),
                    None => epc_pack::pack_core_format_to_directory(source_dir, output_dir),
                }
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
                Err(epc_pack::PackError::SealedSource(message)) => {
                    eprintln!("failed to pack capsule: {message}");
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
                Err(epc_pack::PackError::SealedSource(message)) => {
                    eprintln!("failed to sign capsule: {message}");
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
    println!("  create [--force] <draft-dir|cover.jpg|cover.jpeg|cover.png|cover.webp|cover.jxl> <author> [message]");
    println!("                                      Create or reset an EPC draft from a prepared dir or cover image");
    println!("  validate <capsule.epc>               Validate a core-format EPC archive");
    println!("  validate-dir <unpacked-capsule-dir>  Validate an unpacked core-format capsule");
    println!("  pack [--issued] [--sign <ssh-key>] [--force] <source-dir> [output-dir]");
    println!("                                      Pack sealed by default, or issued for travel handoff");
    println!("  sign [--force] --ssh-key <ssh-key> <source-dir>");
    println!("                                      Write proof/signature.json with Ed25519");
    println!("  image <command> ...                  Inspect, preview, encode, or prepare JXL");
    println!("  generate-test-vectors <dir>          Generate core-format conformance vectors");
    println!("  --version                            Print the EPC version");
}

fn create_draft(
    input: &str,
    author_display_name: &str,
    message: Option<&str>,
    force: bool,
    options: &epc_image::EncodeOptions,
) -> ExitCode {
    let input = PathBuf::from(input);
    if is_supported_create_image(&input) {
        return create_draft_from_image(&input, author_display_name, message, force, options);
    }

    if !input.exists() {
        eprintln!("draft directory does not exist: {}", input.display());
        return ExitCode::from(2);
    }
    if !input.is_dir() {
        eprintln!(
            "create expects an existing draft directory or a .jpg/.jpeg/.png/.webp/.jxl cover image: {}",
            input.display()
        );
        return ExitCode::from(2);
    }

    let cover_path = supported_cover_paths()
        .iter()
        .map(|path| input.join(path))
        .zip(supported_cover_paths())
        .find_map(|(full_path, cover_path)| full_path.is_file().then_some(cover_path));
    let Some(cover_path) = cover_path else {
        eprintln!("draft directory is missing a supported media/cover image");
        return ExitCode::from(2);
    };

    let request = epc_pack::CreateDraftRequest::new(&input, author_display_name)
        .with_force(force)
        .with_cover_path(cover_path);
    finish_create_draft(request, message)
}

fn create_draft_from_image(
    input: &Path,
    author_display_name: &str,
    message: Option<&str>,
    force: bool,
    options: &epc_image::EncodeOptions,
) -> ExitCode {
    if !input.is_file() {
        eprintln!("cover image does not exist: {}", input.display());
        return ExitCode::from(2);
    }

    let draft_dir = match default_image_draft_dir(input) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to choose draft directory: {error}");
            return ExitCode::from(2);
        }
    };
    let cover_path = match cover_path_for_source_image(input) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let request = epc_pack::CreateDraftRequest::new(&draft_dir, author_display_name)
        .with_force(force)
        .with_cover_path(cover_path);

    match epc_pack::create_draft_directory(request) {
        Ok(draft_dir) => {
            if let Err(error) = prepare_cover_image(input, &draft_dir, force, options) {
                let cleanup_error = fs::remove_dir_all(&draft_dir).err();
                eprintln!("{error}");
                if let Some(cleanup_error) = cleanup_error {
                    eprintln!(
                        "failed to remove incomplete draft {}: {cleanup_error}",
                        draft_dir.display()
                    );
                }
                return ExitCode::from(2);
            }
            if let Err(error) = write_optional_message(&draft_dir, message) {
                let cleanup_error = fs::remove_dir_all(&draft_dir).err();
                eprintln!("{error}");
                if let Some(cleanup_error) = cleanup_error {
                    eprintln!(
                        "failed to remove incomplete draft {}: {cleanup_error}",
                        draft_dir.display()
                    );
                }
                return ExitCode::from(2);
            }
            println!("{}", draft_dir.display());
            ExitCode::SUCCESS
        }
        Err(error) => report_create_draft_error(error),
    }
}

fn finish_create_draft(request: epc_pack::CreateDraftRequest, message: Option<&str>) -> ExitCode {
    match epc_pack::create_draft_directory(request) {
        Ok(draft_dir) => {
            if let Err(error) = write_optional_message(&draft_dir, message) {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
            println!("{}", draft_dir.display());
            ExitCode::SUCCESS
        }
        Err(error) => report_create_draft_error(error),
    }
}

fn report_create_draft_error(error: epc_pack::PackError) -> ExitCode {
    match error {
        epc_pack::PackError::Io(error) => {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                eprintln!(
                    "failed to create draft: manifest.json already exists; refusing to overwrite an existing draft"
                );
            } else {
                eprintln!("failed to create draft: {error}");
            }
            ExitCode::from(2)
        }
        error => {
            eprintln!("failed to create draft: {error:?}");
            ExitCode::from(2)
        }
    }
}

fn write_optional_message(draft_dir: &Path, message: Option<&str>) -> Result<(), String> {
    let Some(message) = message else {
        return Ok(());
    };

    fs::write(draft_dir.join(epc_core::MESSAGE_PATH), message)
        .map_err(|error| format!("failed to write message.md: {error}"))
}

fn is_supported_create_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("jpg")
                || extension.eq_ignore_ascii_case("jpeg")
                || extension.eq_ignore_ascii_case("png")
                || extension.eq_ignore_ascii_case("webp")
                || extension.eq_ignore_ascii_case("jxl")
        })
        .unwrap_or(false)
}

fn is_epc_archive_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("epc"))
        .unwrap_or(false)
}

fn cover_path_for_source_image(path: &Path) -> Result<String, String> {
    let metadata = epc_image::read_image_metadata_file(path)
        .map_err(|error| format!("failed to identify cover image format: {error:?}"))?;
    match metadata.format.as_str() {
        "JPEG" => {
            let extension = path.extension().and_then(|extension| extension.to_str());
            if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("jpeg")) {
                Ok("media/cover.jpeg".to_string())
            } else {
                Ok("media/cover.jpg".to_string())
            }
        }
        "PNG" => Ok("media/cover.png".to_string()),
        "WebP" => Ok("media/cover.webp".to_string()),
        "JPEG XL" => Ok(epc_core::COVER_PATH.to_string()),
        format => Err(format!("unsupported cover image format: {format}")),
    }
}

fn supported_cover_paths() -> [&'static str; 5] {
    [
        "media/cover.jpg",
        "media/cover.jpeg",
        "media/cover.png",
        "media/cover.webp",
        epc_core::COVER_PATH,
    ]
}

fn default_image_draft_dir(input: &Path) -> Result<PathBuf, String> {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    for _ in 0..16 {
        let candidate = parent.join(generated_draft_dir_name()?);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("could not find an unused escale-TTTTTT-RR directory name".to_string())
}

fn generated_draft_dir_name() -> Result<String, String> {
    let time = current_time_code()?;
    let random = random_base36_code(2)?;
    Ok(format!("escale-{time}-{random}"))
}

fn current_time_code() -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock must not be before the Unix epoch".to_string())?;
    let offset_millis = device_utc_offset_millis();
    let utc_seconds = now.as_secs() as i128;
    let offset_seconds = offset_millis / 1000;
    let seconds_per_day = 86_400_i128;
    let local_seconds = (utc_seconds + offset_seconds).rem_euclid(seconds_per_day);
    Ok(base36_fixed(local_seconds as u64, 6))
}

fn device_utc_offset_millis() -> i128 {
    let local_time = epc_pack::detect_device_created_local_time();
    parse_utc_offset_millis(&local_time.utc_offset).unwrap_or(0)
}

fn parse_utc_offset_millis(value: &str) -> Option<i128> {
    let sign = match value.as_bytes().first().copied()? {
        b'+' => 1_i128,
        b'-' => -1_i128,
        _ => return None,
    };
    let (hours, minutes) = value.get(1..)?.split_once(':')?;
    let hours = hours.parse::<i128>().ok()?;
    let minutes = minutes.parse::<i128>().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * ((hours * 60 + minutes) * 60 * 1000))
}

fn random_base36_code(width: usize) -> Result<String, String> {
    let mut bytes = [0_u8; 2];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_err()
    {
        let counter = DRAFT_DIR_RANDOM_COUNTER.fetch_add(1, Ordering::Relaxed) as u16;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "system clock must not be before the Unix epoch".to_string())?
            .subsec_nanos() as u16;
        bytes = (counter ^ nanos).to_be_bytes();
    }

    let space = 36_u64.pow(width as u32);
    let value = u16::from_be_bytes(bytes) as u64 % space;
    Ok(base36_fixed(value, width))
}

fn base36_fixed(mut value: u64, width: usize) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut output = vec![b'0'; width];
    for slot in output.iter_mut().rev() {
        *slot = ALPHABET[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(output).expect("base36 alphabet is valid UTF-8")
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
    eprintln!("  image encode <input.jpg|png|webp> <output.jxl> --kind cover|thumbnail [--distance <n>|--quality <n>] [--effort <n>] [--force]");
    eprintln!("  image prepare <input.jpg|jpeg|png|webp|jxl> <draft-dir> [--distance <n>|--quality <n>] [--effort <n>] [--force]");
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
        eprintln!("usage: escale-epc image encode <input.jpg|png|webp> <output.jxl> --kind cover|thumbnail [--force]");
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
        eprintln!(
            "usage: escale-epc image prepare <input.jpg|jpeg|png|webp|jxl> <draft-dir> [--force]"
        );
        return ExitCode::from(2);
    }

    let input = PathBuf::from(&values[0]);
    let draft = PathBuf::from(&values[1]);
    match prepare_cover_image(&input, &draft, force, &options) {
        Ok(cover_path) => {
            println!("{}", draft.join(cover_path).display());
            println!("{}", draft.join(epc_core::THUMBNAIL_PATH).display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn prepare_cover_image(
    input: &Path,
    draft: &Path,
    force: bool,
    options: &epc_image::EncodeOptions,
) -> Result<String, String> {
    let media_dir = draft.join("media");
    if let Err(error) = fs::create_dir_all(&media_dir) {
        return Err(format!("failed to create media directory: {error}"));
    }
    let cover_path = cover_path_for_source_image(input)?;
    let cover = draft.join(&cover_path);
    let thumbnail = draft.join(epc_core::THUMBNAIL_PATH);
    if !force && (cover.exists() || thumbnail.exists()) {
        return Err("cover or thumbnail already exists; use --force".to_string());
    }

    fs::copy(input, &cover)
        .map_err(|error| format!("failed to copy cover without modification: {error}"))?;

    epc_image::encode_thumbnail_from_cover_file(&cover, &thumbnail, options).map_err(|error| {
        format!(
            "failed to create thumbnail: {}",
            format_thumbnail_error(error)
        )
    })?;

    epc_pack::refresh_manifest_image_metadata(draft)
        .map_err(|error| format!("failed to update manifest image metadata: {error:?}"))?;
    Ok(cover_path)
}

fn encode_and_validate_file(
    input: &std::path::Path,
    output: &std::path::Path,
    kind: epc_image::EpcImageKind,
    options: &epc_image::EncodeOptions,
) -> Result<(), String> {
    let encode_result = match kind {
        epc_image::EpcImageKind::Cover => {
            epc_image::encode_file_to_jxl_file(input, output, options)
        }
        epc_image::EpcImageKind::Thumbnail => {
            epc_image::encode_file_to_thumbnail_jxl_file(input, output, options)
        }
    };
    encode_result.map_err(|error| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base36_fixed_pads_and_encodes_time_range() {
        assert_eq!(base36_fixed(0, 6), "000000");
        assert_eq!(base36_fixed(35, 2), "0Z");
        assert_eq!(base36_fixed(86_399, 6), "001UNZ");
    }

    #[test]
    fn generated_draft_dir_name_uses_time_and_random_groups() {
        let name = generated_draft_dir_name().unwrap();
        assert_eq!(name.len(), "escale-000000-00".len());
        assert!(name.starts_with("escale-"));
        assert_eq!(name.as_bytes()[13], b'-');
    }

    #[test]
    fn parses_normalized_utc_offsets() {
        assert_eq!(parse_utc_offset_millis("+02:30"), Some(9_000_000));
        assert_eq!(parse_utc_offset_millis("-01:15"), Some(-4_500_000));
        assert_eq!(parse_utc_offset_millis("+24:00"), None);
        assert_eq!(parse_utc_offset_millis("+02"), None);
    }

    #[test]
    fn detects_supported_create_images_case_insensitively() {
        assert!(is_supported_create_image(Path::new("cover.JPG")));
        assert!(is_supported_create_image(Path::new("cover.jpeg")));
        assert!(is_supported_create_image(Path::new("cover.PNG")));
        assert!(is_supported_create_image(Path::new("cover.webp")));
        assert!(is_supported_create_image(Path::new("cover.jxl")));
    }

    #[test]
    fn chooses_cover_path_from_image_signature_instead_of_extension() {
        let path = std::env::temp_dir().join("epc-cli-jpeg-named-png.png");
        std::fs::write(
            &path,
            include_bytes!("../../../../resource/images/arc-de-triomphe-paris.jpeg"),
        )
        .unwrap();

        let cover_path = cover_path_for_source_image(&path).unwrap();

        let _ = std::fs::remove_file(path);
        assert_eq!(cover_path, "media/cover.jpg");
    }
}
