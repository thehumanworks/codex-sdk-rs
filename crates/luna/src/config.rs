use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use codex_app_server_sdk::{DynamicToolSpec, ModelVerbosity, WebSearchMode};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::LunaError;

#[derive(Debug)]
pub(crate) struct LoadedAgent {
    pub(crate) instructions: String,
}

#[derive(Debug, Deserialize)]
struct CodexConfigFile {
    #[serde(default)]
    agents: toml::Table,
}

#[derive(Debug, Deserialize)]
struct AgentRoleConfig {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    config_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentConfigLayer {
    #[serde(default)]
    developer_instructions: Option<String>,
    #[serde(default)]
    model_instructions_file: Option<String>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliDynamicToolSpec {
    name: String,
    description: String,
    input_schema: Value,
}

fn parse_json_value(raw: &str, flag: &str) -> Result<Value, LunaError> {
    serde_json::from_str(raw)
        .map_err(|error| LunaError::Usage(format!("failed to parse JSON for {flag}: {error}")))
}

pub(crate) fn parse_optional_json_value(
    raw: Option<String>,
    flag: &str,
) -> Result<Option<Value>, LunaError> {
    raw.map(|value| parse_json_value(&value, flag)).transpose()
}

pub(crate) fn parse_optional_json_object(
    raw: Option<String>,
    flag: &str,
) -> Result<Option<Map<String, Value>>, LunaError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = parse_json_value(&raw, flag)?;
    match value {
        Value::Object(object) => Ok(Some(object)),
        _ => Err(LunaError::Usage(format!("{flag} must be a JSON object"))),
    }
}

pub(crate) fn parse_optional_dynamic_tools(
    raw: Option<String>,
) -> Result<Option<Vec<DynamicToolSpec>>, LunaError> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let parsed = parse_json_value(&raw, "--dynamic-tools-json")?;
    let array = match parsed {
        Value::Array(array) => array,
        _ => {
            return Err(LunaError::Usage(
                "--dynamic-tools-json must be a JSON array".to_string(),
            ));
        }
    };

    let mut tools = Vec::with_capacity(array.len());
    for (index, item) in array.into_iter().enumerate() {
        let spec: CliDynamicToolSpec = serde_json::from_value(item).map_err(|error| {
            LunaError::Usage(format!(
                "invalid --dynamic-tools-json entry at index {index}: {error}"
            ))
        })?;
        tools.push(DynamicToolSpec::new(
            spec.name,
            spec.description,
            spec.input_schema,
        ));
    }

    Ok(Some(tools))
}

