use std::fs;
use std::path::{Path, PathBuf};

use crate::json::{parse_json, JsonValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFormat {
    #[default]
    Jpeg,
    Png,
}

impl ImageFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Png => "png",
        }
    }

    pub fn mime_type(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageConfig {
    pub format: ImageFormat,
    pub quality: u8,
    pub max_width: usize,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            format: ImageFormat::Jpeg,
            quality: 80,
            max_width: 1280,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingConfig {
    pub settle_ms: u64,
    pub poll_interval_ms: u64,
    pub key_delay_us: u64,
}

impl Default for TimingConfig {
    fn default() -> Self {
        Self {
            settle_ms: 80,
            poll_interval_ms: 30,
            key_delay_us: 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotConfig {
    pub reuse_form_refs: bool,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            reuse_form_refs: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppConfig {
    pub image: ImageConfig,
    pub timing: TimingConfig,
    pub snapshot: SnapshotConfig,
}

impl AppConfig {
    pub fn load_or_default(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .map_err(|e| format!("failed to read config file {}: {e}", path.display()))?;
        let root = parse_json(&content)
            .map_err(|e| format!("failed to parse config JSON in {}: {e}", path.display()))?;
        let map = root
            .as_object()
            .ok_or_else(|| format!("config root in {} must be a JSON object", path.display()))?;

        let mut config = Self::default();

        if let Some(image_val) = map.get("image") {
            let im_obj = image_val
                .as_object()
                .ok_or_else(|| "field 'image' must be an object".to_string())?;
            if let Some(fmt_val) = im_obj.get("format") {
                let s = fmt_val
                    .as_str()
                    .ok_or_else(|| "image.format must be a string".to_string())?;
                config.set_key("image.format", s)?;
            }
            if let Some(q_val) = im_obj.get("quality") {
                let q = q_val
                    .as_i64()
                    .ok_or_else(|| "image.quality must be an integer".to_string())?;
                config.set_key("image.quality", &q.to_string())?;
            }
            if let Some(w_val) = im_obj.get("max_width") {
                let w = w_val
                    .as_i64()
                    .ok_or_else(|| "image.max_width must be an integer".to_string())?;
                config.set_key("image.max_width", &w.to_string())?;
            }
        }
        if let Some(fmt_val) = map.get("image.format") {
            let s = fmt_val
                .as_str()
                .ok_or_else(|| "image.format must be a string".to_string())?;
            config.set_key("image.format", s)?;
        }
        if let Some(q_val) = map.get("image.quality") {
            let q = q_val
                .as_i64()
                .ok_or_else(|| "image.quality must be an integer".to_string())?;
            config.set_key("image.quality", &q.to_string())?;
        }
        if let Some(w_val) = map.get("image.max_width") {
            let w = w_val
                .as_i64()
                .ok_or_else(|| "image.max_width must be an integer".to_string())?;
            config.set_key("image.max_width", &w.to_string())?;
        }

        if let Some(timing_val) = map.get("timing") {
            let t_obj = timing_val
                .as_object()
                .ok_or_else(|| "field 'timing' must be an object".to_string())?;
            if let Some(v) = t_obj.get("settle_ms") {
                let ms = v
                    .as_i64()
                    .ok_or_else(|| "timing.settle_ms must be an integer".to_string())?;
                config.set_key("timing.settle_ms", &ms.to_string())?;
            }
            if let Some(v) = t_obj.get("poll_interval_ms") {
                let ms = v
                    .as_i64()
                    .ok_or_else(|| "timing.poll_interval_ms must be an integer".to_string())?;
                config.set_key("timing.poll_interval_ms", &ms.to_string())?;
            }
            if let Some(v) = t_obj.get("key_delay_us") {
                let us = v
                    .as_i64()
                    .ok_or_else(|| "timing.key_delay_us must be an integer".to_string())?;
                config.set_key("timing.key_delay_us", &us.to_string())?;
            }
        }
        if let Some(v) = map.get("timing.settle_ms") {
            let ms = v
                .as_i64()
                .ok_or_else(|| "timing.settle_ms must be an integer".to_string())?;
            config.set_key("timing.settle_ms", &ms.to_string())?;
        }
        if let Some(v) = map.get("timing.poll_interval_ms") {
            let ms = v
                .as_i64()
                .ok_or_else(|| "timing.poll_interval_ms must be an integer".to_string())?;
            config.set_key("timing.poll_interval_ms", &ms.to_string())?;
        }
        if let Some(v) = map.get("timing.key_delay_us") {
            let us = v
                .as_i64()
                .ok_or_else(|| "timing.key_delay_us must be an integer".to_string())?;
            config.set_key("timing.key_delay_us", &us.to_string())?;
        }

        if let Some(snap_val) = map.get("snapshot") {
            let s_obj = snap_val
                .as_object()
                .ok_or_else(|| "field 'snapshot' must be an object".to_string())?;
            if let Some(v) = s_obj.get("reuse_form_refs") {
                let b = v
                    .as_bool()
                    .ok_or_else(|| "snapshot.reuse_form_refs must be a boolean".to_string())?;
                config.set_key("snapshot.reuse_form_refs", if b { "true" } else { "false" })?;
            }
        }
        if let Some(v) = map.get("snapshot.reuse_form_refs") {
            let b = v
                .as_bool()
                .ok_or_else(|| "snapshot.reuse_form_refs must be a boolean".to_string())?;
            config.set_key("snapshot.reuse_form_refs", if b { "true" } else { "false" })?;
        }

        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
            }
        }
        let file_name = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("config.json");
        let tmp_file_name = format!(".{}.tmp.{}", file_name, std::process::id());
        let tmp_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_file_name),
            _ => PathBuf::from(tmp_file_name),
        };

        let json_str = self.to_json().to_json_string();
        if let Err(e) = fs::write(&tmp_path, json_str.as_bytes()) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!(
                "failed to write temp config file {}: {e}",
                tmp_path.display()
            ));
        }

        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!(
                "failed to rename temp config {} to {}: {e}",
                tmp_path.display(),
                path.display()
            ));
        }

        Ok(())
    }

    pub fn to_json(&self) -> JsonValue {
        JsonValue::object(vec![
            (
                "image",
                JsonValue::object(vec![
                    ("format", JsonValue::String(self.image.format.as_str().to_string())),
                    ("quality", JsonValue::Number(self.image.quality as f64)),
                    ("max_width", JsonValue::Number(self.image.max_width as f64)),
                ]),
            ),
            (
                "timing",
                JsonValue::object(vec![
                    ("settle_ms", JsonValue::Number(self.timing.settle_ms as f64)),
                    ("poll_interval_ms", JsonValue::Number(self.timing.poll_interval_ms as f64)),
                    ("key_delay_us", JsonValue::Number(self.timing.key_delay_us as f64)),
                ]),
            ),
            (
                "snapshot",
                JsonValue::object(vec![
                    ("reuse_form_refs", JsonValue::Bool(self.snapshot.reuse_form_refs)),
                ]),
            ),
        ])
    }

    pub fn get_key(&self, key: &str) -> Result<JsonValue, String> {
        match key {
            "image.format" => Ok(JsonValue::String(self.image.format.as_str().to_string())),
            "image.quality" => Ok(JsonValue::Number(self.image.quality as f64)),
            "image.max_width" => Ok(JsonValue::Number(self.image.max_width as f64)),
            "timing.settle_ms" => Ok(JsonValue::Number(self.timing.settle_ms as f64)),
            "timing.poll_interval_ms" => Ok(JsonValue::Number(self.timing.poll_interval_ms as f64)),
            "timing.key_delay_us" => Ok(JsonValue::Number(self.timing.key_delay_us as f64)),
            "snapshot.reuse_form_refs" => Ok(JsonValue::Bool(self.snapshot.reuse_form_refs)),
            "image" => Ok(JsonValue::object(vec![
                ("format", JsonValue::String(self.image.format.as_str().to_string())),
                ("quality", JsonValue::Number(self.image.quality as f64)),
                ("max_width", JsonValue::Number(self.image.max_width as f64)),
            ])),
            "timing" => Ok(JsonValue::object(vec![
                ("settle_ms", JsonValue::Number(self.timing.settle_ms as f64)),
                ("poll_interval_ms", JsonValue::Number(self.timing.poll_interval_ms as f64)),
                ("key_delay_us", JsonValue::Number(self.timing.key_delay_us as f64)),
            ])),
            "snapshot" => Ok(JsonValue::object(vec![
                ("reuse_form_refs", JsonValue::Bool(self.snapshot.reuse_form_refs)),
            ])),
            _ => Err(format!("unknown config key: {key}")),
        }
    }

    pub fn set_key(&mut self, key: &str, raw_value: &str) -> Result<(), String> {
        let val = raw_value.trim().trim_matches('"');
        match key {
            "image.format" => match val.to_ascii_lowercase().as_str() {
                "jpeg" | "jpg" => self.image.format = ImageFormat::Jpeg,
                "png" => self.image.format = ImageFormat::Png,
                _ => {
                    return Err(format!(
                        "invalid image format '{val}', expected 'jpeg' or 'png'"
                    ))
                }
            },
            "image.quality" => {
                let q: u8 = val
                    .parse()
                    .map_err(|_| format!("invalid integer for image.quality: '{val}'"))?;
                if !(1..=100).contains(&q) {
                    return Err(format!("image.quality out of bounds (1..=100): {q}"));
                }
                self.image.quality = q;
            }
            "image.max_width" => {
                let w: usize = val
                    .parse()
                    .map_err(|_| format!("invalid integer for image.max_width: '{val}'"))?;
                if !(640..=2560).contains(&w) {
                    return Err(format!("image.max_width out of bounds (640..=2560): {w}"));
                }
                self.image.max_width = w;
            }
            "timing.settle_ms" => {
                let ms: u64 = val
                    .parse()
                    .map_err(|_| format!("invalid integer for timing.settle_ms: '{val}'"))?;
                if !(10..=3000).contains(&ms) {
                    return Err(format!("timing.settle_ms out of bounds (10..=3000): {ms}"));
                }
                self.timing.settle_ms = ms;
            }
            "timing.poll_interval_ms" => {
                let ms: u64 = val
                    .parse()
                    .map_err(|_| format!("invalid integer for timing.poll_interval_ms: '{val}'"))?;
                if !(10..=500).contains(&ms) {
                    return Err(format!("timing.poll_interval_ms out of bounds (10..=500): {ms}"));
                }
                self.timing.poll_interval_ms = ms;
            }
            "timing.key_delay_us" => {
                let us: u64 = val
                    .parse()
                    .map_err(|_| format!("invalid integer for timing.key_delay_us: '{val}'"))?;
                if !(0..=50000).contains(&us) {
                    return Err(format!("timing.key_delay_us out of bounds (0..=50000): {us}"));
                }
                self.timing.key_delay_us = us;
            }
            "snapshot.reuse_form_refs" => match val.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => self.snapshot.reuse_form_refs = true,
                "false" | "0" | "no" => self.snapshot.reuse_form_refs = false,
                _ => return Err(format!("invalid boolean for snapshot.reuse_form_refs: '{val}'")),
            },
            _ => return Err(format!("unknown config key: {key}")),
        }
        Ok(())
    }
}

