//! Reusable Yazelix cursor registry and Ghostty shader generation.
// Test lane: default

use ratconfig::migration::{MigrationError, MigrationMutation, MigrationOp, apply_migrations_text};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const DEFAULT_CURSOR_CONFIG_FILENAME: &str = "yazelix_cursors_default.toml";
pub const DEFAULT_CURSOR_CONFIG_TEMPLATE: &str = include_str!("../yazelix_cursors_default.toml");
pub const STANDALONE_CURSOR_CONFIG_DIR_NAME: &str = "yazelix_cursors";
pub const STANDALONE_CURSOR_CONFIG_FILENAME: &str = "cursors.toml";
pub const LEGACY_STANDALONE_CURSOR_SETTINGS_FILENAME: &str = "settings.jsonc";
pub const DEFAULT_GHOSTTY_TRAIL_DURATION: f64 = 1.0;
pub const GHOSTTY_TRAIL_DURATION_MIN: f64 = 0.25;
pub const GHOSTTY_TRAIL_DURATION_MAX: f64 = 4.0;

pub const SUPPORTED_TRAIL_EFFECTS: &[&str] = &["tail", "warp", "sweep"];
pub const SUPPORTED_MODE_EFFECTS: &[&str] =
    &["ripple", "sonic_boom", "rectangle_boom", "ripple_rectangle"];
pub const SUPPORTED_GLOW_LEVELS: &[&str] = &["none", "low", "medium", "high"];
const LIGHT_RANDOM_EXCLUDED_CURSOR_NAMES: &[&str] = &["snow"];
const REMOVED_CURSOR_NAMES: &[&str] = &["party", RETIRED_CURSOR_NAME];
const RETIRED_CURSOR_NAME: &str = "neon";
const RETIRED_CURSOR_FAMILY: &str = "curated_template";
const RETIRED_CURSOR_REPLACEMENT_NAME: &str = "cosmic";
const RETIRED_CURSOR_REPLACEMENT_COLOR: &str = "#c761f5";
const GHOSTTY_CURSOR_EFFECT_TEMPLATES: &[(&str, &str)] = &[
    ("tail", "cursor_tail.glsl"),
    ("warp", "cursor_warp.glsl"),
    ("ripple", "ripple_cursor.glsl"),
    ("rectangle_boom", "rectangle_boom_cursor.glsl"),
    ("sonic_boom", "sonic_boom_cursor.glsl"),
    ("sweep", "cursor_sweep.glsl"),
    ("ripple_rectangle", "ripple_rectangle_cursor.glsl"),
];
const GHOSTTY_CURSOR_MOVEMENT_EFFECTS: &[&str] = &["tail", "warp", "sweep"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CursorTargetContract {
    pub name: &'static str,
    pub status: &'static str,
    pub emits: &'static [&'static str],
    pub requires: &'static [&'static str],
    pub notes: &'static [&'static str],
}

const CURSOR_TARGET_CONTRACTS: &[CursorTargetContract] = &[
    CursorTargetContract {
        name: "ghostty",
        status: "supported",
        emits: &[
            "ghostty_include",
            "ghostty_palette_shaders",
            "ghostty_effect_shaders",
        ],
        requires: &[
            "iCurrentCursor",
            "iPreviousCursor",
            "iCurrentCursorColor",
            "iTimeCursorChange",
        ],
        notes: &["Standalone `yzc generate ghostty` writes the include and shaders."],
    },
    CursorTargetContract {
        name: "rio-compatible-config",
        status: "supported",
        emits: &["rio_compatible_config"],
        requires: &["colors.cursor"],
        notes: &[
            "`yzc materialize rio-compatible-config` writes a launch-local config for Rio and Rio-derived terminals.",
        ],
    },
    CursorTargetContract {
        name: "mars",
        status: "supported",
        emits: &["ghostty_palette_shaders", "ghostty_effect_shaders"],
        requires: &[
            "MARS_RIO_TRAIL",
            "iYazelixRioTrailActive",
            "iYazelixRioTrailAnimating",
            "iYazelixRioTrailAnimatedCursor",
            "iYazelixRioTrailCorners",
        ],
        notes: &["Yazelix owns launch-scoped config placement; this crate owns shader content."],
    },
    CursorTargetContract {
        name: "rio",
        status: "abi_documented",
        emits: &["ghostty_palette_shaders", "ghostty_effect_shaders"],
        requires: &[
            "MARS_RIO_TRAIL",
            "rio_trail_uniforms",
            "native_cursor_visibility_control",
        ],
        notes: &["Rio-compatible consumers provide the terminal-side uniform ABI."],
    },
    CursorTargetContract {
        name: "ratty",
        status: "experimental_noop",
        emits: &[],
        requires: &["terminal_cursor_effect_surface"],
        notes: &["Ratty has an explicit target slot but no emitted shader/protocol payload yet."],
    },
    CursorTargetContract {
        name: "protocol_cursor_positions",
        status: "documented_noop",
        emits: &[],
        requires: &[
            "editor_cursor_position_events",
            "terminal_multiple_cursor_protocol",
        ],
        notes: &["Protocol-backed cursors are separate from Ghostty-compatible shader files."],
    },
];

pub fn cursor_target_contracts() -> &'static [CursorTargetContract] {
    CURSOR_TARGET_CONTRACTS
}

pub fn cursor_target_contract(name: &str) -> Option<&'static CursorTargetContract> {
    CURSOR_TARGET_CONTRACTS
        .iter()
        .find(|target| target.name == name)
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorErrorClass {
    Usage,
    Config,
    Io,
    Runtime,
    Internal,
}

impl CursorErrorClass {
    pub fn exit_code(self) -> i32 {
        match self {
            CursorErrorClass::Usage => 64,
            CursorErrorClass::Config => 65,
            CursorErrorClass::Io => 66,
            CursorErrorClass::Runtime | CursorErrorClass::Internal => 70,
        }
    }
}

#[derive(Debug, Error)]
pub enum CursorError {
    #[error("{message}")]
    Classified {
        class: CursorErrorClass,
        code: String,
        message: String,
        remediation: String,
        details: Value,
    },
    #[error("{message}: {source}")]
    Io {
        code: String,
        message: String,
        remediation: String,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{message}: {source}")]
    Toml {
        code: String,
        message: String,
        remediation: String,
        path: String,
        #[source]
        source: Box<toml::de::Error>,
    },
}

impl CursorError {
    pub fn classified(
        class: CursorErrorClass,
        code: impl Into<String>,
        message: impl Into<String>,
        remediation: impl Into<String>,
        details: Value,
    ) -> Self {
        Self::Classified {
            class,
            code: code.into(),
            message: message.into(),
            remediation: remediation.into(),
            details,
        }
    }

    pub fn io(
        code: impl Into<String>,
        message: impl Into<String>,
        remediation: impl Into<String>,
        path: impl Into<String>,
        source: io::Error,
    ) -> Self {
        Self::Io {
            code: code.into(),
            message: message.into(),
            remediation: remediation.into(),
            path: path.into(),
            source,
        }
    }

    pub fn toml(
        code: impl Into<String>,
        message: impl Into<String>,
        remediation: impl Into<String>,
        path: impl Into<String>,
        source: toml::de::Error,
    ) -> Self {
        Self::Toml {
            code: code.into(),
            message: message.into(),
            remediation: remediation.into(),
            path: path.into(),
            source: Box::new(source),
        }
    }

    pub fn class(&self) -> CursorErrorClass {
        match self {
            Self::Classified { class, .. } => *class,
            Self::Io { .. } => CursorErrorClass::Io,
            Self::Toml { .. } => CursorErrorClass::Config,
        }
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Classified { code, .. } => code,
            Self::Io { code, .. } => code,
            Self::Toml { code, .. } => code,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Classified { message, .. } => message.clone(),
            Self::Io {
                message, source, ..
            } => format!("{message}: {source}"),
            Self::Toml {
                message, source, ..
            } => format!("{message}: {source}"),
        }
    }

    pub fn remediation(&self) -> String {
        match self {
            Self::Classified { remediation, .. } => remediation.clone(),
            Self::Io { remediation, .. } => remediation.clone(),
            Self::Toml { remediation, .. } => remediation.clone(),
        }
    }

    pub fn details(&self) -> Value {
        match self {
            Self::Classified { details, .. } => details.clone(),
            Self::Io { path, .. } | Self::Toml { path, .. } => json!({ "path": path }),
        }
    }
}