pub(crate) fn build_thread_config(
    config_json: Option<String>,
    config_entries: Vec<String>,
    web_search_mode: Option<WebSearchMode>,
    config_profile: Option<String>,
    model_verbosity: Option<ModelVerbosity>,
    sandbox_network_access_enabled: Option<bool>,
    sandbox_writable_roots: Vec<String>,
) -> Result<Option<Map<String, Value>>, LunaError> {
    let mut config = parse_optional_json_object(config_json, "--config-json")?.unwrap_or_default();

    for entry in config_entries {
        let Some((raw_key, raw_value)) = entry.split_once('=') else {
            return Err(LunaError::Usage(
                "invalid --config entry; expected KEY=VALUE".to_string(),
            ));
        };

        let key = raw_key.trim();
        if key.is_empty() {
            return Err(LunaError::Usage(
                "invalid --config entry; key cannot be empty".to_string(),
            ));
        }

        let value = raw_value.trim();
        if value.is_empty() {
            return Err(LunaError::Usage(format!(
                "invalid --config entry for key '{key}'; value cannot be empty"
            )));
        }

        let parsed = serde_json::from_str::<Value>(value)
            .unwrap_or_else(|_| Value::String(value.to_string()));
        insert_thread_config(&mut config, key, parsed, "--config")?;
    }

    if let Some(mode) = web_search_mode {
        insert_thread_config(
            &mut config,
            "web_search",
            Value::String(mode.as_str().to_string()),
            "--web-search-mode",
        )?;
    }
    if let Some(profile) = config_profile {
        insert_thread_config(
            &mut config,
            "profile",
            Value::String(profile),
            "--config-profile",
        )?;
    }
    if let Some(verbosity) = model_verbosity {
        insert_thread_config(
            &mut config,
            "model_verbosity",
            Value::String(verbosity.as_str().to_string()),
            "--model-verbosity",
        )?;
    }
    if let Some(enabled) = sandbox_network_access_enabled {
        insert_thread_config(
            &mut config,
            "sandbox_workspace_write.network_access",
            Value::Bool(enabled),
            "--sandbox-network-access-enabled/--sandbox-network-access-disabled",
        )?;
    }
    if !sandbox_writable_roots.is_empty() {
        insert_thread_config(
            &mut config,
            "sandbox_workspace_write.writable_roots",
            Value::Array(
                sandbox_writable_roots
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
            "--sandbox-writable-root",
        )?;
    }

    if config.is_empty() {
        Ok(None)
    } else {
        Ok(Some(config))
    }
}

fn insert_thread_config(
    config: &mut Map<String, Value>,
    key: &str,
    value: Value,
    source: &str,
) -> Result<(), LunaError> {
    if let Some(existing) = config.get(key) {
        if existing == &value {
            return Ok(());
        }
        return Err(LunaError::Usage(format!(
            "conflicting configuration sources for '{key}'; {source} would replace an existing value"
        )));
    }
    config.insert(key.to_string(), value);
    Ok(())
}

pub(crate) fn resolve_output_schema(
    output_schema_json: Option<String>,
    output_schema_file: Option<String>,
) -> Result<Option<Value>, LunaError> {
    if let Some(raw) = output_schema_json {
        return Ok(Some(parse_json_value(&raw, "--output-schema-json")?));
    }

    let Some(path) = output_schema_file else {
        return Ok(None);
    };
    let path = path.trim();
    if path.is_empty() {
        return Err(LunaError::Usage(
            "value for --output-schema-file cannot be empty".to_string(),
        ));
    }
    let schema_path = PathBuf::from(path);
    let raw = fs::read_to_string(&schema_path).map_err(|error| {
        LunaError::Config(format!(
            "failed to read output schema file {}: {error}",
            schema_path.display()
        ))
    })?;
    let schema = serde_json::from_str::<Value>(&raw).map_err(|error| {
        LunaError::Usage(format!(
            "failed to parse JSON in output schema file {}: {error}",
            schema_path.display()
        ))
    })?;
    Ok(Some(schema))
}

pub(crate) fn normalize_agent_name(raw: &str) -> Result<String, LunaError> {
    let candidate = raw.trim();
    let candidate = candidate.strip_suffix(".toml").unwrap_or(candidate);
    let candidate = candidate.strip_suffix(".md").unwrap_or(candidate);
    if candidate.is_empty() {
        return Err(LunaError::Usage("agent name cannot be empty".to_string()));
    }
    if candidate == "." || candidate == ".." {
        return Err(LunaError::Usage(
            "agent name cannot be '.' or '..'".to_string(),
        ));
    }
    if candidate.contains('/') || candidate.contains('\\') {
        return Err(LunaError::Usage(
            "agent name cannot include path separators".to_string(),
        ));
    }
    if !candidate
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(LunaError::Usage(
            "agent name may only contain [A-Za-z0-9._-]".to_string(),
        ));
    }
    Ok(candidate.to_string())
}

pub(crate) fn load_agent_profile(agent_name: &str) -> Result<LoadedAgent, LunaError> {
    let codex_home = resolve_codex_home_dir().ok_or_else(|| {
        LunaError::Config(
            "unable to resolve Codex home directory (expected CODEX_HOME, HOME, USERPROFILE, or HOMEDRIVE+HOMEPATH)"
                .to_string(),
        )
    })?;
    let config_path = codex_home.join("config.toml");
    load_agent_profile_from_config(agent_name, &config_path)
}

pub(crate) fn resolve_codex_home_dir() -> Option<PathBuf> {
    if let Some(codex_home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(codex_home));
    }
    resolve_home_dir().map(|home| home.join(".codex"))
}

fn resolve_home_dir() -> Option<PathBuf> {
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    if let Some(userprofile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(userprofile));
    }
    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) if !drive.is_empty() && !path.is_empty() => {
            let mut resolved = PathBuf::from(drive);
            resolved.push(path);
            Some(resolved)
        }
        _ => None,
    }
}

pub(crate) fn load_agent_profile_from_config(
    agent_name: &str,
    config_path: &Path,
) -> Result<LoadedAgent, LunaError> {
    let config_raw = fs::read_to_string(config_path).map_err(|error| {
        LunaError::Config(format!(
            "failed to read Codex config file {}: {error}",
            config_path.display()
        ))
    })?;
    let config: CodexConfigFile =
        toml::from_str(&config_raw).map_err(|source| LunaError::Toml {
            path: config_path.to_path_buf(),
            source,
        })?;
    let role_value = config.agents.get(agent_name).ok_or_else(|| {
        LunaError::Config(format!(
            "agent '{}' was not found in {} under [agents.{}]",
            agent_name,
            config_path.display(),
            agent_name
        ))
    })?;
    let role: AgentRoleConfig = role_value.clone().try_into().map_err(|source| {
        LunaError::Config(format!(
            "invalid [agents.{agent_name}] in {}: {source}",
            config_path.display()
        ))
    })?;
    let instructions = if let Some(config_file) = role
        .config_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let role_config_path = resolve_path_from_file(config_path, config_file);
        let role_config_raw = fs::read_to_string(&role_config_path).map_err(|error| {
            LunaError::Config(format!(
                "failed to read agent config file for '{}' at {}: {error}",
                agent_name,
                role_config_path.display()
            ))
        })?;
        let role_config: AgentConfigLayer =
            toml::from_str(&role_config_raw).map_err(|source| LunaError::Toml {
                path: role_config_path.clone(),
                source,
            })?;
        resolve_agent_instructions(agent_name, &role, &role_config, &role_config_path)?
    } else if let Some(description) = role
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        description.to_string()
    } else {
        return Err(LunaError::Config(format!(
            "[agents.{agent_name}] in {} must set config_file or description",
            config_path.display()
        )));
    };

    Ok(LoadedAgent { instructions })
}

