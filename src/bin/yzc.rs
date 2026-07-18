use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use yazelix_cursors::{
    CursorDefinition, CursorError, CursorErrorClass, CursorFamily, CursorRegistry,
    ResolvedCursorRegistryState, STANDALONE_CURSOR_CONFIG_DIR_NAME,
    STANDALONE_CURSOR_CONFIG_FILENAME, SplitDivider, cursor_target_contracts,
    format_ghostty_trail_duration, initialize_cursor_config, load_cursor_config,
    write_ghostty_cursor_effect_shaders, write_ghostty_cursor_palette_shaders,
};

const GHOSTTY_INCLUDE_FILE_NAME: &str = "ghostty.conf";
const SHARE_RELATIVE_PATH: &[&str] = &["share", "yazelix", "yazelix_cursors"];
const EFFECTS_REQUIRING_ALWAYS_ANIMATION: &[&str] =
    &["ripple", "sonic_boom", "rectangle_boom", "ripple_rectangle"];

#[derive(Debug)]
struct Cli {
    config_dir: PathBuf,
    share_dir: Option<PathBuf>,
    command: Command,
}

#[derive(Debug)]
enum Command {
    Init,
    List,
    ListTargets,
    Inspect,
    Current {
        format: CurrentFormat,
    },
    GenerateGhostty,
    MaterializeRioConfig {
        source_config: PathBuf,
        output_root: Option<PathBuf>,
    },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentFormat {
    Env,
    Json,
}

#[derive(Debug)]
struct Paths {
    config_dir: PathBuf,
    config_path: PathBuf,
    ghostty_include_path: PathBuf,
    shaders_path: PathBuf,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("Error: {}", error.message());
            eprintln!("{}", error.remediation());
            std::process::exit(error.class().exit_code());
        }
    }
}

fn run() -> Result<(), CursorError> {
    let cli = parse_cli(env::args().skip(1))?;
    match &cli.command {
        Command::Init => run_init(&cli),
        Command::List => run_list(&cli),
        Command::ListTargets => run_list_targets(),
        Command::Inspect => run_inspect(&cli),
        Command::Current { format } => run_current(&cli, *format),
        Command::GenerateGhostty => run_generate_ghostty(&cli),
        Command::MaterializeRioConfig {
            source_config,
            output_root,
        } => run_materialize_rio_compatible_config(&cli, source_config, output_root.as_deref()),
        Command::Help => {
            print_help();
            Ok(())
        }
    }
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<Cli, CursorError> {
    let mut args = args.into_iter();
    let mut config_dir = None;
    let mut share_dir = None;
    let mut command_parts = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" if command_parts.is_empty() => {
                return Ok(Cli {
                    config_dir: default_config_dir()?,
                    share_dir,
                    command: Command::Help,
                });
            }
            "--config-dir" if command_parts.is_empty() => {
                let value = args.next().ok_or_else(|| {
                    usage_error("Missing value after --config-dir. Try `yzc --help`.")
                })?;
                config_dir = Some(expand_tilde(PathBuf::from(value))?);
            }
            "--share-dir" if command_parts.is_empty() => {
                let value = args.next().ok_or_else(|| {
                    usage_error("Missing value after --share-dir. Try `yzc --help`.")
                })?;
                share_dir = Some(expand_tilde(PathBuf::from(value))?);
            }
            other if other.starts_with('-') && command_parts.is_empty() => {
                return Err(usage_error(format!(
                    "Unknown yzc option: {other}. Try `yzc --help`."
                )));
            }
            _ => command_parts.push(arg),
        }
    }

    let command = match command_parts.as_slice() {
        [] => Command::Help,
        [single] if matches!(single.as_str(), "-h" | "--help" | "help") => Command::Help,
        [single] if single == "init" => Command::Init,
        [single] if single == "list" => Command::List,
        [single] if single == "list-targets" => Command::ListTargets,
        [single] if single == "inspect" => Command::Inspect,
        [single] if single == "current" => Command::Current {
            format: CurrentFormat::Env,
        },
        [current, flag, format] if current == "current" && flag == "--format" => Command::Current {
            format: parse_current_format(format)?,
        },
        [generate, target] if generate == "generate" && target == "ghostty" => {
            Command::GenerateGhostty
        }
        [materialize, target, rest @ ..]
            if materialize == "materialize" && target == "rio-compatible-config" =>
        {
            let (source_config, output_root) = parse_materialize_rio_compatible_config_args(rest)?;
            Command::MaterializeRioConfig {
                source_config,
                output_root,
            }
        }
        _ => {
            return Err(usage_error(format!(
                "Unknown yzc command: {}. Try `yzc --help`.",
                command_parts.join(" ")
            )));
        }
    };

    Ok(Cli {
        config_dir: config_dir.unwrap_or(default_config_dir()?),
        share_dir,
        command,
    })
}

