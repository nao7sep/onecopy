//! Runtime AI acceleration choices. The backend is the sole authority for
//! which modes this packaged binary offers on this platform; Settings renders
//! these capabilities instead of duplicating target rules in the webview.

use serde::{Deserialize, Serialize};

pub const TRANSCRIPTION: &str = "transcription";
pub const FACE_SCORING: &str = "face-scoring";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    None,
    Metal,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Metal => "metal",
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.id())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionDescriptor {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub feature: &'static str,
    pub label: &'static str,
    pub selected: &'static str,
    pub default: &'static str,
    pub options: Vec<OptionDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub transcription: Mode,
    pub face_scoring: Mode,
}

pub fn metal_available() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

pub fn default_for(feature: &str) -> Result<Mode, String> {
    match feature {
        TRANSCRIPTION if metal_available() => Ok(Mode::Metal),
        TRANSCRIPTION | FACE_SCORING => Ok(Mode::None),
        _ => Err(format!("unknown AI acceleration feature: {feature}")),
    }
}

pub fn available(feature: &str) -> Result<Vec<Mode>, String> {
    match feature {
        TRANSCRIPTION => Ok(if metal_available() {
            vec![Mode::None, Mode::Metal]
        } else {
            vec![Mode::None]
        }),
        FACE_SCORING => Ok(vec![Mode::None]),
        _ => Err(format!("unknown AI acceleration feature: {feature}")),
    }
}

pub fn require_supported(feature: &str, mode: Mode) -> Result<Mode, String> {
    if available(feature)?.contains(&mode) {
        Ok(mode)
    } else {
        Err(format!(
            "AI acceleration '{mode}' is not available for {feature} in this packaged application"
        ))
    }
}

fn parse_mode(feature: &str, value: &serde_json::Value) -> Result<Mode, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("AI acceleration for {feature} must be a string"))?;
    let mode = match text {
        "none" => Mode::None,
        "metal" => Mode::Metal,
        _ => return Err(format!("unknown AI acceleration '{text}' for {feature}")),
    };
    require_supported(feature, mode)
}

fn selected(config: Option<&serde_json::Value>, feature: &str) -> Result<Mode, String> {
    let Some(value) = config
        .and_then(|value| value.get("aiAcceleration"))
        .and_then(|value| value.get(feature))
    else {
        return default_for(feature);
    };
    parse_mode(feature, value)
}

pub fn selection_from_config(config: Option<&serde_json::Value>) -> Result<Selection, String> {
    Ok(Selection {
        transcription: selected(config, TRANSCRIPTION)?,
        face_scoring: selected(config, FACE_SCORING)?,
    })
}

pub fn default_config() -> serde_json::Value {
    let mut values = serde_json::Map::new();
    for feature in [TRANSCRIPTION, FACE_SCORING] {
        values.insert(
            feature.to_string(),
            serde_json::Value::String(
                default_for(feature)
                    .expect("known feature")
                    .id()
                    .to_string(),
            ),
        );
    }
    serde_json::Value::Object(values)
}

pub fn validate_patch(patch: &serde_json::Value) -> Result<(), String> {
    let Some(value) = patch.get("aiAcceleration") else {
        return Ok(());
    };
    let entries = value
        .as_object()
        .ok_or("aiAcceleration must be an object")?;
    for (feature, value) in entries {
        if feature != TRANSCRIPTION && feature != FACE_SCORING {
            return Err(format!("unknown AI acceleration feature: {feature}"));
        }
        parse_mode(feature, value)?;
    }
    Ok(())
}

pub fn capabilities(config: Option<&serde_json::Value>) -> Result<Vec<Capability>, String> {
    let selection = selection_from_config(config)?;
    Ok([
        (TRANSCRIPTION, "Transcription", selection.transcription),
        (FACE_SCORING, "Face scoring", selection.face_scoring),
    ]
    .into_iter()
    .map(|(feature, label, selected)| Capability {
        feature,
        label,
        selected: selected.id(),
        default: default_for(feature).expect("known feature").id(),
        options: available(feature)
            .expect("known feature")
            .into_iter()
            .map(|mode| OptionDescriptor {
                id: mode.id(),
                label: match mode {
                    Mode::None => "CPU only",
                    Mode::Metal => "Metal",
                },
            })
            .collect(),
    })
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_is_always_available_and_face_has_no_hidden_accelerator() {
        assert!(available(TRANSCRIPTION).unwrap().contains(&Mode::None));
        assert_eq!(available(FACE_SCORING).unwrap(), vec![Mode::None]);
    }

    #[test]
    fn missing_values_use_the_accepted_platform_default() {
        let selected = selection_from_config(None).unwrap();
        assert_eq!(selected.transcription, default_for(TRANSCRIPTION).unwrap());
        assert_eq!(selected.face_scoring, Mode::None);
    }

    #[test]
    fn unsupported_and_unknown_values_never_fall_back() {
        let unknown = serde_json::json!({ "aiAcceleration": { "transcription": "cuda" } });
        assert!(selection_from_config(Some(&unknown))
            .unwrap_err()
            .contains("unknown"));
        let metal = serde_json::json!({ "aiAcceleration": { "face-scoring": "metal" } });
        assert!(selection_from_config(Some(&metal))
            .unwrap_err()
            .contains("not available"));
        let unknown_feature = serde_json::json!({ "aiAcceleration": { "future": "none" } });
        assert!(validate_patch(&unknown_feature)
            .unwrap_err()
            .contains("unknown"));
    }
}