pub fn parse_jsonc_value(path: &Path, raw: &str) -> Result<Value, CursorError> {
    jsonc_parser::parse_to_serde_value::<Value>(
        raw,
        &jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_loose_object_property_names: false,
            allow_trailing_commas: true,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
        },
    )
    .map_err(|source| {
        CursorError::classified(
            CursorErrorClass::Config,
            "invalid_cursor_settings_jsonc",
            format!(
                "Could not parse Yazelix cursor settings JSONC at {}: {source}.",
                path.display()
            ),
            "Fix the JSONC syntax in settings.jsonc and retry. Comments must use `//` or `/* ... */`, not `#`.",
            json!({
                "path": path.display().to_string(),
                "error": source.to_string(),
            }),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSettingsMigration {
    pub text: String,
    pub changed_paths: Vec<String>,
}

impl CursorSettingsMigration {
    pub fn changed(&self) -> bool {
        !self.changed_paths.is_empty()
    }
}

pub fn migrate_cursor_settings_jsonc_text(
    path: &Path,
    raw: &str,
) -> Result<CursorSettingsMigration, CursorError> {
    let initial = parse_jsonc_value(path, raw)?;
    let needs_replacement_definition = cursor_config_references_retired_cursor(&initial);
    let mut operations = vec![
        MigrationOp::Transform {
            path: "enabled_cursors".to_string(),
            transform: transform_enabled_cursors_without_retired,
        },
        MigrationOp::Transform {
            path: "settings.trail".to_string(),
            transform: transform_retired_trail_selection,
        },
        MigrationOp::Transform {
            path: "cursor".to_string(),
            transform: remove_retired_cursor_definitions,
        },
    ];
    if needs_replacement_definition {
        operations.push(MigrationOp::AddDefault {
            path: "cursor".to_string(),
            value: json!([retired_cursor_replacement_definition()]),
        });
        operations.push(MigrationOp::Transform {
            path: "cursor".to_string(),
            transform: append_replacement_cursor_definition,
        });
    }

    let outcome = apply_migrations_text(raw, &operations)
        .map_err(|source| cursor_settings_migration_error(path, source))?;
    Ok(CursorSettingsMigration {
        text: outcome.text,
        changed_paths: migration_changed_paths(&outcome.mutations),
    })
}

pub fn parse_cursor_settings_jsonc_text(
    path: &Path,
    raw: &str,
) -> Result<(CursorRegistry, CursorSettingsMigration), CursorError> {
    let migration = migrate_cursor_settings_jsonc_text(path, raw)?;
    let value = parse_jsonc_value(path, &migration.text)?;
    let registry = CursorRegistry::parse_json_value(path, value)?;
    Ok((registry, migration))
}

pub fn load_cursor_settings_jsonc(
    path: &Path,
) -> Result<(CursorRegistry, CursorSettingsMigration), CursorError> {
    let raw = fs::read_to_string(path).map_err(|source| {
        CursorError::io(
            "read_cursor_settings_jsonc",
            "Could not read Yazelix cursor settings.jsonc",
            "Run `yzc init`, or restore ~/.config/yazelix_cursors/settings.jsonc, then retry.",
            path.to_string_lossy(),
            source,
        )
    })?;
    parse_cursor_settings_jsonc_text(path, &raw)
}

pub fn load_cursor_config(path: &Path) -> Result<CursorRegistry, CursorError> {
    let raw = fs::read_to_string(path).map_err(|source| {
        CursorError::io(
            "read_cursor_config_toml",
            "Could not read Yazelix cursor config",
            "Run `yzc init`, or restore cursors.toml, then retry.",
            path.to_string_lossy(),
            source,
        )
    })?;
    CursorRegistry::parse_str(path, &raw)
}

pub fn initialize_cursor_config(path: &Path) -> Result<bool, CursorError> {
    if path_entry_exists(path)? {
        return Ok(false);
    }
    CursorRegistry::parse_str(path, DEFAULT_CURSOR_CONFIG_TEMPLATE)?;
    write_cursor_config_atomic(path, DEFAULT_CURSOR_CONFIG_TEMPLATE)?;
    Ok(true)
}

pub fn import_cursor_settings_jsonc(
    legacy_path: &Path,
    config_path: &Path,
) -> Result<PathBuf, CursorError> {
    if path_entry_exists(config_path)? {
        return Err(CursorError::classified(
            CursorErrorClass::Config,
            "cursor_config_import_target_exists",
            format!(
                "Refusing to replace existing cursor config at {}.",
                config_path.display()
            ),
            "Keep cursors.toml as the active config, or move it aside before importing settings.jsonc.",
            json!({ "path": config_path.display().to_string() }),
        ));
    }
    if fs::metadata(legacy_path)
        .map_err(|source| {
            CursorError::io(
                "read_legacy_cursor_settings_metadata",
                "Could not inspect legacy Yazelix cursor settings",
                "Restore settings.jsonc or move it aside, then retry.",
                legacy_path.to_string_lossy(),
                source,
            )
        })?
        .permissions()
        .readonly()
    {
        return Err(CursorError::classified(
            CursorErrorClass::Config,
            "read_only_legacy_cursor_settings",
            format!(
                "Legacy cursor settings are read-only at {}.",
                legacy_path.display()
            ),
            "Make settings.jsonc writable or copy it to a writable Yazelix Cursors config directory, then retry.",
            json!({ "path": legacy_path.display().to_string() }),
        ));
    }

    let raw = fs::read_to_string(legacy_path).map_err(|source| {
        CursorError::io(
            "read_legacy_cursor_settings_jsonc",
            "Could not read legacy Yazelix cursor settings.jsonc",
            "Restore settings.jsonc or move it aside, then retry.",
            legacy_path.to_string_lossy(),
            source,
        )
    })?;
    let migration = migrate_cursor_settings_jsonc_text(legacy_path, &raw)?;
    let value = parse_jsonc_value(legacy_path, &migration.text)?;
    CursorRegistry::parse_json_value(legacy_path, value.clone())?;
    let rendered = toml::to_string_pretty(&value).map_err(|source| {
        CursorError::classified(
            CursorErrorClass::Internal,
            "render_imported_cursor_config_toml",
            "Could not render imported cursor settings as TOML.",
            "Report this Yazelix Cursors bug.",
            json!({ "error": source.to_string() }),
        )
    })?;
    CursorRegistry::parse_str(config_path, &rendered)?;

    let backup_path = cursor_settings_backup_path(legacy_path);
    fs::copy(legacy_path, &backup_path).map_err(|source| {
        cursor_config_io(
            "backup_cursor_settings_jsonc_before_import",
            "Could not back up legacy Yazelix cursor settings.jsonc before import",
            &backup_path,
            source,
        )
    })?;
    write_cursor_config_atomic(config_path, &rendered)?;
    Ok(backup_path)
}

fn path_entry_exists(path: &Path) -> Result<bool, CursorError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(cursor_config_io(
            "inspect_cursor_config_path",
            "Could not inspect the Yazelix cursor config path",
            path,
            source,
        )),
    }
}

fn write_cursor_config_atomic(path: &Path, raw: &str) -> Result<(), CursorError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| {
        cursor_config_io(
            "create_cursor_config_dir",
            "Could not create the Yazelix cursor config directory",
            parent,
            source,
        )
    })?;
    let temp_path = cursor_settings_temp_path(path);
    fs::write(&temp_path, raw).map_err(|source| {
        cursor_config_io(
            "write_cursor_config_temp",
            "Could not write temporary Yazelix cursor config",
            &temp_path,
            source,
        )
    })?;
    if let Err(source) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(cursor_config_io(
            "replace_cursor_config_toml",
            "Could not install the Yazelix cursor config",
            path,
            source,
        ));
    }
    Ok(())
}

pub fn persist_migrated_cursor_settings_jsonc(
    path: &Path,
    migration: &CursorSettingsMigration,
) -> Result<Option<PathBuf>, CursorError> {
    if !migration.changed() {
        return Ok(None);
    }

    let backup_path = cursor_settings_backup_path(path);
    fs::copy(path, &backup_path).map_err(|source| {
        CursorError::io(
            "backup_cursor_settings_jsonc_before_migration",
            "Could not back up Yazelix cursor settings.jsonc before migration",
            "Check permissions for ~/.config/yazelix_cursors and retry.",
            backup_path.to_string_lossy(),
            source,
        )
    })?;

    let temp_path = cursor_settings_temp_path(path);
    fs::write(&temp_path, &migration.text).map_err(|source| {
        CursorError::io(
            "write_migrated_cursor_settings_jsonc",
            "Could not write migrated Yazelix cursor settings.jsonc",
            "Check permissions for ~/.config/yazelix_cursors and retry.",
            temp_path.to_string_lossy(),
            source,
        )
    })?;
    if let Err(source) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(CursorError::io(
            "replace_migrated_cursor_settings_jsonc",
            "Could not replace Yazelix cursor settings.jsonc with the migrated version",
            "Check permissions for ~/.config/yazelix_cursors and retry.",
            path.to_string_lossy(),
            source,
        ));
    }

    Ok(Some(backup_path))
}

fn cursor_config_io(code: &str, message: &str, path: &Path, source: io::Error) -> CursorError {
    CursorError::io(
        code,
        message,
        "Check permissions for the cursor config directory and retry.",
        path.to_string_lossy(),
        source,
    )
}

fn transform_enabled_cursors_without_retired(value: &Value) -> Result<Option<Value>, String> {
    let Some(items) = value.as_array() else {
        return Ok(None);
    };
    let mut removed = false;
    let mut kept = Vec::new();
    for item in items {
        if value_is_retired_cursor_name(item) {
            removed = true;
        } else {
            kept.push(item.clone());
        }
    }
    if !removed {
        return Ok(None);
    }
    if kept.is_empty() {
        kept.push(json!(RETIRED_CURSOR_REPLACEMENT_NAME));
    }
    Ok(Some(Value::Array(kept)))
}

fn transform_retired_trail_selection(value: &Value) -> Result<Option<Value>, String> {
    if value_is_retired_cursor_name(value) {
        Ok(Some(json!(RETIRED_CURSOR_REPLACEMENT_NAME)))
    } else {
        Ok(None)
    }
}

fn remove_retired_cursor_definitions(value: &Value) -> Result<Option<Value>, String> {
    let Some(definitions) = value.as_array() else {
        return Ok(None);
    };
    let kept = definitions
        .iter()
        .filter(|definition| !cursor_definition_is_retired(definition))
        .cloned()
        .collect::<Vec<_>>();
    if kept.len() == definitions.len() {
        Ok(None)
    } else {
        Ok(Some(Value::Array(kept)))
    }
}

fn append_replacement_cursor_definition(value: &Value) -> Result<Option<Value>, String> {
    let Some(definitions) = value.as_array() else {
        return Ok(None);
    };
    if definitions.iter().any(|definition| {
        definition
            .get("name")
            .is_some_and(|name| value_matches_cursor_name(name, RETIRED_CURSOR_REPLACEMENT_NAME))
    }) {
        return Ok(None);
    }
    let mut next = definitions.clone();
    next.push(retired_cursor_replacement_definition());
    Ok(Some(Value::Array(next)))
}

fn retired_cursor_replacement_definition() -> Value {
    json!({
        "name": RETIRED_CURSOR_REPLACEMENT_NAME,
        "family": "mono",
        "color": RETIRED_CURSOR_REPLACEMENT_COLOR,
    })
}

fn cursor_config_references_retired_cursor(value: &Value) -> bool {
    value
        .get("enabled_cursors")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(value_is_retired_cursor_name))
        || value
            .get("settings")
            .and_then(|settings| settings.get("trail"))
            .is_some_and(value_is_retired_cursor_name)
        || value
            .get("cursor")
            .and_then(Value::as_array)
            .is_some_and(|definitions| definitions.iter().any(cursor_definition_is_retired))
}