fn parse_materialize_rio_compatible_config_args(
    args: &[String],
) -> Result<(PathBuf, Option<PathBuf>), CursorError> {
    let mut source_config = None;
    let mut output_root = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source-config" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    usage_error("Missing value after --source-config. Try `yzc --help`.")
                })?;
                source_config = Some(expand_tilde(PathBuf::from(value))?);
            }
            "--output-root" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    usage_error("Missing value after --output-root. Try `yzc --help`.")
                })?;
                output_root = Some(expand_tilde(PathBuf::from(value))?);
            }
            other => {
                return Err(usage_error(format!(
                    "Unknown yzc materialize rio-compatible-config option: {other}. Try `yzc --help`."
                )));
            }
        }
        index += 1;
    }
    let source_config = source_config.ok_or_else(|| {
        usage_error("yzc materialize rio-compatible-config requires --source-config <path>.")
    })?;
    Ok((source_config, output_root))
}

fn parse_current_format(raw: &str) -> Result<CurrentFormat, CursorError> {
    match raw {
        "env" => Ok(CurrentFormat::Env),
        "json" => Ok(CurrentFormat::Json),
        _ => Err(usage_error(format!(
            "Unknown yzc current format: {raw}. Use env or json."
        ))),
    }
}

fn run_init(cli: &Cli) -> Result<(), CursorError> {
    let paths = paths(&cli.config_dir);
    fs::create_dir_all(&paths.config_dir).map_err(|source| {
        CursorError::io(
            "create_yzc_config_dir",
            "Could not create Yazelix cursor config directory",
            "Check permissions for the config directory and retry.",
            paths.config_dir.to_string_lossy(),
            source,
        )
    })?;

    if !initialize_cursor_config(&paths.config_path)? {
        println!(
            "cursors.toml already exists: {}",
            paths.config_path.display()
        );
        return Ok(());
    }

    println!("created {}", paths.config_path.display());
    Ok(())
}

fn run_list(cli: &Cli) -> Result<(), CursorError> {
    let paths = paths(&cli.config_dir);
    let registry = load_cursor_config(&paths.config_path)?;

    println!("Yazelix cursors");
    println!("Config: {}", paths.config_path.display());
    println!("Trail: {}", trail_summary(&registry));
    println!("Trail effect: {}", registry.settings.trail_effect);
    println!("Mode effect: {}", registry.settings.mode_effect);
    println!("Glow: {}", registry.settings.glow);
    println!(
        "Duration: {}",
        format_ghostty_trail_duration(registry.settings.duration)
    );
    println!();
    for definition in registry.enabled_definitions() {
        println!("- {}", cursor_definition_summary(definition));
    }
    Ok(())
}

