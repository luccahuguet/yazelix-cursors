use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use yazelix_cursors::{
    CursorDefinition, CursorError, CursorErrorClass, CursorFamily, CursorRegistry,
    ResolvedCursorRegistryState, SplitDivider, format_ghostty_trail_duration, parse_jsonc_value,
    render_cursor_settings_jsonc, write_ghostty_cursor_effect_shaders,
    write_ghostty_cursor_palette_shaders,
};

const DEFAULT_CURSOR_CONFIG: &str = include_str!("../../yazelix_ghostty_cursors_default.toml");
const CONFIG_DIR_NAME: &str = "yazelix_ghostty_cursors";
const SETTINGS_FILE_NAME: &str = "settings.jsonc";
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
    Inspect,
    Current { format: CurrentFormat },
    GenerateGhostty,
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
    match cli.command {
        Command::Init => run_init(&cli),
        Command::List => run_list(&cli),
        Command::Inspect => run_inspect(&cli),
        Command::Current { format } => run_current(&cli, format),
        Command::GenerateGhostty => run_generate_ghostty(&cli),
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

    if paths.config_path.exists() {
        println!(
            "settings.jsonc already exists: {}",
            paths.config_path.display()
        );
        return Ok(());
    }

    let default_registry = CursorRegistry::parse_str(
        Path::new("yazelix_ghostty_cursors_default.toml"),
        DEFAULT_CURSOR_CONFIG,
    )?;
    let content = render_cursor_settings_jsonc(&default_registry);
    fs::write(&paths.config_path, content).map_err(|source| {
        CursorError::io(
            "write_yzc_settings_jsonc",
            "Could not write Yazelix cursor settings.jsonc",
            "Check permissions for the config directory and retry.",
            paths.config_path.to_string_lossy(),
            source,
        )
    })?;

    println!("created {}", paths.config_path.display());
    Ok(())
}

fn run_list(cli: &Cli) -> Result<(), CursorError> {
    let paths = paths(&cli.config_dir);
    let registry = load_standalone_registry(&paths.config_path)?;

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

    let registry = load_standalone_registry(&paths.config_path)?;
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

    let registry = load_standalone_registry(&paths.config_path)?;
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

    let registry = load_standalone_registry(&paths.config_path)?;
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

fn print_help() {
    println!("Yazelix Cursors");
    println!();
    println!("Usage:");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] init");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] list");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] inspect");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] current [--format env|json]");
    println!("  yzc [--config-dir <dir>] [--share-dir <dir>] generate ghostty");
    println!();
    println!("Defaults:");
    println!("  config: ~/.config/yazelix_ghostty_cursors/settings.jsonc");
    println!("  Ghostty include: ~/.config/yazelix_ghostty_cursors/ghostty.conf");
    println!();
    println!("Ghostty opt-in:");
    println!("  config-file = ~/.config/yazelix_ghostty_cursors/ghostty.conf");
}

fn paths(config_dir: &Path) -> Paths {
    Paths {
        config_dir: config_dir.to_path_buf(),
        config_path: config_dir.join(SETTINGS_FILE_NAME),
        ghostty_include_path: config_dir.join(GHOSTTY_INCLUDE_FILE_NAME),
        shaders_path: config_dir.join("shaders"),
    }
}

fn default_config_dir() -> Result<PathBuf, CursorError> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(config_home).join(CONFIG_DIR_NAME));
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
    Ok(PathBuf::from(home).join(".config").join(CONFIG_DIR_NAME))
}

fn load_standalone_registry(path: &Path) -> Result<CursorRegistry, CursorError> {
    let raw = fs::read_to_string(path).map_err(|source| {
        CursorError::io(
            "read_yzc_settings_jsonc",
            "Could not read Yazelix cursor settings.jsonc",
            "Run `yzc init`, or fix the settings path and retry.",
            path.to_string_lossy(),
            source,
        )
    })?;
    let value = parse_jsonc_value(path, &raw)?;
    CursorRegistry::parse_json_value(path, value)
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
        "# Generated by yzc. Re-run `yzc generate ghostty` after editing settings.jsonc."
            .to_string(),
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
    match cursor.family {
        CursorFamily::CuratedTemplate => {
            format!(
                "cursor_trail_{}.glsl",
                cursor.template.as_deref().unwrap_or(&cursor.name)
            )
        }
        CursorFamily::Mono | CursorFamily::Split => format!("cursor_trail_{}.glsl", cursor.name),
    }
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
        CursorFamily::CuratedTemplate => format!(
            "{}: curated_template template={} cursor={}",
            definition.name,
            definition.template.as_deref().unwrap_or("unknown"),
            definition.cursor_color.hex
        ),
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