fn cursor_definition_is_retired(value: &Value) -> bool {
    value.get("name").is_some_and(value_is_retired_cursor_name)
        || value
            .get("family")
            .is_some_and(|family| value_matches_cursor_name(family, RETIRED_CURSOR_FAMILY))
}

fn value_is_retired_cursor_name(value: &Value) -> bool {
    value_matches_cursor_name(value, RETIRED_CURSOR_NAME)
}

fn value_matches_cursor_name(value: &Value, expected: &str) -> bool {
    value
        .as_str()
        .map(|raw| raw.trim().eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn migration_changed_paths(mutations: &[MigrationMutation]) -> Vec<String> {
    mutations
        .iter()
        .map(|mutation| match mutation {
            MigrationMutation::Renamed { from, to } => format!("{from}->{to}"),
            MigrationMutation::Deleted { path }
            | MigrationMutation::AddedDefault { path }
            | MigrationMutation::Transformed { path } => path.clone(),
        })
        .collect()
}

fn cursor_settings_migration_error(path: &Path, source: MigrationError) -> CursorError {
    CursorError::classified(
        CursorErrorClass::Config,
        "migrate_cursor_settings_jsonc",
        format!(
            "Could not migrate Yazelix cursor settings JSONC at {}.",
            path.display()
        ),
        "Fix ~/.config/yazelix_cursors/settings.jsonc or move it aside and run `yzc init`.",
        json!({
            "path": path.display().to_string(),
            "error": format!("{source:?}"),
        }),
    )
}

fn cursor_settings_backup_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.backup_before_yazelix_cursors_v2_{}",
        cursor_settings_file_name(path),
        migration_stamp(),
    ))
}

fn cursor_settings_temp_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.tmp_yazelix_cursors_migration_{}_{}",
        cursor_settings_file_name(path),
        process::id(),
        migration_stamp(),
    ))
}

fn cursor_settings_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(LEGACY_STANDALONE_CURSOR_SETTINGS_FILENAME)
        .to_string()
}

fn migration_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CursorRegistry {
    pub schema_version: u32,
    pub enabled_cursors: Vec<String>,
    pub settings: CursorSettings,
    pub definitions: BTreeMap<String, CursorDefinition>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CursorSettings {
    pub trail: String,
    pub trail_effect: String,
    pub mode_effect: String,
    pub glow: String,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CursorDefinition {
    pub name: String,
    pub family: CursorFamily,
    pub colors: Vec<CursorColor>,
    pub divider: Option<SplitDivider>,
    pub transition: Option<SplitTransition>,
    pub cursor_color: CursorColor,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorFamily {
    Mono,
    Split,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SplitDivider {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SplitTransition {
    Soft,
    Hard,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CursorColor {
    pub hex: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResolvedCursorRegistryState {
    pub selected_cursor: Option<CursorDefinition>,
    pub trail_disabled: bool,
    pub selected_trail_effect: Option<String>,
    pub selected_mode_effect: Option<String>,
    pub duration: f64,
    pub glow: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCursorRegistry {
    schema_version: u32,
    enabled_cursors: Vec<String>,
    settings: RawCursorSettings,
    #[serde(default)]
    cursor: Vec<RawCursorDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCursorSettings {
    trail: String,
    trail_effect: String,
    mode_effect: String,
    glow: String,
    duration: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCursorDefinition {
    name: String,
    family: String,
    color: Option<String>,
    accent_color: Option<String>,
    #[serde(default)]
    colors: Vec<String>,
    divider: Option<String>,
    transition: Option<String>,
    template: Option<String>,
    cursor_color: Option<String>,
}

impl CursorRegistry {
    pub fn parse_json_value(path: &Path, cursors: Value) -> Result<Self, CursorError> {
        let parsed = serde_json::from_value::<RawCursorRegistry>(cursors).map_err(|source| {
            CursorError::classified(
                CursorErrorClass::Config,
                "invalid_cursor_registry_json",
                format!(
                    "Could not parse Yazelix cursor registry JSON in {}.",
                    path.display()
                ),
                "Fix the cursor registry data and retry.",
                json!({
                    "path": path.display().to_string(),
                    "error": source.to_string(),
                }),
            )
        })?;
        CursorRegistry::from_raw(path, parsed)
    }

    pub fn parse_str(path: &Path, raw: &str) -> Result<Self, CursorError> {
        let parsed = toml::from_str::<RawCursorRegistry>(raw).map_err(|source| {
            CursorError::toml(
                "invalid_cursor_config_toml",
                "Could not parse Yazelix cursor config",
                "Fix the cursor registry data and retry.",
                path.to_string_lossy(),
                source,
            )
        })?;
        CursorRegistry::from_raw(path, parsed)
    }

    pub fn enabled_definitions(&self) -> Vec<&CursorDefinition> {
        self.enabled_cursors
            .iter()
            .filter_map(|name| self.definitions.get(name))
            .collect()
    }

    pub fn is_random_request(&self) -> bool {
        self.settings.trail == "random"
            || self.settings.trail_effect == "random"
            || self.settings.mode_effect == "random"
    }

    pub fn resolve(&self) -> ResolvedCursorRegistryState {
        let entropy = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as usize)
            .unwrap_or(0);
        self.resolve_with_entropy(entropy)
    }

    pub fn resolve_with_entropy(&self, entropy: usize) -> ResolvedCursorRegistryState {
        self.resolve_with_entropy_for_appearance(entropy, "dark")
    }

    pub fn resolve_for_appearance(&self, appearance_mode: &str) -> ResolvedCursorRegistryState {
        let entropy = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as usize)
            .unwrap_or(0);
        self.resolve_with_entropy_for_appearance(entropy, appearance_mode)
    }

    pub fn resolve_with_entropy_for_appearance(
        &self,
        entropy: usize,
        appearance_mode: &str,
    ) -> ResolvedCursorRegistryState {
        let selected_cursor = match self.settings.trail.as_str() {
            "none" => None,
            "random" => self.random_cursor_name_for_appearance(entropy, appearance_mode),
            name => self.definitions.get(name).cloned(),
        };

        ResolvedCursorRegistryState {
            selected_cursor,
            trail_disabled: self.settings.trail == "none",
            selected_trail_effect: resolve_optional_effect(
                &self.settings.trail_effect,
                SUPPORTED_TRAIL_EFFECTS,
                entropy,
            ),
            selected_mode_effect: resolve_optional_effect(
                &self.settings.mode_effect,
                SUPPORTED_MODE_EFFECTS,
                entropy / 17,
            ),
            duration: self.settings.duration,
            glow: self.settings.glow.clone(),
        }
    }

    fn random_cursor_name_for_appearance(
        &self,
        entropy: usize,
        appearance_mode: &str,
    ) -> Option<CursorDefinition> {
        let pool = self.random_cursor_pool_for_appearance(appearance_mode);
        pool.get(entropy % pool.len())
            .and_then(|name| self.definitions.get(*name))
            .cloned()
    }

    fn random_cursor_pool_for_appearance(&self, appearance_mode: &str) -> Vec<&str> {
        if !light_safe_random_pool(appearance_mode) {
            return self.enabled_cursors.iter().map(String::as_str).collect();
        }

        let filtered = self
            .enabled_cursors
            .iter()
            .map(String::as_str)
            .filter(|name| {
                !LIGHT_RANDOM_EXCLUDED_CURSOR_NAMES
                    .iter()
                    .any(|excluded| name.eq_ignore_ascii_case(excluded))
            })
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            self.enabled_cursors.iter().map(String::as_str).collect()
        } else {
            filtered
        }
    }

    fn from_raw(path: &Path, raw: RawCursorRegistry) -> Result<Self, CursorError> {
        if raw.schema_version != 1 {
            return Err(invalid_cursor_config(
                path,
                "schema_version",
                format!(
                    "Unsupported cursor config schema_version {}. Expected 1.",
                    raw.schema_version
                ),
            ));
        }

        let mut enabled_seen = BTreeSet::new();
        let mut enabled_cursors = Vec::new();
        for name in raw.enabled_cursors {
            let normalized = validate_cursor_name(path, "enabled_cursors", &name)?;
            if !enabled_seen.insert(normalized.clone()) {
                return Err(invalid_cursor_config(
                    path,
                    "enabled_cursors",
                    format!("Cursor '{normalized}' is listed more than once in enabled_cursors."),
                ));
            }
            enabled_cursors.push(normalized);
        }
        if enabled_cursors.is_empty() {
            return Err(invalid_cursor_config(
                path,
                "enabled_cursors",
                "enabled_cursors must contain at least one cursor name.".to_string(),
            ));
        }

        let settings = validate_settings(path, raw.settings, &enabled_cursors)?;
        let mut definitions = BTreeMap::new();
        for raw_definition in raw.cursor {
            let definition = validate_definition(path, raw_definition)?;
            if definitions
                .insert(definition.name.clone(), definition.clone())
                .is_some()
            {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.name",
                    format!("Cursor '{}' is defined more than once.", definition.name),
                ));
            }
        }

        for enabled in &enabled_cursors {
            if !definitions.contains_key(enabled) {
                return Err(invalid_cursor_config(
                    path,
                    "enabled_cursors",
                    format!(
                        "enabled_cursors references '{enabled}', but no matching [[cursor]] table exists."
                    ),
                ));
            }
        }

        Ok(CursorRegistry {
            schema_version: raw.schema_version,
            enabled_cursors,
            settings,
            definitions,
        })
    }
}

impl CursorDefinition {
    pub fn shader_path(&self) -> String {
        format!("./shaders/cursor_trail_{}.glsl", self.name)
    }

    pub fn cursor_color_hex(&self) -> &str {
        &self.cursor_color.hex
    }

    pub fn family_name(&self) -> &'static str {
        self.family.as_str()
    }

    pub fn divider_name(&self) -> Option<&'static str> {
        self.divider.map(|divider| divider.as_str())
    }

    pub fn split_primary_color_hex(&self) -> Option<&str> {
        matches!(self.family, CursorFamily::Split)
            .then(|| self.colors.first().map(|color| color.hex.as_str()))
            .flatten()
    }

    pub fn split_secondary_color_hex(&self) -> Option<&str> {
        matches!(self.family, CursorFamily::Split)
            .then(|| self.colors.get(1).map(|color| color.hex.as_str()))
            .flatten()
    }

    pub fn cursor_color_literal(&self) -> String {
        self.cursor_color.glsl_vec4()
    }
}

fn light_safe_random_pool(appearance_mode: &str) -> bool {
    matches!(
        appearance_mode.trim().to_ascii_lowercase().as_str(),
        "light" | "auto"
    )
}

impl CursorFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            CursorFamily::Mono => "mono",
            CursorFamily::Split => "split",
        }
    }
}