fn run_inspect(cli: &Cli) -> Result<(), CursorError> {
    let paths = paths(&cli.config_dir);
    let share_dir = resolve_share_dir(cli.share_dir.as_deref());

    println!("Yazelix cursors");
    println!("Config dir: {}", paths.config_dir.display());
    println!("Config: {}", paths.config_path.display());
    println!("Ghostty include: {}", paths.ghostty_include_path.display());
    println!("Generated shaders: {}", paths.shaders_path.display());
    match share_dir {
        Ok(path) => println!("Packaged shaders: {}", path.join("shaders").display()),
        Err(error) => println!("Packaged shaders: unavailable ({})", error.message()),
    }

    if !paths.config_path.exists() {
        println!("Status: missing config");
        println!("Next: yzc init");
        return Ok(());
    }

    let registry = load_cursor_config(&paths.config_path)?;
    let resolved = registry.resolve();
    println!("Status: config ok");
    println!(
        "Selected cursor: {}",
        selected_cursor_summary(&resolved.selected_cursor)
    );
    println!(
        "Selected effects: trail={} mode={}",
        resolved.selected_trail_effect.as_deref().unwrap_or("none"),
        resolved.selected_mode_effect.as_deref().unwrap_or("none")
    );
    Ok(())
}

fn run_current(cli: &Cli, format: CurrentFormat) -> Result<(), CursorError> {
    let paths = paths(&cli.config_dir);
    if !paths.config_path.exists() {
        print_current_cursor(None, format);
        return Ok(());
    }

    let registry = load_cursor_config(&paths.config_path)?;
    let resolved = registry.resolve();
    print_current_cursor(resolved.selected_cursor.as_ref(), format);
    Ok(())
}

fn print_current_cursor(cursor: Option<&CursorDefinition>, format: CurrentFormat) {
    match format {
        CurrentFormat::Env => {
            if let Some(cursor) = cursor {
                println!("YAZELIX_CURSOR_NAME={}", cursor.name);
                println!("YAZELIX_CURSOR_COLOR={}", cursor.cursor_color.hex);
                println!("YAZELIX_CURSOR_FAMILY={}", cursor.family.as_str());
                if cursor.family == CursorFamily::Split {
                    if let Some(divider) = cursor.divider {
                        println!("YAZELIX_CURSOR_DIVIDER={}", divider.as_str());
                    }
                    if let Some(primary) = cursor.colors.first() {
                        println!("YAZELIX_CURSOR_PRIMARY_COLOR={}", primary.hex);
                    }
                    if let Some(secondary) = cursor.colors.get(1) {
                        println!("YAZELIX_CURSOR_SECONDARY_COLOR={}", secondary.hex);
                    }
                }
            }
        }
        CurrentFormat::Json => {
            if let Some(cursor) = cursor {
                println!("{}", current_cursor_json(cursor));
            } else {
                println!("{{}}");
            }
        }
    }
}

fn current_cursor_json(cursor: &CursorDefinition) -> serde_json::Value {
    let mut value = json!({
        "name": cursor.name,
        "color": cursor.cursor_color.hex,
        "family": cursor.family.as_str(),
    });
    if cursor.family == CursorFamily::Split {
        if let Some(divider) = cursor.divider {
            value["divider"] = json!(divider.as_str());
        }
        if let Some(primary) = cursor.colors.first() {
            value["primary_color"] = json!(primary.hex);
        }
        if let Some(secondary) = cursor.colors.get(1) {
            value["secondary_color"] = json!(secondary.hex);
        }
    }
    value
}

