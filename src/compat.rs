use semver::{Version, VersionReq};

use crate::error::ClientError;

pub const TESTED_CLI_VERSION_REQ: &str = ">=0.100.0-alpha.2, <0.101.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityPolicy {
    Off,
    Warn,
    Strict,
}

impl Default for CompatibilityPolicy {
    fn default() -> Self {
        Self::Warn
    }
}

pub fn parse_cli_version(version_output: &str) -> Option<Version> {
    let token = version_output
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    Version::parse(token).ok()
}

pub fn check_cli_version(
    detected: Option<Version>,
    policy: CompatibilityPolicy,
) -> Result<Option<String>, ClientError> {
    if policy == CompatibilityPolicy::Off {
        return Ok(None);
    }

    let Some(detected) = detected else {
        if policy == CompatibilityPolicy::Strict {
            return Err(ClientError::Compatibility(
                "could not detect codex-cli version".to_string(),
            ));
        }
        return Ok(Some("could not detect codex-cli version".to_string()));
    };

    let req = VersionReq::parse(TESTED_CLI_VERSION_REQ).map_err(|err| {
        ClientError::Compatibility(format!("invalid internal version range: {err}"))
    })?;

    if req.matches(&detected) {
        return Ok(None);
    }

    let warning = format!("codex-cli {detected} is outside tested range {TESTED_CLI_VERSION_REQ}");

    if policy == CompatibilityPolicy::Strict {
        Err(ClientError::Compatibility(warning))
    } else {
        Ok(Some(warning))
    }
}