fn resolve_agent_instructions(
    agent_name: &str,
    role: &AgentRoleConfig,
    role_config: &AgentConfigLayer,
    role_config_path: &Path,
) -> Result<String, LunaError> {
    if let Some(instructions) = role_config
        .developer_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(instructions.to_string());
    }

    if let Some(model_instructions_file) = role_config
        .model_instructions_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let instructions_path = resolve_path_from_file(role_config_path, model_instructions_file);
        let instructions = fs::read_to_string(&instructions_path).map_err(|error| {
            LunaError::Config(format!(
                "failed to read model_instructions_file for '{}' at {}: {error}",
                agent_name,
                instructions_path.display()
            ))
        })?;
        let trimmed = instructions.trim();
        if trimmed.is_empty() {
            return Err(LunaError::Config(format!(
                "model_instructions_file for '{}' at {} is empty",
                agent_name,
                instructions_path.display()
            )));
        }
        return Ok(trimmed.to_string());
    }

    if let Some(description) = role
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(description.to_string());
    }

    Err(LunaError::Config(format!(
        "agent '{}' config {} must set `developer_instructions` or `model_instructions_file`",
        agent_name,
        role_config_path.display()
    )))
}

fn resolve_path_from_file(file_path: &Path, raw_path: &str) -> PathBuf {
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        file_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(candidate)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn thread_config_rejects_conflicting_layers_and_accepts_identical_values() {
        let error = build_thread_config(
            Some(r#"{"web_search":"cached"}"#.to_string()),
            vec![],
            Some(WebSearchMode::Live),
            None,
            None,
            None,
            vec![],
        )
        .expect_err("conflicting values");
        assert!(matches!(error, LunaError::Usage(_)));
        assert!(error.to_string().contains("web_search"));

        let config = build_thread_config(
            Some(r#"{"web_search":"live"}"#.to_string()),
            vec![],
            Some(WebSearchMode::Live),
            None,
            None,
            None,
            vec![],
        )
        .expect("identical values")
        .expect("config");
        assert_eq!(config["web_search"], "live");
    }

    #[test]
    fn malformed_json_and_toml_are_reported_in_their_categories() {
        let json = parse_optional_json_object(Some("{".to_string()), "--config-json")
            .expect_err("malformed JSON");
        assert!(matches!(json, LunaError::Usage(_)));

        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        fs::write(&config_path, "[agents.reviewer\n").expect("write malformed config");
        let toml =
            load_agent_profile_from_config("reviewer", &config_path).expect_err("malformed TOML");
        assert!(matches!(toml, LunaError::Toml { .. }));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn agent_instruction_precedence_is_inline_then_file_then_description() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        let role_path = dir.join("role.toml");
        let instructions_path = dir.join("instructions.md");
        fs::write(
            &config_path,
            "[agents.reviewer]\ndescription = \"description\"\nconfig_file = \"role.toml\"\n",
        )
        .expect("config");
        fs::write(&instructions_path, "file instructions").expect("instructions");
        fs::write(
            &role_path,
            "developer_instructions = \"inline instructions\"\nmodel_instructions_file = \"instructions.md\"\n",
        )
        .expect("role");
        assert_eq!(
            load_agent_profile_from_config("reviewer", &config_path)
                .expect("inline")
                .instructions,
            "inline instructions"
        );

        fs::write(
            &role_path,
            "model_instructions_file = \"instructions.md\"\n",
        )
        .expect("role");
        assert_eq!(
            load_agent_profile_from_config("reviewer", &config_path)
                .expect("file")
                .instructions,
            "file instructions"
        );

        fs::write(&role_path, "model = \"ignored\"\n").expect("role");
        assert_eq!(
            load_agent_profile_from_config("reviewer", &config_path)
                .expect("description")
                .instructions,
            "description"
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }

    fn make_temp_dir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let seq = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "luna-config-tests-{}-{stamp}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }
}