fn run_generate_ghostty(cli: &Cli) -> Result<(), CursorError> {
    let paths = paths(&cli.config_dir);
    let share_dir = resolve_share_dir(cli.share_dir.as_deref())?;
    let shader_src = share_dir.join("shaders");
    if !shader_src.exists() {
        return Err(CursorError::classified(
            CursorErrorClass::Io,
            "missing_yzc_packaged_shaders",
            "Could not find packaged Yazelix cursor shaders.",
            "Reinstall the yazelix_cursors package or pass --share-dir pointing at share/yazelix/yazelix_cursors.",
            json!({ "path": shader_src.display().to_string() }),
        ));
    }

    let registry = load_cursor_config(&paths.config_path)?;
    let resolved = registry.resolve();
    fs::create_dir_all(&paths.config_dir).map_err(|source| {
        CursorError::io(
            "create_yzc_config_dir",
            "Could not create Yazelix cursor config directory",
            "Check permissions for the config directory and retry.",
            paths.config_dir.to_string_lossy(),
            source,
        )
    })?;
    replace_dir(&shader_src, &paths.shaders_path)?;
    write_ghostty_cursor_palette_shaders(
        &paths.shaders_path,
        &registry,
        &resolved.glow,
        resolved.duration,
    )?;
    write_ghostty_cursor_effect_shaders(
        &paths.shaders_path,
        &resolved.glow,
        "iCurrentCursorColor",
        resolved.duration,
    )?;

    let config = render_ghostty_include(&paths, &resolved)?;
    fs::write(&paths.ghostty_include_path, config).map_err(|source| {
        CursorError::io(
            "write_yzc_ghostty_include",
            "Could not write Yazelix cursor Ghostty include",
            "Check permissions for the config directory and retry.",
            paths.ghostty_include_path.to_string_lossy(),
            source,
        )
    })?;

    println!("wrote {}", paths.ghostty_include_path.display());
    Ok(())
}

fn run_materialize_rio_compatible_config(
    cli: &Cli,
    source_config: &Path,
    output_root: Option<&Path>,
) -> Result<(), CursorError> {
    let source_config = expand_tilde(source_config.to_path_buf())?;
    let source_text = fs::read_to_string(&source_config).map_err(|source| {
        CursorError::io(
            "read_yzc_rio_compatible_config_source",
            "Could not read the source Rio-compatible terminal config.",
            "Pass --source-config pointing at an existing config.toml.",
            source_config.to_string_lossy(),
            source,
        )
    })?;
    toml::from_str::<toml::Value>(&source_text).map_err(|source| {
        CursorError::toml(
            "parse_yzc_rio_compatible_config_source",
            "Could not parse the source Rio-compatible terminal config.",
            "Fix the source config.toml before materializing a launch config.",
            source_config.to_string_lossy(),
            source,
        )
    })?;

    let paths = paths(&cli.config_dir);
    let selected_cursor = if paths.config_path.exists() {
        let registry = load_cursor_config(&paths.config_path)?;
        registry.resolve().selected_cursor
    } else {
        None
    };
    let color = selected_cursor
        .as_ref()
        .map(CursorDefinition::cursor_color_hex);
    let patched = patch_rio_compatible_cursor_color(&source_text, color);
    toml::from_str::<toml::Value>(&patched).map_err(|source| {
        CursorError::toml(
            "parse_yzc_materialized_rio_compatible_config",
            "Generated Rio-compatible terminal config is not valid TOML.",
            "Report this Yazelix Cursors bug with the source config and cursor settings.",
            source_config.to_string_lossy(),
            source,
        )
    })?;

    let destination = create_launch_config_dir(output_root)?;
    let config_path = destination.join("config.toml");
    fs::write(&config_path, patched).map_err(|source| {
        CursorError::io(
            "write_yzc_materialized_rio_compatible_config",
            "Could not write the generated Rio-compatible terminal config.",
            "Check permissions for the output root and retry.",
            config_path.to_string_lossy(),
            source,
        )
    })?;

    let (name, family, v1_mode) = match selected_cursor.as_ref() {
        Some(cursor) if cursor.family == CursorFamily::Mono => (
            Some(cursor.name.as_str()),
            Some(cursor.family.as_str()),
            "monocolor",
        ),
        Some(cursor) => (
            Some(cursor.name.as_str()),
            Some(cursor.family.as_str()),
            "monocolor-fallback",
        ),
        None => (None, None, "disabled"),
    };
    let metadata = json!({
        "name": name,
        "family": family,
        "color": color,
        "v1_mode": v1_mode,
    });
    let metadata_path = destination.join("yazelix_cursor.json");
    fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata).expect("cursor metadata must be serializable")
            + "\n",
    )
    .map_err(|source| {
        CursorError::io(
            "write_yzc_materialized_rio_compatible_cursor_metadata",
            "Could not write generated Rio-compatible cursor metadata.",
            "Check permissions for the output root and retry.",
            metadata_path.to_string_lossy(),
            source,
        )
    })?;

    println!("{}", destination.display());
    Ok(())
}