impl SplitDivider {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitDivider::Vertical => "vertical",
            SplitDivider::Horizontal => "horizontal",
        }
    }
}

impl SplitTransition {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitTransition::Soft => "soft",
            SplitTransition::Hard => "hard",
        }
    }
}

impl CursorColor {
    pub fn glsl_vec4(&self) -> String {
        let bytes = self.rgb_bytes();
        format!(
            "vec4({:.3}, {:.3}, {:.3}, 1.0)",
            bytes[0] as f64 / 255.0,
            bytes[1] as f64 / 255.0,
            bytes[2] as f64 / 255.0
        )
    }

    fn rgb_bytes(&self) -> [u8; 3] {
        [
            u8::from_str_radix(&self.hex[1..3], 16).unwrap_or(0),
            u8::from_str_radix(&self.hex[3..5], 16).unwrap_or(0),
            u8::from_str_radix(&self.hex[5..7], 16).unwrap_or(0),
        ]
    }
}

pub fn render_cursor_settings_jsonc(registry: &CursorRegistry) -> String {
    let mut out = String::new();
    out.push_str("// Yazelix Cursors settings\n");
    out.push_str("// Edit this file through `yzx config ui`, `yzc init`, or your editor.\n");
    out.push_str("// In Ghostty standalone setups, add: config-file = ~/.config/yazelix_cursors/ghostty.conf\n");
    out.push_str("{\n");
    out.push_str(&format!(
        "  \"schema_version\": {},\n",
        registry.schema_version
    ));
    out.push_str("  \"enabled_cursors\": [\n");
    for (index, name) in registry.enabled_cursors.iter().enumerate() {
        let comma = if index + 1 == registry.enabled_cursors.len() {
            ""
        } else {
            ","
        };
        out.push_str(&format!("    \"{name}\"{comma}\n"));
    }
    out.push_str("  ],\n");
    out.push_str("  \"settings\": {\n");
    out.push_str(&format!(
        "    \"trail\": \"{}\",\n",
        registry.settings.trail
    ));
    out.push_str(&format!(
        "    \"trail_effect\": \"{}\",\n",
        registry.settings.trail_effect
    ));
    out.push_str(&format!(
        "    \"mode_effect\": \"{}\",\n",
        registry.settings.mode_effect
    ));
    out.push_str(&format!("    \"glow\": \"{}\",\n", registry.settings.glow));
    out.push_str(&format!(
        "    \"duration\": {}\n",
        format_ghostty_trail_duration(registry.settings.duration)
    ));
    out.push_str("  },\n");
    out.push_str("  \"cursor\": [\n");
    let definitions = registry.enabled_definitions();
    for (index, definition) in definitions.iter().enumerate() {
        let comma = if index + 1 == definitions.len() {
            ""
        } else {
            ","
        };
        out.push_str(&render_cursor_definition_jsonc(definition, comma));
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn render_cursor_definition_jsonc(definition: &CursorDefinition, comma: &str) -> String {
    let mut out = String::new();
    out.push_str("    {\n");
    out.push_str(&format!("      \"name\": \"{}\",\n", definition.name));
    out.push_str(&format!(
        "      \"family\": \"{}\",\n",
        definition.family.as_str()
    ));
    match definition.family {
        CursorFamily::Mono => {
            out.push_str(&format!(
                "      \"color\": \"{}\",\n",
                definition.colors[0].hex
            ));
            out.push_str(&format!(
                "      \"accent_color\": \"{}\",\n",
                definition.colors[1].hex
            ));
        }
        CursorFamily::Split => {
            let divider = definition
                .divider
                .expect("validated split cursor definitions always have a divider");
            let transition = definition
                .transition
                .expect("validated split cursor definitions always have a transition");
            out.push_str(&format!("      \"divider\": \"{}\",\n", divider.as_str()));
            out.push_str(&format!(
                "      \"transition\": \"{}\",\n",
                transition.as_str()
            ));
            out.push_str("      \"colors\": [\n");
            out.push_str(&format!("        \"{}\",\n", definition.colors[0].hex));
            out.push_str(&format!("        \"{}\"\n", definition.colors[1].hex));
            out.push_str("      ],\n");
        }
    }
    out.push_str(&format!(
        "      \"cursor_color\": \"{}\"\n",
        definition.cursor_color.hex
    ));
    out.push_str(&format!("    }}{comma}\n"));
    out
}

fn validate_settings(
    path: &Path,
    raw: RawCursorSettings,
    enabled_cursors: &[String],
) -> Result<CursorSettings, CursorError> {
    let trail = raw.trail.trim().to_ascii_lowercase();
    if trail != "none" && trail != "random" && !enabled_cursors.contains(&trail) {
        return Err(invalid_cursor_config(
            path,
            "settings.trail",
            format!(
                "settings.trail is '{trail}', but it must be \"none\", \"random\", or a name from enabled_cursors."
            ),
        ));
    }

    let trail_effect = validate_optional_setting(
        path,
        "settings.trail_effect",
        &raw.trail_effect,
        SUPPORTED_TRAIL_EFFECTS,
    )?;
    let mode_effect = validate_optional_setting(
        path,
        "settings.mode_effect",
        &raw.mode_effect,
        SUPPORTED_MODE_EFFECTS,
    )?;
    let glow = validate_required_setting(path, "settings.glow", &raw.glow, SUPPORTED_GLOW_LEVELS)?;
    if !raw.duration.is_finite()
        || !(GHOSTTY_TRAIL_DURATION_MIN..=GHOSTTY_TRAIL_DURATION_MAX).contains(&raw.duration)
    {
        return Err(invalid_cursor_config(
            path,
            "settings.duration",
            format!(
                "settings.duration is {}. Expected a number from {} to {}.",
                raw.duration, GHOSTTY_TRAIL_DURATION_MIN, GHOSTTY_TRAIL_DURATION_MAX
            ),
        ));
    }

    Ok(CursorSettings {
        trail,
        trail_effect,
        mode_effect,
        glow,
        duration: raw.duration,
    })
}

fn validate_definition(
    path: &Path,
    raw: RawCursorDefinition,
) -> Result<CursorDefinition, CursorError> {
    let name = validate_cursor_name(path, "cursor.name", &raw.name)?;
    if REMOVED_CURSOR_NAMES.contains(&name.as_str()) {
        return Err(invalid_cursor_config(
            path,
            "cursor.name",
            format!("Cursor '{name}' is not supported. Remove it from the cursor registry."),
        ));
    }

    let family = match raw.family.trim() {
        "mono" => CursorFamily::Mono,
        "split" => CursorFamily::Split,
        other => {
            return Err(invalid_cursor_config(
                path,
                "cursor.family",
                format!(
                    "Cursor '{name}' uses unsupported family '{other}'. Expected mono or split."
                ),
            ));
        }
    };

    match family {
        CursorFamily::Mono => {
            if !raw.colors.is_empty() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.colors",
                    format!("Cursor '{name}' uses mono and must not define colors."),
                ));
            }
            if raw.divider.is_some() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.divider",
                    format!("Cursor '{name}' uses mono and must not set divider."),
                ));
            }
            if raw.transition.is_some() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.transition",
                    format!("Cursor '{name}' uses mono and must not set transition."),
                ));
            }
            if raw.template.is_some() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.template",
                    format!("Cursor '{name}' is data-driven and must not set template."),
                ));
            }
            let color = raw.color.as_deref().ok_or_else(|| {
                invalid_cursor_config(
                    path,
                    "cursor.color",
                    format!("Cursor '{name}' uses mono and must set color."),
                )
            })?;
            let base_color = validate_color(path, &format!("cursor.{name}.color"), color)?;
            let accent_color = match raw.accent_color.as_deref() {
                Some(accent) => {
                    validate_color(path, &format!("cursor.{name}.accent_color"), accent)?
                }
                None => derive_accent_color(&base_color),
            };
            let cursor_color = match raw.cursor_color.as_deref() {
                Some(cursor_color) => {
                    validate_color(path, &format!("cursor.{name}.cursor_color"), cursor_color)?
                }
                None => base_color.clone(),
            };

            Ok(CursorDefinition {
                name,
                family,
                colors: vec![base_color, accent_color],
                divider: None,
                transition: None,
                cursor_color,
            })
        }
        CursorFamily::Split => {
            if raw.color.is_some() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.color",
                    format!("Cursor '{name}' uses split and must not set color."),
                ));
            }
            if raw.accent_color.is_some() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.accent_color",
                    format!("Cursor '{name}' uses split and must not set accent_color."),
                ));
            }
            if raw.colors.len() != 2 {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.colors",
                    format!("Cursor '{name}' uses split and must define exactly 2 colors."),
                ));
            }
            if raw.template.is_some() {
                return Err(invalid_cursor_config(
                    path,
                    "cursor.template",
                    format!("Cursor '{name}' is data-driven and must not set template."),
                ));
            }
            let divider = validate_split_divider(path, &name, raw.divider.as_deref())?;
            let transition = validate_split_transition(path, &name, raw.transition.as_deref())?;
            let colors = raw
                .colors
                .iter()
                .enumerate()
                .map(|(index, color)| {
                    validate_color(path, &format!("cursor.{name}.colors[{index}]"), color)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let cursor_color = match raw.cursor_color.as_deref() {
                Some(cursor_color) => {
                    validate_color(path, &format!("cursor.{name}.cursor_color"), cursor_color)?
                }
                None => colors[0].clone(),
            };

            Ok(CursorDefinition {
                name,
                family,
                colors,
                divider: Some(divider),
                transition: Some(transition),
                cursor_color,
            })
        }
    }
}