pub fn resolve_config_path(state_dir: &Path) -> PathBuf {
    resolve_config_path_internal(state_dir, std::env::var("APEXUSE_CONFIG").ok().as_deref())
}

pub(crate) fn resolve_config_path_internal(state_dir: &Path, env_val: Option<&str>) -> PathBuf {
    if let Some(val) = env_val {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    state_dir.join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_image_format_helpers() {
        assert_eq!(ImageFormat::Jpeg.as_str(), "jpeg");
        assert_eq!(ImageFormat::Jpeg.mime_type(), "image/jpeg");
        assert_eq!(ImageFormat::Png.as_str(), "png");
        assert_eq!(ImageFormat::Png.mime_type(), "image/png");
    }

    #[test]
    fn test_app_config_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.image.format, ImageFormat::Jpeg);
        assert_eq!(cfg.image.quality, 80);
        assert_eq!(cfg.image.max_width, 1280);
        assert_eq!(cfg.timing.settle_ms, 80);
        assert_eq!(cfg.timing.poll_interval_ms, 30);
        assert_eq!(cfg.timing.key_delay_us, 500);
        assert_eq!(cfg.snapshot.reuse_form_refs, true);
    }

    #[test]
    fn test_load_or_default_missing_file_and_malformed() {
        let dir = std::env::temp_dir().join(format!("apexuse_config_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let missing = dir.join("nonexistent.json");
        let default_cfg =
            AppConfig::load_or_default(&missing).expect("should return default on missing");
        assert_eq!(default_cfg, AppConfig::default());

        let malformed = dir.join("malformed.json");
        fs::write(&malformed, "not valid json {[[}").unwrap();
        assert!(AppConfig::load_or_default(&malformed).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_or_default_partial_override() {
        let dir = std::env::temp_dir().join(format!("apexuse_config_partial_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("partial.json");
        fs::write(
            &path,
            r#"{
                "image": {
                    "quality": 92
                },
                "timing": {
                    "settle_ms": 110
                }
            }"#,
        )
        .unwrap();

        let loaded = AppConfig::load_or_default(&path).expect("load should succeed");
        assert_eq!(loaded.image.quality, 92);
        assert_eq!(loaded.image.format, ImageFormat::Jpeg);
        assert_eq!(loaded.image.max_width, 1280);
        assert_eq!(loaded.timing.settle_ms, 110);
        assert_eq!(loaded.timing.poll_interval_ms, 30);
        assert_eq!(loaded.timing.key_delay_us, 500);
        assert_eq!(loaded.snapshot.reuse_form_refs, true);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_set_key_and_get_key_and_validation() {
        let mut cfg = AppConfig::default();

        // Valid updates
        cfg.set_key("image.format", "png").unwrap();
        assert_eq!(cfg.image.format, ImageFormat::Png);
        assert_eq!(cfg.get_key("image.format").unwrap().as_str(), Some("png"));

        cfg.set_key("image.format", "\"jpeg\"").unwrap();
        assert_eq!(cfg.image.format, ImageFormat::Jpeg);

        cfg.set_key("image.quality", "90").unwrap();
        assert_eq!(cfg.image.quality, 90);
        assert_eq!(cfg.get_key("image.quality").unwrap().as_i64(), Some(90));

        cfg.set_key("image.max_width", "1600").unwrap();
        assert_eq!(cfg.image.max_width, 1600);
        assert_eq!(cfg.get_key("image.max_width").unwrap().as_i64(), Some(1600));

        cfg.set_key("timing.settle_ms", "120").unwrap();
        assert_eq!(cfg.timing.settle_ms, 120);
        assert_eq!(cfg.get_key("timing.settle_ms").unwrap().as_i64(), Some(120));

        cfg.set_key("timing.poll_interval_ms", "25").unwrap();
        assert_eq!(cfg.timing.poll_interval_ms, 25);
        assert_eq!(
            cfg.get_key("timing.poll_interval_ms").unwrap().as_i64(),
            Some(25)
        );

        cfg.set_key("timing.key_delay_us", "0").unwrap();
        assert_eq!(cfg.timing.key_delay_us, 0);
        assert_eq!(cfg.get_key("timing.key_delay_us").unwrap().as_i64(), Some(0));

        cfg.set_key("snapshot.reuse_form_refs", "false").unwrap();
        assert_eq!(cfg.snapshot.reuse_form_refs, false);
        assert_eq!(
            cfg.get_key("snapshot.reuse_form_refs").unwrap().as_bool(),
            Some(false)
        );

        // Subtree queries
        assert!(cfg.get_key("image").unwrap().as_object().is_some());
        assert!(cfg.get_key("timing").unwrap().as_object().is_some());
        assert!(cfg.get_key("snapshot").unwrap().as_object().is_some());

        // Validation bounds
        // quality: 1..=100
        assert!(cfg.set_key("image.quality", "0").is_err());
        assert!(cfg.set_key("image.quality", "101").is_err());
        assert!(cfg.set_key("image.quality", "abc").is_err());

        // max_width: 640..=2560
        assert!(cfg.set_key("image.max_width", "639").is_err());
        assert!(cfg.set_key("image.max_width", "2561").is_err());

        // settle_ms: 10..=3000
        assert!(cfg.set_key("timing.settle_ms", "9").is_err());
        assert!(cfg.set_key("timing.settle_ms", "3001").is_err());

        // poll_interval_ms: 10..=500
        assert!(cfg.set_key("timing.poll_interval_ms", "9").is_err());
        assert!(cfg.set_key("timing.poll_interval_ms", "501").is_err());

        // key_delay_us: 0..=50000
        assert!(cfg.set_key("timing.key_delay_us", "50001").is_err());

        // invalid booleans / formats
        assert!(cfg.set_key("image.format", "gif").is_err());
        assert!(cfg.set_key("snapshot.reuse_form_refs", "notabool").is_err());

        // unknown keys
        assert!(cfg.set_key("invalid.key", "val").is_err());
        assert!(cfg.get_key("invalid.key").is_err());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("apexuse_config_rt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        let mut original = AppConfig::default();
        original.set_key("image.format", "png").unwrap();
        original.set_key("image.quality", "95").unwrap();
        original.set_key("image.max_width", "1920").unwrap();
        original.set_key("timing.settle_ms", "150").unwrap();
        original.set_key("timing.poll_interval_ms", "45").unwrap();
        original.set_key("timing.key_delay_us", "1000").unwrap();
        original.set_key("snapshot.reuse_form_refs", "false").unwrap();

        original.save(&path).expect("save should succeed");
        let loaded = AppConfig::load_or_default(&path).expect("load should succeed");
        assert_eq!(original, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_config_path() {
        let state_dir = Path::new("/var/app/state");
        assert_eq!(
            resolve_config_path_internal(state_dir, None),
            PathBuf::from("/var/app/state/config.json")
        );
        assert_eq!(
            resolve_config_path_internal(state_dir, Some("")),
            PathBuf::from("/var/app/state/config.json")
        );
        assert_eq!(
            resolve_config_path_internal(state_dir, Some("   ")),
            PathBuf::from("/var/app/state/config.json")
        );
        assert_eq!(
            resolve_config_path_internal(state_dir, Some("/custom/path/cfg.json")),
            PathBuf::from("/custom/path/cfg.json")
        );
    }
}