fn run_list_targets() -> Result<(), CursorError> {
    for target in cursor_target_contracts() {
        println!("{}", target.name);
        println!("  status: {}", target.status);
        println!("  emits: {}", target.emits.join(", "));
        println!("  requires: {}", target.requires.join(", "));
        println!("  notes: {}", target.notes.join(" "));
    }
    Ok(())
}

fn print_help() {
    println!("Yazelix Cursors");
    println!();
    println!("Usage:");
    println!("  yzc [--config-dir <dir>] init");
    println!("  yzc [--config-dir <dir>] list");
    println!("  yzc list-targets");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] inspect");
    println!("  yzc [--config-dir <dir>] current [--format env|json]");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] generate ghostty");
    println!(
        "  yzc [--config-dir <dir>] materialize rio-compatible-config --source-config <path> [--output-root <dir>]"
    );
    println!();
    println!("Defaults:");
    println!("  config: ~/.config/yazelix_cursors/cursors.toml");
    println!("  Ghostty include: ~/.config/yazelix_cursors/ghostty.conf");
    println!();
    println!("Ghostty opt-in:");
    println!("  config-file = ~/.config/yazelix_cursors/ghostty.conf");
}

fn paths(config_dir: &Path) -> Paths {
    Paths {
        config_dir: config_dir.to_path_buf(),
        config_path: config_dir.join(STANDALONE_CURSOR_CONFIG_FILENAME),
        ghostty_include_path: config_dir.join(GHOSTTY_INCLUDE_FILE_NAME),
        shaders_path: config_dir.join("shaders"),
    }
}

fn default_config_dir() -> Result<PathBuf, CursorError> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(config_home).join(STANDALONE_CURSOR_CONFIG_DIR_NAME));
    }
    let home = env::var_os("HOME").ok_or_else(|| {
        CursorError::classified(
            CursorErrorClass::Config,
            "missing_home_for_yzc_config",
            "Could not determine the Yazelix cursor config directory.",
            "Set XDG_CONFIG_HOME or HOME, or pass --config-dir explicitly.",
            json!({}),
        )
    })?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join(STANDALONE_CURSOR_CONFIG_DIR_NAME))
}

fn resolve_share_dir(override_dir: Option<&Path>) -> Result<PathBuf, CursorError> {
    if let Some(path) = override_dir {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env::var_os("YZC_SHARE_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let exe = env::current_exe().map_err(|source| {
        CursorError::io(
            "resolve_yzc_current_exe",
            "Could not resolve the yzc executable path",
            "Run yzc from the yazelix_cursors package, or pass --share-dir explicitly.",
            "yzc",
            source,
        )
    })?;
    let Some(package_root) = exe.parent().and_then(Path::parent) else {
        return Err(CursorError::classified(
            CursorErrorClass::Runtime,
            "invalid_yzc_package_layout",
            "Could not infer the yazelix_cursors package root from the yzc executable path.",
            "Run yzc from the yazelix_cursors package, or pass --share-dir explicitly.",
            json!({ "executable": exe.display().to_string() }),
        ));
    };

    let share_dir = SHARE_RELATIVE_PATH
        .iter()
        .fold(package_root.to_path_buf(), |path, segment| {
            path.join(segment)
        });
    if share_dir.exists() {
        Ok(share_dir)
    } else {
        Err(CursorError::classified(
            CursorErrorClass::Runtime,
            "missing_yzc_share_dir",
            "Could not find the yazelix_cursors packaged share directory.",
            "Run yzc from the yazelix_cursors package, or pass --share-dir pointing at share/yazelix/yazelix_cursors.",
            json!({
                "executable": exe.display().to_string(),
                "expected": share_dir.display().to_string(),
            }),
        ))
    }
}