fn validate_split_divider(
    path: &Path,
    name: &str,
    raw_divider: Option<&str>,
) -> Result<SplitDivider, CursorError> {
    let Some(divider) = raw_divider.map(str::trim) else {
        return Err(invalid_cursor_config(
            path,
            "cursor.divider",
            format!("Cursor '{name}' uses split and must set divider to vertical or horizontal."),
        ));
    };

    match divider {
        "vertical" => Ok(SplitDivider::Vertical),
        "horizontal" => Ok(SplitDivider::Horizontal),
        other => Err(invalid_cursor_config(
            path,
            "cursor.divider",
            format!(
                "Cursor '{name}' uses unsupported split divider '{other}'. Expected vertical or horizontal."
            ),
        )),
    }
}

fn validate_split_transition(
    path: &Path,
    name: &str,
    raw_transition: Option<&str>,
) -> Result<SplitTransition, CursorError> {
    let Some(transition) = raw_transition.map(str::trim) else {
        return Err(invalid_cursor_config(
            path,
            "cursor.transition",
            format!("Cursor '{name}' uses split and must set transition to soft or hard."),
        ));
    };

    match transition {
        "soft" => Ok(SplitTransition::Soft),
        "hard" => Ok(SplitTransition::Hard),
        other => Err(invalid_cursor_config(
            path,
            "cursor.transition",
            format!(
                "Cursor '{name}' uses unsupported split transition '{other}'. Expected soft or hard."
            ),
        )),
    }
}

fn validate_optional_setting(
    path: &Path,
    field: &str,
    value: &str,
    allowed: &[&str],
) -> Result<String, CursorError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized == "none" || normalized == "random" || allowed.contains(&normalized.as_str()) {
        return Ok(normalized);
    }
    Err(invalid_cursor_config(
        path,
        field,
        format!(
            "{field} is '{normalized}'. Expected none, random, or one of: {}.",
            allowed.join(", ")
        ),
    ))
}

fn validate_required_setting(
    path: &Path,
    field: &str,
    value: &str,
    allowed: &[&str],
) -> Result<String, CursorError> {
    let normalized = value.trim().to_ascii_lowercase();
    if allowed.contains(&normalized.as_str()) {
        return Ok(normalized);
    }
    Err(invalid_cursor_config(
        path,
        field,
        format!(
            "{field} is '{normalized}'. Expected one of: {}.",
            allowed.join(", ")
        ),
    ))
}

fn validate_cursor_name(path: &Path, field: &str, value: &str) -> Result<String, CursorError> {
    let normalized = value.trim().to_ascii_lowercase();
    let valid = !normalized.is_empty()
        && normalized
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if valid {
        return Ok(normalized);
    }
    Err(invalid_cursor_config(
        path,
        field,
        format!(
            "{field} value '{value}' is invalid. Use lowercase letters, digits, and underscores only."
        ),
    ))
}

fn validate_color(path: &Path, field: &str, value: &str) -> Result<CursorColor, CursorError> {
    let normalized = value.trim().to_ascii_lowercase();
    let valid = normalized.len() == 7
        && normalized.starts_with('#')
        && normalized[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
    if valid {
        return Ok(CursorColor { hex: normalized });
    }
    Err(invalid_cursor_config(
        path,
        field,
        format!("{field} value '{value}' is invalid. Use a #rrggbb hex color."),
    ))
}

fn derive_accent_color(base: &CursorColor) -> CursorColor {
    let [red, green, blue] = base.rgb_bytes();
    let (hue, saturation, lightness) = rgb_to_hsl(red, green, blue);
    let (accent_hue, accent_saturation, accent_lightness) = if saturation < 0.08 || lightness > 0.92
    {
        (hue, saturation, lightness * 0.80)
    } else if !(45.0..330.0).contains(&hue) {
        (hue - 22.0, (saturation + 0.05).min(1.0), lightness - 0.06)
    } else if hue < 80.0 {
        (hue - 45.0, saturation, lightness - 0.08)
    } else if hue < 180.0 {
        (hue + 4.0, (saturation + 0.08).min(1.0), lightness - 0.16)
    } else if hue < 250.0 {
        (hue + 8.0, (saturation - 0.08).max(0.0), lightness - 0.15)
    } else {
        (hue - 20.0, (saturation - 0.15).max(0.0), lightness - 0.12)
    };

    let [red, green, blue] = hsl_to_rgb(
        accent_hue,
        accent_saturation,
        accent_lightness.clamp(0.0, 1.0),
    );
    CursorColor {
        hex: format!("#{red:02x}{green:02x}{blue:02x}"),
    }
}

fn rgb_to_hsl(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
    let red = f64::from(red) / 255.0;
    let green = f64::from(green) / 255.0;
    let blue = f64::from(blue) / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta == 0.0 {
        return (0.0, 0.0, lightness);
    }

    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == red {
        60.0 * ((green - blue) / delta).rem_euclid(6.0)
    } else if max == green {
        60.0 * (((blue - red) / delta) + 2.0)
    } else {
        60.0 * (((red - green) / delta) + 4.0)
    };

    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> [u8; 3] {
    let hue = hue.rem_euclid(360.0) / 360.0;
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);

    if saturation == 0.0 {
        let value = float_to_byte(lightness);
        return [value, value, value];
    }

    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - (lightness * saturation)
    };
    let p = (2.0 * lightness) - q;
    [
        float_to_byte(hue_to_rgb(p, q, hue + (1.0 / 3.0))),
        float_to_byte(hue_to_rgb(p, q, hue)),
        float_to_byte(hue_to_rgb(p, q, hue - (1.0 / 3.0))),
    ]
}

fn hue_to_rgb(p: f64, q: f64, hue: f64) -> f64 {
    let hue = hue.rem_euclid(1.0);
    if hue < 1.0 / 6.0 {
        p + (q - p) * 6.0 * hue
    } else if hue < 1.0 / 2.0 {
        q
    } else if hue < 2.0 / 3.0 {
        p + (q - p) * ((2.0 / 3.0) - hue) * 6.0
    } else {
        p
    }
}

fn float_to_byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn resolve_optional_effect(value: &str, allowed: &[&str], entropy: usize) -> Option<String> {
    match value {
        "none" => None,
        "random" => allowed
            .get(entropy % allowed.len())
            .map(|value| value.to_string()),
        other => Some(other.to_string()),
    }
}

pub fn format_ghostty_trail_duration(duration: f64) -> String {
    let mut rendered = format!("{duration:.3}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.push('0');
    }
    rendered
}

pub fn write_ghostty_cursor_palette_shaders(
    shaders_dest: &Path,
    registry: &CursorRegistry,
    glow_level: &str,
    trail_duration: f64,
) -> Result<(), CursorError> {
    let definitions = registry.enabled_definitions();
    if definitions.is_empty() {
        return Ok(());
    }

    let common_path = shaders_dest.join("cursor_trail_common.glsl");
    let common = fs::read_to_string(&common_path).map_err(|source| {
        CursorError::io(
            "read_ghostty_shader_common",
            "Could not read the copied Ghostty cursor shader common library",
            "Reinstall Yazelix so the runtime includes configs/terminal_emulators/ghostty/shaders/cursor_trail_common.glsl.",
            common_path.to_string_lossy(),
            source,
        )
    })?;
    let glow_header = render_trail_glow_header(glow_level);

    for definition in definitions {
        let output_path = shaders_dest.join(format!("cursor_trail_{}.glsl", definition.name));
        let rendered = format!(
            "{}{}\n{}",
            glow_header,
            common,
            render_data_driven_cursor_variant(definition, trail_duration)
        );
        fs::write(&output_path, rendered).map_err(|source| {
            CursorError::io(
                "write_data_driven_cursor_shader",
                "Could not write generated Ghostty cursor shader",
                "Check permissions for the Yazelix state directory and retry.",
                output_path.to_string_lossy(),
                source,
            )
        })?;
    }

    Ok(())
}

pub fn write_ghostty_cursor_effect_shaders(
    shaders_dest: &Path,
    glow_level: &str,
    effect_color_literal: &str,
    trail_duration: f64,
) -> Result<(), CursorError> {
    let templates_dir = shaders_dest.join("upstream_effects");
    if !templates_dir.exists() {
        return Err(CursorError::classified(
            CursorErrorClass::Io,
            "missing_ghostty_effect_templates",
            "Could not find bundled Ghostty cursor effect templates.",
            "Reinstall the yazelix_cursors package so share/yazelix/yazelix_cursors/shaders/upstream_effects exists.",
            json!({ "path": templates_dir.display().to_string() }),
        ));
    }

    let generated_dir = shaders_dest.join("generated_effects");
    if generated_dir.exists() {
        fs::remove_dir_all(&generated_dir).map_err(|source| {
            CursorError::io(
                "remove_generated_ghostty_effect_shaders",
                "Could not remove previous generated Ghostty cursor effect shaders",
                "Check permissions for the generated Yazelix cursor shader directory and retry.",
                generated_dir.to_string_lossy(),
                source,
            )
        })?;
    }
    fs::create_dir_all(&generated_dir).map_err(|source| {
        CursorError::io(
            "create_generated_ghostty_effect_shaders",
            "Could not create generated Ghostty cursor effect shader directory",
            "Check permissions for the generated Yazelix cursor shader directory and retry.",
            generated_dir.to_string_lossy(),
            source,
        )
    })?;

    for (effect, template_name) in GHOSTTY_CURSOR_EFFECT_TEMPLATES {
        let template_path = templates_dir.join(template_name);
        let template = fs::read_to_string(&template_path).map_err(|source| {
            CursorError::io(
                "read_ghostty_effect_template",
                "Could not read bundled Ghostty cursor effect template",
                "Reinstall the yazelix_cursors package and retry.",
                template_path.to_string_lossy(),
                source,
            )
        })?;
        let duration = if GHOSTTY_CURSOR_MOVEMENT_EFFECTS.contains(effect) {
            trail_duration
        } else {
            1.0
        };
        let rendered = render_ghostty_cursor_effect_shader(
            &template,
            glow_level,
            effect_color_literal,
            duration,
        );
        let header = format!(
            "// Generated by Yazelix from a vendored Ghostty cursor effect template\n\
             // Source repository: https://github.com/sahaj-b/ghostty-cursor-shaders\n\
             // Effect: {effect}\n\
             // Color source: {effect_color_literal}\n\
             // cursor settings.glow = {glow_level}\n\
             // cursor settings.duration = {}\n\n",
            format_ghostty_trail_duration(duration)
        );
        let output_path = generated_dir.join(format!("{effect}.glsl"));
        fs::write(&output_path, format!("{header}{rendered}")).map_err(|source| {
            CursorError::io(
                "write_ghostty_effect_shader",
                "Could not write generated Ghostty cursor effect shader",
                "Check permissions for the generated Yazelix cursor shader directory and retry.",
                output_path.to_string_lossy(),
                source,
            )
        })?;
    }

    Ok(())
}

fn render_data_driven_cursor_variant(definition: &CursorDefinition, duration_scale: f64) -> String {
    let color_0 = definition.colors[0].glsl_vec4();
    let color_1 = definition.colors[1].glsl_vec4();
    match definition.family {
        CursorFamily::Mono => {
            let duration = format_ghostty_trail_duration(0.25 * duration_scale);
            format!(
                r#"// Generated Yazelix mono cursor variant

const vec4 YAZELIX_CURSOR_COLOR_0 = {color_0};
const vec4 YAZELIX_CURSOR_COLOR_1 = {color_1};
const float DURATION = {duration};

void mainImage(out vec4 fragColor, in vec2 fragCoord)
{{
    renderMonoColorTrail(fragColor, fragCoord, YAZELIX_CURSOR_COLOR_0, YAZELIX_CURSOR_COLOR_1, DURATION, .007, 1.5);
}}
"#
            )
        }
        CursorFamily::Split => {
            let duration = format_ghostty_trail_duration(0.24 * duration_scale);
            let horizontal = match definition
                .divider
                .expect("validated split cursor definitions always have a divider")
            {
                SplitDivider::Vertical => "0.0",
                SplitDivider::Horizontal => "1.0",
            };
            let transition = match definition
                .transition
                .expect("validated split cursor definitions always have a transition")
            {
                SplitTransition::Soft => "1.0",
                SplitTransition::Hard => "0.0",
            };
            format!(
                r#"// Generated Yazelix split cursor variant

const vec4 YAZELIX_CURSOR_COLOR_0 = {color_0};
const vec4 YAZELIX_CURSOR_COLOR_1 = {color_1};
const float DURATION = {duration};
const float YAZELIX_SPLIT_HORIZONTAL = {horizontal};
const float YAZELIX_SPLIT_BLEND = {transition};

void mainImage(out vec4 fragColor, in vec2 fragCoord)
{{
    renderSplitColorTrail(fragColor, fragCoord, YAZELIX_CURSOR_COLOR_0, YAZELIX_CURSOR_COLOR_1, DURATION, YAZELIX_SPLIT_HORIZONTAL, YAZELIX_SPLIT_BLEND);
}}
"#
            )
        }
    }
}

fn render_trail_glow_header(glow_level: &str) -> String {
    let profile = glow_profile(glow_level);
    format!(
        r#"// Generated by Yazelix with cursor glow = {glow_level}
const float YAZELIX_TRAIL_GLOW_STRENGTH = {};
const float YAZELIX_TRAIL_GLOW_WIDTH_SCALE = {};
const float YAZELIX_CURSOR_GLOW_STRENGTH = {};
const float YAZELIX_CURSOR_GLOW_WIDTH_SCALE = {};
const float YAZELIX_TRAIL_EDGE_WIDTH_SCALE = {};
const float YAZELIX_CURSOR_EDGE_WIDTH_SCALE = {};
const float YAZELIX_TRAIL_CORE_OFFSET_SCALE = {};

"#,
        profile.trail_glow_strength,
        profile.trail_glow_width_scale,
        profile.cursor_glow_strength,
        profile.cursor_glow_width_scale,
        profile.trail_edge_width_scale,
        profile.cursor_edge_width_scale,
        profile.trail_core_offset_scale,
    )
}

fn render_ghostty_cursor_effect_shader(
    template: &str,
    glow_level: &str,
    effect_color_literal: &str,
    duration_scale: f64,
) -> String {
    let profile = glow_profile(glow_level);
    let color_source = normalize_effect_color_literal(effect_color_literal);
    let mut rendered = replace_vec4_assignment(template, "COLOR", &color_source);
    rendered = replace_vec4_assignment(&rendered, "TRAIL_COLOR", &color_source);
    rendered = scale_glsl_float_constant(&rendered, "BLUR", profile.effect_blur_factor);
    rendered = scale_glsl_float_constant(&rendered, "MAX_RADIUS", profile.effect_spread_factor);
    rendered = scale_glsl_float_constant(&rendered, "MAX_SIZE", profile.effect_spread_factor);
    rendered =
        scale_glsl_float_constant(&rendered, "MAX_TRAIL_LENGTH", profile.effect_spread_factor);
    rendered = scale_glsl_float_constant(&rendered, "TRAIL_LENGTH", profile.effect_spread_factor);
    rendered = scale_glsl_float_constant(&rendered, "TRAIL_SIZE", profile.effect_spread_factor);
    rendered = scale_glsl_float_constant(
        &rendered,
        "RING_THICKNESS",
        profile.effect_ring_thickness_factor,
    );
    scale_glsl_float_constant(&rendered, "DURATION", duration_scale)
}

fn normalize_effect_color_literal(effect_color_literal: &str) -> String {
    let trimmed = effect_color_literal.trim();
    if trimmed.is_empty() {
        "iCurrentCursorColor".to_string()
    } else {
        trimmed.to_string()
    }
}

fn replace_vec4_assignment(source: &str, constant_name: &str, value: &str) -> String {
    rewrite_assignment_line(source, &format!("vec4 {constant_name} = "), value)
}

fn scale_glsl_float_constant(source: &str, constant_name: &str, factor: f64) -> String {
    let prefix = format!("const float {constant_name} = ");
    rewrite_assignment_line_with_value(source, &prefix, |value| {
        parse_leading_float(value)
            .map(|parsed| format_ghostty_trail_duration(parsed * factor))
            .unwrap_or_else(|| value.trim().to_string())
    })
}

fn rewrite_assignment_line(source: &str, prefix: &str, value: &str) -> String {
    rewrite_assignment_line_with_value(source, prefix, |_| value.to_string())
}