fn render_ghostty_include(
    paths: &Paths,
    resolved: &ResolvedCursorRegistryState,
) -> Result<String, CursorError> {
    let mut lines = vec![
        "# Yazelix Cursors Ghostty include".to_string(),
        "# Generated by yzc. Re-run `yzc generate ghostty` after editing cursors.toml.".to_string(),
        format!(
            "# Cursor trail duration multiplier: {}",
            format_ghostty_trail_duration(resolved.duration)
        ),
    ];

    if let Some(cursor) = &resolved.selected_cursor {
        lines.push(format!("# Cursor palette: {}", cursor.name));
        lines.push(format!("cursor-color = {}", cursor.cursor_color_hex()));
        lines.push(format!(
            "custom-shader = {}",
            absolute_shader_path(&paths.shaders_path, cursor_shader_file_name(cursor))?
        ));
    } else if resolved.trail_disabled {
        lines.push("# Cursor palette: none (disabled)".to_string());
    } else {
        lines.push("# Cursor palette: n/a".to_string());
    }

    let selected_effects = [
        resolved.selected_trail_effect.as_deref(),
        resolved.selected_mode_effect.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if selected_effects.is_empty() {
        lines.push("# Cursor effects: none".to_string());
    } else {
        lines.push(format!("# Cursor effects: {}", selected_effects.join(", ")));
        if resolved
            .selected_mode_effect
            .as_deref()
            .is_some_and(|effect| EFFECTS_REQUIRING_ALWAYS_ANIMATION.contains(&effect))
        {
            lines.push("custom-shader-animation = always".to_string());
        }
        for effect in selected_effects {
            lines.push(format!(
                "custom-shader = {}",
                absolute_shader_path(
                    &paths.shaders_path,
                    format!("generated_effects/{effect}.glsl")
                )?
            ));
        }
    }

    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn absolute_shader_path(
    shaders_path: &Path,
    relative_file: impl AsRef<Path>,
) -> Result<String, CursorError> {
    let path = shaders_path.join(relative_file);
    let absolute = path.canonicalize().map_err(|source| {
        CursorError::io(
            "resolve_yzc_shader_path",
            "Could not resolve generated Yazelix cursor shader path",
            "Run `yzc generate ghostty` again and check permissions for the generated shader directory.",
            path.to_string_lossy(),
            source,
        )
    })?;
    Ok(absolute.display().to_string())
}

fn cursor_shader_file_name(cursor: &CursorDefinition) -> String {
    format!("cursor_trail_{}.glsl", cursor.name)
}

fn replace_dir(src: &Path, dst: &Path) -> Result<(), CursorError> {
    if dst.exists() {
        fs::remove_dir_all(dst).map_err(|source| {
            CursorError::io(
                "remove_yzc_shader_dir",
                "Could not remove previous generated Yazelix cursor shader directory",
                "Check permissions for the config directory and retry.",
                dst.to_string_lossy(),
                source,
            )
        })?;
    }
    copy_dir_all(src, dst).map_err(|source| {
        CursorError::io(
            "copy_yzc_shader_assets",
            "Could not copy packaged Yazelix cursor shader assets",
            "Check permissions and disk space, then retry.",
            format!("{} -> {}", src.display(), dst.display()),
            source,
        )
    })?;
    make_tree_writable(dst).map_err(|source| {
        CursorError::io(
            "make_yzc_shader_assets_writable",
            "Could not make copied Yazelix cursor shader assets writable",
            "Check permissions for the config directory and retry.",
            dst.to_string_lossy(),
            source,
        )
    })
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn patch_rio_compatible_cursor_color(config: &str, color: Option<&str>) -> String {
    let Some(color) = color else {
        return config.to_string();
    };

    let mut lines = config.lines().map(str::to_string).collect::<Vec<_>>();
    let Some(colors_index) = lines.iter().position(|line| line.trim() == "[colors]") else {
        let mut rendered = config.trim_end_matches('\n').to_string();
        if !rendered.is_empty() {
            rendered.push_str("\n\n");
        }
        rendered.push_str("[colors]\n");
        rendered.push_str(&format!("cursor = \"{color}\"\n"));
        return rendered;
    };

    let next_section = lines
        .iter()
        .enumerate()
        .skip(colors_index + 1)
        .find_map(|(index, line)| {
            let trimmed = line.trim();
            (trimmed.starts_with('[') && trimmed.ends_with(']')).then_some(index)
        })
        .unwrap_or(lines.len());

    for line in lines.iter_mut().take(next_section).skip(colors_index + 1) {
        if is_cursor_assignment(line) {
            let indent = line
                .chars()
                .take_while(|character| character.is_whitespace())
                .collect::<String>();
            *line = format!("{indent}cursor = \"{color}\"");
            return render_lines_with_final_newline(lines);
        }
    }

    lines.insert(colors_index + 1, format!("cursor = \"{color}\""));
    render_lines_with_final_newline(lines)
}

fn is_cursor_assignment(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("cursor") else {
        return false;
    };
    rest.trim_start().starts_with('=')
}

fn render_lines_with_final_newline(lines: Vec<String>) -> String {
    let mut rendered = lines.join("\n");
    rendered.push('\n');
    rendered
}

fn create_launch_config_dir(output_root: Option<&Path>) -> Result<PathBuf, CursorError> {
    let root = output_root
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .map(|path| path.join("yazelix-cursors-rio-compatible-config"))
        .unwrap_or_else(|| env::temp_dir().join("yazelix-cursors-rio-compatible-config"));
    fs::create_dir_all(&root).map_err(|source| {
        CursorError::io(
            "create_yzc_rio_compatible_config_output_root",
            "Could not create the Rio-compatible launch config output root.",
            "Check permissions for the output root and retry.",
            root.to_string_lossy(),
            source,
        )
    })?;

    for attempt in 0..1000 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let candidate = root.join(format!("launch-{}-{nanos}-{attempt}", process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(CursorError::io(
                    "create_yzc_rio_compatible_config_launch_dir",
                    "Could not create the Rio-compatible launch config directory.",
                    "Check permissions for the output root and retry.",
                    candidate.to_string_lossy(),
                    source,
                ));
            }
        }
    }

    Err(CursorError::classified(
        CursorErrorClass::Runtime,
        "create_yzc_rio_compatible_config_launch_dir_exhausted",
        "Could not allocate a unique Rio-compatible launch config directory.",
        "Remove stale launch directories or pass a different --output-root.",
        json!({ "root": root.display().to_string() }),
    ))
}

#[cfg(unix)]
fn make_tree_writable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            make_tree_writable(&entry_path)?;
        }
        let metadata = fs::metadata(&entry_path)?;
        let mut permissions = metadata.permissions();
        let executable_bit = if metadata.is_dir() { 0o100 } else { 0 };
        permissions.set_mode(permissions.mode() | 0o200 | executable_bit);
        fs::set_permissions(&entry_path, permissions)?;
    }

    let metadata = fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o300);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn make_tree_writable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn selected_cursor_summary(cursor: &Option<CursorDefinition>) -> String {
    match cursor {
        Some(cursor) => format!("{} ({})", cursor.name, cursor.family.as_str()),
        None => "none".to_string(),
    }
}

fn trail_summary(registry: &CursorRegistry) -> String {
    match registry.settings.trail.as_str() {
        "none" => "none (disabled)".to_string(),
        "random" => format!(
            "random from {} enabled cursors",
            registry.enabled_cursors.len()
        ),
        selected => selected.to_string(),
    }
}

fn cursor_definition_summary(definition: &CursorDefinition) -> String {
    match definition.family {
        CursorFamily::Mono => format!(
            "{}: mono base={} accent={} cursor={}",
            definition.name,
            definition.colors[0].hex,
            definition.colors[1].hex,
            definition.cursor_color.hex
        ),
        CursorFamily::Split => {
            let divider = definition
                .divider
                .expect("validated split cursor definitions always have a divider");
            let transition = definition
                .transition
                .expect("validated split cursor definitions always have a transition");
            let (first_label, second_label) = split_color_labels(divider);
            format!(
                "{}: split divider={} transition={} {}={} {}={} cursor={}",
                definition.name,
                divider.as_str(),
                transition.as_str(),
                first_label,
                definition.colors[0].hex,
                second_label,
                definition.colors[1].hex,
                definition.cursor_color.hex
            )
        }
    }
}

fn split_color_labels(divider: SplitDivider) -> (&'static str, &'static str) {
    match divider {
        SplitDivider::Vertical => ("left", "right"),
        SplitDivider::Horizontal => ("top", "bottom"),
    }
}