fn rewrite_assignment_line_with_value(
    source: &str,
    prefix: &str,
    rewrite: impl Fn(&str) -> String,
) -> String {
    source
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent_len = line.len() - trimmed.len();
            let Some(rest) = trimmed.strip_prefix(prefix) else {
                return line.to_string();
            };
            let Some(semicolon_index) = rest.find(';') else {
                return line.to_string();
            };
            let (value, suffix) = rest.split_at(semicolon_index);
            format!(
                "{}{}{}{}",
                &line[..indent_len],
                prefix,
                rewrite(value),
                suffix
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_leading_float(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

struct GlowProfile {
    trail_glow_strength: &'static str,
    cursor_glow_strength: &'static str,
    trail_edge_width_scale: &'static str,
    cursor_edge_width_scale: &'static str,
    trail_core_offset_scale: &'static str,
    trail_glow_width_scale: &'static str,
    cursor_glow_width_scale: &'static str,
    effect_blur_factor: f64,
    effect_spread_factor: f64,
    effect_ring_thickness_factor: f64,
}

fn glow_profile(glow_level: &str) -> GlowProfile {
    match glow_level {
        "none" => GlowProfile {
            trail_glow_strength: "0.0",
            cursor_glow_strength: "0.0",
            trail_edge_width_scale: "0.0",
            cursor_edge_width_scale: "0.0",
            trail_core_offset_scale: "0.0",
            trail_glow_width_scale: "1.0",
            cursor_glow_width_scale: "1.0",
            effect_blur_factor: 0.1,
            effect_spread_factor: 0.0,
            effect_ring_thickness_factor: 0.0,
        },
        "low" => GlowProfile {
            trail_glow_strength: "0.5",
            cursor_glow_strength: "0.5",
            trail_edge_width_scale: "1.0",
            cursor_edge_width_scale: "1.0",
            trail_core_offset_scale: "1.0",
            trail_glow_width_scale: "0.275",
            cursor_glow_width_scale: "0.3",
            effect_blur_factor: 0.35,
            effect_spread_factor: 0.5,
            effect_ring_thickness_factor: 0.5,
        },
        "high" => GlowProfile {
            trail_glow_strength: "0.5",
            cursor_glow_strength: "0.5",
            trail_edge_width_scale: "1.0",
            cursor_edge_width_scale: "1.0",
            trail_core_offset_scale: "1.0",
            trail_glow_width_scale: "0.85",
            cursor_glow_width_scale: "0.8",
            effect_blur_factor: 0.725,
            effect_spread_factor: 0.5,
            effect_ring_thickness_factor: 0.5,
        },
        _ => GlowProfile {
            trail_glow_strength: "0.5",
            cursor_glow_strength: "0.5",
            trail_edge_width_scale: "1.0",
            cursor_edge_width_scale: "1.0",
            trail_core_offset_scale: "1.0",
            trail_glow_width_scale: "0.5",
            cursor_glow_width_scale: "0.5",
            effect_blur_factor: 0.5,
            effect_spread_factor: 0.5,
            effect_ring_thickness_factor: 0.5,
        },
    }
}

fn invalid_cursor_config(path: &Path, field: &str, detail: String) -> CursorError {
    CursorError::classified(
        CursorErrorClass::Config,
        "invalid_cursor_config",
        format!("Invalid Yazelix cursor config at {field}."),
        "Update the cursor registry data, then retry.",
        json!({
            "path": path.display().to_string(),
            "field": field,
            "detail": detail,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use tempfile::{TempDir, tempdir};

    fn write_registry(raw: &str) -> (TempDir, PathBuf) {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cursors.toml");
        fs::write(&path, raw).unwrap();
        (temp, path)
    }

    // Defends: the TOML cutover backs up legacy state and preserves ordered settings plus custom definitions in the sole normal config.
    #[test]
    fn imports_legacy_jsonc_into_loadable_toml() {
        let temp = tempdir().unwrap();
        let legacy = temp.path().join("settings.jsonc");
        let config = temp.path().join("nested/cursors.toml");
        let raw = r##"{
  // user-owned cursor order and definition
  "schema_version": 1,
  "enabled_cursors": ["local_split", "blaze"],
  "settings": { "trail": "local_split", "trail_effect": "sweep", "mode_effect": "none", "glow": "high", "duration": 1.5 },
  "cursor": [
    { "name": "local_split", "family": "split", "colors": ["#112233", "#aabbcc"], "divider": "horizontal", "transition": "hard", "cursor_color": "#aabbcc" },
    { "name": "blaze", "family": "mono", "color": "#ffb929" }
  ]
}
"##;
        let expected = parse_cursor_settings_jsonc_text(&legacy, raw).unwrap().0;
        fs::write(&legacy, raw).unwrap();

        let backup = import_cursor_settings_jsonc(&legacy, &config).unwrap();
        let registry = load_cursor_config(&config).unwrap();

        assert_eq!(
            fs::read_to_string(backup).unwrap(),
            fs::read_to_string(&legacy).unwrap()
        );
        assert_eq!(registry, expected);
    }

    fn copy_packaged_shader_sources(destination: &Path) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ghostty/shaders");
        fs::copy(
            source.join("cursor_trail_common.glsl"),
            destination.join("cursor_trail_common.glsl"),
        )
        .unwrap();
    }

    fn generated_cursor_shader_name(definition: &CursorDefinition) -> String {
        format!("cursor_trail_{}.glsl", definition.name)
    }

    fn base_registry(extra: &str) -> String {
        format!(
            r##"
schema_version = 1
enabled_cursors = ["blaze"]

[settings]
trail = "random"
trail_effect = "random"
mode_effect = "random"
glow = "medium"
duration = 1.0

[[cursor]]
name = "blaze"
family = "mono"
color = "#ffb929"
{extra}
"##
        )
    }

    fn snow_random_registry(trail: &str) -> String {
        format!(
            r##"
schema_version = 1
enabled_cursors = ["snow", "blaze"]

[settings]
trail = "{trail}"
trail_effect = "tail"
mode_effect = "ripple"
glow = "medium"
duration = 1.0

[[cursor]]
name = "snow"
family = "mono"
color = "#ffffff"

[[cursor]]
name = "blaze"
family = "mono"
color = "#ffb929"
"##
        )
    }

    // Defends: terminal cursor targets are an explicit child-owned contract, not main-repo-only shader branches.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn cursor_target_contracts_cover_terminal_shader_and_protocol_boundaries() {
        let target_names: Vec<&str> = cursor_target_contracts()
            .iter()
            .map(|target| target.name)
            .collect();

        assert_eq!(
            target_names,
            vec![
                "ghostty",
                "rio-compatible-config",
                "mars",
                "rio",
                "ratty",
                "protocol_cursor_positions"
            ]
        );

        let rio_config = cursor_target_contract("rio-compatible-config").unwrap();
        assert_eq!(rio_config.status, "supported");
        assert!(rio_config.emits.contains(&"rio_compatible_config"));
        assert!(rio_config.requires.contains(&"colors.cursor"));

        let mars = cursor_target_contract("mars").unwrap();
        assert_eq!(mars.status, "supported");
        assert!(mars.emits.contains(&"ghostty_palette_shaders"));
        assert!(mars.requires.contains(&"MARS_RIO_TRAIL"));
        assert!(mars.requires.contains(&"iYazelixRioTrailAnimatedCursor"));

        let ratty = cursor_target_contract("ratty").unwrap();
        assert_eq!(ratty.status, "experimental_noop");
        assert!(ratty.emits.is_empty());

        let protocol = cursor_target_contract("protocol_cursor_positions").unwrap();
        assert!(
            protocol
                .requires
                .contains(&"terminal_multiple_cursor_protocol")
        );
    }

    // Defends: the shipped cursor registry can resolve a one-item enabled list and random only draws from that list.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_resolves_random_from_enabled_cursors() {
        let (_temp, path) = write_registry(&base_registry(""));
        let registry = load_cursor_config(&path).unwrap();

        let resolved = registry.resolve_with_entropy(51);

        assert_eq!(resolved.selected_cursor.unwrap().name, "blaze");
        assert_eq!(resolved.selected_trail_effect, Some("tail".to_string()));
        assert_eq!(
            resolved.selected_mode_effect,
            Some("ripple_rectangle".to_string())
        );
    }

    // Regression: light and auto appearance keep the configured random pool but skip snow when another cursor is available.
    // Strength: defect=3 behavior=3 resilience=2 cost=1 uniqueness=1 total=10/10
    #[test]
    fn random_cursor_resolution_skips_snow_for_light_safe_appearances() {
        let (_temp, path) = write_registry(&snow_random_registry("random"));
        let registry = load_cursor_config(&path).unwrap();

        assert_eq!(
            registry
                .resolve_with_entropy_for_appearance(0, "dark")
                .selected_cursor
                .unwrap()
                .name,
            "snow"
        );
        assert_eq!(
            registry
                .resolve_with_entropy_for_appearance(0, "light")
                .selected_cursor
                .unwrap()
                .name,
            "blaze"
        );
        assert_eq!(
            registry
                .resolve_with_entropy_for_appearance(0, "auto")
                .selected_cursor
                .unwrap()
                .name,
            "blaze"
        );
    }

    // Defends: explicitly selecting snow remains a user-owned choice even when light mode is active.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn explicit_snow_cursor_selection_is_not_filtered_by_light_mode() {
        let (_temp, path) = write_registry(&snow_random_registry("snow"));
        let registry = load_cursor_config(&path).unwrap();

        assert_eq!(
            registry
                .resolve_with_entropy_for_appearance(0, "light")
                .selected_cursor
                .unwrap()
                .name,
            "snow"
        );
    }

    // Defends: mono cursors accept one base color and derive the shader accent without requiring palette duplication.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_derives_mono_accent_and_cursor_color() {
        let (_temp, path) = write_registry(&base_registry(""));

        let registry = load_cursor_config(&path).unwrap();
        let blaze = registry.definitions.get("blaze").unwrap();

        assert_eq!(blaze.family, CursorFamily::Mono);
        assert_eq!(blaze.colors[0].hex, "#ffb929");
        assert_eq!(blaze.colors.len(), 2);
        assert_ne!(blaze.colors[1].hex, "#ffb929");
        assert_eq!(blaze.cursor_color.hex, "#ffb929");
    }

    // Defends: mono cursors still allow explicit accent and cursor overrides when the heuristic is not enough.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_accepts_mono_accent_and_cursor_overrides() {
        let (_temp, path) = write_registry(&base_registry(
            r##"
accent_color = "#ff0000"
cursor_color = "#00ff66"
"##,
        ));

        let registry = load_cursor_config(&path).unwrap();
        let blaze = registry.definitions.get("blaze").unwrap();

        assert_eq!(blaze.colors[1].hex, "#ff0000");
        assert_eq!(blaze.cursor_color.hex, "#00ff66");
    }

    // Defends: split cursors carry the explicit divider and transition contract used by generated shaders.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_parses_split_divider_and_transition() {
        let raw = base_registry("").replace(
            r##"name = "blaze"
family = "mono"
color = "#ffb929""##,
            r##"name = "blaze"
family = "split"
divider = "horizontal"
transition = "hard"
colors = ["#ff1600", "#2a3340"]"##,
        );
        let (_temp, path) = write_registry(&raw);

        let registry = load_cursor_config(&path).unwrap();
        let blaze = registry.definitions.get("blaze").unwrap();

        assert_eq!(blaze.family, CursorFamily::Split);
        assert_eq!(blaze.divider, Some(SplitDivider::Horizontal));
        assert_eq!(blaze.transition, Some(SplitTransition::Hard));
        assert_eq!(blaze.cursor_color.hex, "#ff1600");
    }

    // Defends: enabled_cursors must resolve exactly once to a cursor definition.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_rejects_missing_enabled_cursor_definition() {
        let raw = base_registry("").replace(
            "enabled_cursors = [\"blaze\"]",
            "enabled_cursors = [\"reef\"]",
        );
        let (_temp, path) = write_registry(&raw);

        let error = load_cursor_config(&path).unwrap_err();

        assert_eq!(error.code(), "invalid_cursor_config");
        assert!(format!("{error:?}").contains("enabled_cursors"));
    }

    // Defends: duplicate cursor definitions fail fast before shader paths can become ambiguous.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_rejects_duplicate_cursor_definitions() {
        let raw = base_registry(
            r##"
[[cursor]]
name = "blaze"
family = "mono"
color = "#ffffff"
"##,
        );
        let (_temp, path) = write_registry(&raw);

        let error = load_cursor_config(&path).unwrap_err();

        assert_eq!(error.code(), "invalid_cursor_config");
        assert!(format!("{error:?}").contains("defined more than once"));
    }

    // Defends: color and family validation rejects invalid user-authored shader inputs before runtime files are written.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_rejects_invalid_color_for_data_driven_cursor() {
        let raw = base_registry("").replace("#ffb929", "red");
        let (_temp, path) = write_registry(&raw);

        let error = load_cursor_config(&path).unwrap_err();

        assert_eq!(error.code(), "invalid_cursor_config");
        assert!(format!("{error:?}").contains("#rrggbb"));
    }

    // Regression: retired data-driven family names must fail fast instead of silently taking compatibility paths.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_rejects_retired_data_driven_family_names() {
        let raw = base_registry("").replace("family = \"mono\"", "family = \"simple_dual\"");
        let (_temp, path) = write_registry(&raw);

        let error = load_cursor_config(&path).unwrap_err();

        assert_eq!(error.code(), "invalid_cursor_config");
        assert!(format!("{error:?}").contains("Expected mono or split"));
    }

    // Regression: retired split field names must fail fast instead of silently taking compatibility paths.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn registry_rejects_retired_split_field_names() {
        let raw = base_registry("").replace(
            r##"name = "blaze"
family = "mono"
color = "#ffb929""##,
            r##"name = "blaze"
family = "split"
direction = "horizontal"
blend = false
colors = ["#ff1600", "#2a3340"]"##,
        );
        let (_temp, path) = write_registry(&raw);

        let error = load_cursor_config(&path).unwrap_err();

        assert_eq!(error.code(), "invalid_cursor_config_toml");
    }

    fn stale_neon_settings_jsonc(enabled: &str, trail: &str) -> String {
        format!(
            r##"{{
  // stale pre-removal cursor config
  "schema_version": 1,
  "enabled_cursors": {enabled},
  "settings": {{
    "trail": "{trail}",
    "trail_effect": "tail",
    "mode_effect": "ripple",
    "glow": "medium",
    "duration": 1.0
  }},
  "cursor": [
    {{
      "name": "blaze",
      "family": "mono",
      "color": "#ffb929"
    }},
    {{
      "name": "neon",
      "family": "curated_template",
      "template": "neon",
      "cursor_color": "#0090ff"
    }}
  ]
}}
"##
        )
    }

    // Regression: stale neon JSONC configs migrate before strict validation so runtime activation does not fail on removed defaults.
    // Strength: defect=3 behavior=3 resilience=2 cost=1 uniqueness=1 total=10/10
    #[test]
    fn cursor_settings_jsonc_migration_removes_retired_neon_entries() {
        let path = Path::new("settings.jsonc");
        let raw = stale_neon_settings_jsonc(r#"["blaze", "neon"]"#, "random");

        let (registry, migration) = parse_cursor_settings_jsonc_text(path, &raw).unwrap();

        assert!(migration.changed());
        assert!(
            migration
                .changed_paths
                .contains(&"enabled_cursors".to_string())
        );
        assert!(migration.changed_paths.contains(&"cursor".to_string()));
        assert!(
            migration
                .text
                .contains("// stale pre-removal cursor config")
        );
        assert!(!migration.text.contains("curated_template"));
        assert!(!migration.text.contains(r#""neon""#));
        assert_eq!(registry.enabled_cursors, vec!["blaze".to_string()]);
        assert!(!registry.definitions.contains_key("neon"));
        assert!(registry.definitions.contains_key("cosmic"));
    }

    // Regression: a config that selected only retired neon is remapped to the supported cosmic replacement.
    // Strength: defect=3 behavior=3 resilience=2 cost=1 uniqueness=1 total=10/10
    #[test]
    fn cursor_settings_jsonc_migration_replaces_neon_when_it_was_selected() {
        let path = Path::new("settings.jsonc");
        let raw = stale_neon_settings_jsonc(r#"["neon"]"#, "neon");

        let (registry, migration) = parse_cursor_settings_jsonc_text(path, &raw).unwrap();

        assert!(
            migration
                .changed_paths
                .contains(&"settings.trail".to_string())
        );
        assert_eq!(registry.enabled_cursors, vec!["cosmic".to_string()]);
        assert_eq!(registry.settings.trail, "cosmic");
        assert_eq!(
            registry.definitions.get("cosmic").unwrap().colors[0].hex,
            RETIRED_CURSOR_REPLACEMENT_COLOR
        );
    }

    // Defends: automatic cursor config migrations are durable and backup-first, not one-off local repair steps.
    // Strength: defect=3 behavior=3 resilience=2 cost=1 uniqueness=1 total=10/10
    #[test]
    fn persisted_cursor_settings_jsonc_migration_writes_backup_and_becomes_idempotent() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("settings.jsonc");
        fs::write(&path, stale_neon_settings_jsonc(r#"["neon"]"#, "neon")).unwrap();

        let (registry, migration) = load_cursor_settings_jsonc(&path).unwrap();
        let backup_path = persist_migrated_cursor_settings_jsonc(&path, &migration)
            .unwrap()
            .expect("migration backup path");

        assert_eq!(registry.settings.trail, "cosmic");
        assert!(backup_path.exists());
        let migrated = fs::read_to_string(&path).unwrap();
        assert!(!migrated.contains(r#""neon""#));
        assert!(
            fs::read_to_string(backup_path)
                .unwrap()
                .contains(r#""neon""#)
        );

        let (_registry, second_migration) = load_cursor_settings_jsonc(&path).unwrap();
        assert!(!second_migration.changed());
    }

    // Defends: the standalone cursor package can generate Ghostty palette shaders from the registry without settings.jsonc or runtime materialization.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn palette_shader_generation_uses_reusable_cursor_registry_boundary() {
        let (_registry_temp, path) = write_registry(&base_registry(""));
        let registry = load_cursor_config(&path).unwrap();
        let shader_dir = tempdir().unwrap();
        fs::write(
            shader_dir.path().join("cursor_trail_common.glsl"),
            "void renderMonoColorTrail(out vec4 fragColor, in vec2 fragCoord, vec4 color0, vec4 color1, float duration, float width, float scale) {}\n",
        )
        .unwrap();

        write_ghostty_cursor_palette_shaders(shader_dir.path(), &registry, "medium", 1.0).unwrap();

        let generated =
            fs::read_to_string(shader_dir.path().join("cursor_trail_blaze.glsl")).unwrap();
        assert!(generated.contains("Generated Yazelix mono cursor variant"));
        assert!(generated.contains("YAZELIX_TRAIL_GLOW_STRENGTH"));
        assert!(generated.contains("const float DURATION = 0.25;"));
        assert!(generated.contains("const vec4 YAZELIX_CURSOR_COLOR_0"));
    }

    // Regression: Rio-aware idle/motion tuning must be generated for every preset, not only the preset used during manual testing.
    // Strength: defect=3 behavior=3 resilience=2 cost=1 uniqueness=1 total=10/10
    #[test]
    fn palette_shader_generation_applies_rio_tuning_to_every_default_preset() {
        let (_temp, path) = write_registry(include_str!("../yazelix_cursors_default.toml"));
        let registry = load_cursor_config(&path).unwrap();
        let shader_dir = tempdir().unwrap();
        copy_packaged_shader_sources(shader_dir.path());

        write_ghostty_cursor_palette_shaders(shader_dir.path(), &registry, "medium", 1.0).unwrap();

        for cursor_name in &registry.enabled_cursors {
            let definition = registry.definitions.get(cursor_name).unwrap();
            let shader_path = shader_dir
                .path()
                .join(generated_cursor_shader_name(definition));
            let generated = fs::read_to_string(&shader_path).unwrap();
            assert!(
                generated.contains("return mix(0.035, mix(0.018, 0.300, motion), activity);"),
                "{} missing shared movement-spread policy",
                shader_path.display()
            );
            assert!(
                generated.contains("return mix(1.0, mix(0.65, 1.75, motion), activity);"),
                "{} missing visible low idle trail-glow policy",
                shader_path.display()
            );
            assert!(
                generated.contains(
                    "return max(max(yazelixRioTrailAnimatingFactor(), stretch), recentMove * 0.80);"
                ),
                "{} missing short movement glow boost",
                shader_path.display()
            );
            assert!(
                generated.contains("return mix(0.004, mix(0.003, 0.022, motion), activity);"),
                "{} missing visible idle cursor-glow width policy",
                shader_path.display()
            );
            assert!(
                generated.contains("return mix(1.0, mix(0.70, 1.55, motion), activity);"),
                "{} missing visible low idle cursor-glow policy",
                shader_path.display()
            );
            assert!(
                !generated.contains("float active"),
                "{} uses reserved GLSL identifier `active` in helper signatures",
                shader_path.display()
            );
            assert!(
                generated.contains("float rioTrailAnimating = yazelixRioTrailAnimatingFactor();"),
                "{} missing Rio animating gate",
                shader_path.display()
            );
            assert!(
                generated.contains(
                    "sdfTrail = mix(sdfTrail, yazelixRioTrailSdf(vu, offsetFactor), rioTrailAnimating);"
                ),
                "{} must only switch to Rio trail geometry while Rio is animating",
                shader_path.display()
            );
            assert!(
                generated.contains(
                    "fragColor = mix(trail, fragColor, mix(revealMix, 0.0, rioTrailAnimating));"
                ),
                "{} must reveal Rio trail geometry only while Rio is animating",
                shader_path.display()
            );
            assert!(
                generated.contains(
                    "trail = mix(trail, saturate(base, trailSaturation), trailCoreMask(sdfCurrentCursor, 0.0));"
                ),
                "{} missing explicit split cursor fill overlay",
                shader_path.display()
            );
            assert!(
                generated.contains(
                    "float cursorGlowWidth = yazelixRioCursorGlowWidth(rioTrailMotion, rioTrailActive);"
                ),
                "{} missing shared cursor glow width policy",
                shader_path.display()
            );
        }

        assert!(!shader_dir.path().join("cursor_trail_neon.glsl").exists());
    }

    // Defends: the shipped default registry parses as the active product cursor surface.
    // Strength: defect=2 behavior=2 resilience=2 cost=1 uniqueness=2 total=9/10
    #[test]
    fn shipped_default_registry_parses_active_cursor_surface() {
        let (_temp, path) = write_registry(include_str!("../yazelix_cursors_default.toml"));

        let registry = load_cursor_config(&path).unwrap();

        assert!(registry.enabled_cursors.contains(&"blaze".to_string()));
        assert!(registry.enabled_cursors.contains(&"snow".to_string()));
        assert!(registry.enabled_cursors.contains(&"ice".to_string()));
        assert!(registry.enabled_cursors.contains(&"midnight".to_string()));
        assert!(!registry.enabled_cursors.contains(&"neon".to_string()));
        assert!(registry.enabled_cursors.contains(&"magma".to_string()));
        assert!(!registry.enabled_cursors.contains(&"inferno".to_string()));
        assert!(
            registry
                .enabled_cursors
                .iter()
                .all(|name| registry.definitions.contains_key(name))
        );
        assert_eq!(
            registry.definitions.get("magma").unwrap().divider,
            Some(SplitDivider::Horizontal)
        );
        assert_eq!(
            registry.definitions.get("orchid").unwrap().transition,
            Some(SplitTransition::Hard)
        );
        assert_eq!(
            registry.definitions.get("reef").unwrap().colors[1].hex,
            "#00ff66"
        );
        assert_eq!(
            registry.definitions.get("ice").unwrap().colors[0].hex,
            "#38bdf8"
        );
        assert_eq!(
            registry.definitions.get("midnight").unwrap().colors[0].hex,
            "#0f172a"
        );
        assert_eq!(registry.settings.trail, "random");
    }
}