fn expand_tilde(path: PathBuf) -> Result<PathBuf, CursorError> {
    let Some(raw) = path.to_str() else {
        return Ok(path);
    };
    if raw == "~" || raw.starts_with("~/") {
        let home = env::var_os("HOME").ok_or_else(|| {
            CursorError::classified(
                CursorErrorClass::Config,
                "missing_home_for_tilde",
                "Could not expand ~ in the yzc path.",
                "Set HOME or pass an absolute path.",
                json!({ "path": raw }),
            )
        })?;
        let home = PathBuf::from(home);
        if raw == "~" {
            return Ok(home);
        }
        return Ok(home.join(&raw[2..]));
    }
    Ok(path)
}

fn usage_error(message: impl Into<String>) -> CursorError {
    CursorError::classified(
        CursorErrorClass::Usage,
        "invalid_yzc_arguments",
        message,
        "Run `yzc --help` for the supported command surface.",
        json!({}),
    )
}

#[cfg(test)]
// Test lane: default
mod tests {
    use super::*;

    // Defends: Rio-compatible terminal config materialization updates only the cursor color in an existing colors table.
    #[test]
    fn patches_existing_rio_compatible_cursor_color() {
        let config = "[colors]\nbackground = \"#111111\"\ncursor = \"#ffffff\"\n";

        let rendered = patch_rio_compatible_cursor_color(config, Some("#ff1600"));

        assert_eq!(
            rendered,
            "[colors]\nbackground = \"#111111\"\ncursor = \"#ff1600\"\n"
        );
        toml::from_str::<toml::Value>(&rendered).unwrap();
    }

    // Defends: launch-local Rio config materialization works for base configs that do not already define colors.
    #[test]
    fn appends_colors_table_when_missing() {
        let config = "confirm-before-quit = false\n";

        let rendered = patch_rio_compatible_cursor_color(config, Some("#3bd17a"));

        assert_eq!(
            rendered,
            "confirm-before-quit = false\n\n[colors]\ncursor = \"#3bd17a\"\n"
        );
        toml::from_str::<toml::Value>(&rendered).unwrap();
    }

    // Defends: disabled cursor materialization preserves the base terminal config instead of inventing a fallback color.
    #[test]
    fn preserves_config_without_cursor_color() {
        let config = "[colors]\nbackground = \"#111111\"\n";

        assert_eq!(patch_rio_compatible_cursor_color(config, None), config);
    }
}
