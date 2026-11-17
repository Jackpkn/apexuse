use crate::json::{parse_json, JsonValue};
use crate::policy::{contains_secure_marker, valid_bundle_id, valid_domain_pattern};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default)]
pub struct WorkflowStep {
    pub action: String,
    pub target_role: String,
    pub target_label: String,
    pub note: String,
    pub verify_change: bool,
}

#[derive(Debug, Clone)]
pub struct WorkflowRecipe {
    pub id: String,
    pub app: String,
    pub goal: String,
    pub expected_domain: Option<String>,
    pub expected_window_contains: Option<String>,
    pub steps: Vec<WorkflowStep>,
    pub updated_at: u64,
}

impl WorkflowRecipe {
    pub fn to_json(&self) -> JsonValue {
        let steps_json: Vec<JsonValue> = self
            .steps
            .iter()
            .map(|s| {
                let mut fields = vec![
                    ("action", JsonValue::String(s.action.clone())),
                    ("target_role", JsonValue::String(s.target_role.clone())),
                    ("target_label", JsonValue::String(s.target_label.clone())),
                    ("note", JsonValue::String(s.note.clone())),
                ];
                if s.verify_change {
                    fields.push(("verify_change", JsonValue::Bool(true)));
                }
                JsonValue::object(fields)
            })
            .collect();
        let mut fields = vec![
            ("id", JsonValue::String(self.id.clone())),
            ("app", JsonValue::String(self.app.clone())),
            ("goal", JsonValue::String(self.goal.clone())),
        ];
        if let Some(ref dom) = self.expected_domain {
            fields.push(("expected_domain", JsonValue::String(dom.clone())));
        }
        if let Some(ref win) = self.expected_window_contains {
            fields.push(("expected_window_contains", JsonValue::String(win.clone())));
        }
        fields.push(("steps", JsonValue::Array(steps_json)));
        fields.push(("updated_at", JsonValue::Number(self.updated_at as f64)));
        JsonValue::object(fields)
    }

    fn from_json(v: &JsonValue) -> Option<Self> {
        let id = v.get("id")?.as_str()?.to_string();
        let app = v.get("app")?.as_str()?.to_string();
        let goal = v.get("goal")?.as_str()?.to_string();
        let expected_domain = v
            .get("expected_domain")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let expected_window_contains = v
            .get("expected_window_contains")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let updated_at = v.get("updated_at").and_then(|x| x.as_i64()).unwrap_or(0) as u64;
        let arr = v.get("steps")?.as_array()?;
        let mut steps = Vec::new();
        for item in arr {
            steps.push(WorkflowStep {
                action: item.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                target_role: item.get("target_role").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                target_label: item.get("target_label").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                note: item.get("note").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                verify_change: item
                    .get("verify_change")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
            });
        }
        Some(Self {
            id,
            app,
            goal,
            expected_domain,
            expected_window_contains,
            steps,
            updated_at,
        })
    }
}

/// Safe structural workflow memory store. Rejects secrets, passwords, and raw user text.
pub struct WorkflowMemory {
    path: PathBuf,
}

impl WorkflowMemory {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load_all(&self) -> Result<Vec<WorkflowRecipe>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.path).map_err(|e| format!("memory read error: {e}"))?;
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let val = parse_json(&text)?;
        let arr = val
            .get("recipes")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "invalid memory file".to_string())?;
        Ok(arr.iter().filter_map(WorkflowRecipe::from_json).collect())
    }

    fn save_all(&self, recipes: &[WorkflowRecipe]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("memory dir error: {e}"))?;
        }
        let doc = JsonValue::object(vec![(
            "recipes",
            JsonValue::Array(recipes.iter().map(WorkflowRecipe::to_json).collect()),
        )]);
        fs::write(&self.path, doc.to_json_string())
            .map_err(|e| format!("memory write error: {e}"))
    }

    pub fn record(&self, app: &str, goal: &str, steps: Vec<WorkflowStep>) -> Result<WorkflowRecipe, String> {
        self.record_with_preconditions(app, goal, None, None, steps)
    }

    pub fn record_with_preconditions(
        &self,
        app: &str,
        goal: &str,
        expected_domain: Option<&str>,
        expected_window_contains: Option<&str>,
        steps: Vec<WorkflowStep>,
    ) -> Result<WorkflowRecipe, String> {
        if !valid_bundle_id(app) {
            return Err("invalid or protected app bundle id".into());
        }
        let clean_goal = sanitize_field(goal, 120)?;
        if clean_goal.is_empty() || steps.is_empty() || steps.len() > 20 {
            return Err("invalid recipe goal or step count".into());
        }
        let clean_domain = match expected_domain.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => {
                let norm = raw.to_ascii_lowercase();
                if !valid_domain_pattern(&norm) || norm == "*" {
                    return Err(format!("invalid expected_domain pattern: {raw}"));
                }
                Some(norm)
            }
            None => None,
        };
        let clean_window = match expected_window_contains.map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => Some(sanitize_field(raw, 120)?),
            None => None,
        };
        let mut clean_steps = Vec::with_capacity(steps.len());
        for s in steps {
            clean_steps.push(WorkflowStep {
                action: sanitize_field(&s.action, 32)?,
                target_role: sanitize_field(&s.target_role, 32)?,
                target_label: sanitize_field(&s.target_label, 80)?,
                note: sanitize_field(&s.note, 120)?,
                verify_change: s.verify_change,
            });
        }

        let mut recipes = self.load_all().unwrap_or_default();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let id = format!(
            "{}:{}",
            app,
            clean_goal
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
        );

        let recipe = WorkflowRecipe {
            id: id.clone(),
            app: app.to_string(),
            goal: clean_goal,
            expected_domain: clean_domain,
            expected_window_contains: clean_window,
            steps: clean_steps,
            updated_at: now,
        };

        recipes.retain(|r| r.id != id);
        recipes.push(recipe.clone());
        if recipes.len() > 200 {
            recipes.sort_by_key(|r| r.updated_at);
            recipes.drain(0..recipes.len() - 200);
        }
        self.save_all(&recipes)?;
        Ok(recipe)
    }

    pub fn lookup(&self, app: &str, query: Option<&str>) -> Result<Vec<WorkflowRecipe>, String> {
        let recipes = self.load_all()?;
        let q = query.map(|s| s.to_ascii_lowercase());
        Ok(recipes
            .into_iter()
            .filter(|r| r.app.eq_ignore_ascii_case(app))
            .filter(|r| match &q {
                None => true,
                Some(term) => r.goal.to_ascii_lowercase().contains(term),
            })
            .collect())
    }

    pub fn clear_app(&self, app: &str) -> Result<usize, String> {
        let mut recipes = self.load_all()?;
        let before = recipes.len();
        recipes.retain(|r| !r.app.eq_ignore_ascii_case(app));
        let removed = before - recipes.len();
        self.save_all(&recipes)?;
        Ok(removed)
    }
}

fn sanitize_field(input: &str, max_len: usize) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.len() > max_len {
        return Err(format!("field exceeds {max_len} characters"));
    }
    if contains_secure_marker(trimmed) {
        return Err("sensitive marker rejected from workflow memory".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    if ["password", "token", "secret", "otp", "passcode", "cvv", "pin"]
        .iter()
        .any(|k| lower.contains(k))
    {
        return Err("potential secret rejected from workflow memory".into());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_records_queries_and_rejects_secrets() {
        let dir = std::env::temp_dir().join(format!("apexuse_mem_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mem = WorkflowMemory::new(dir.join("memory.json"));

        let rec = mem
            .record(
                "com.apple.calculator",
                "Clear display",
                vec![WorkflowStep {
                    action: "click".into(),
                    target_role: "Button".into(),
                    target_label: "All Clear".into(),
                    note: "Reset previous expression".into(),
                    ..Default::default()
                }],
            )
            .unwrap();
        assert_eq!(rec.app, "com.apple.calculator");

        let found = mem.lookup("com.apple.calculator", Some("clear")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].steps[0].target_label, "All Clear");

        assert!(mem
            .record(
                "com.apple.TextEdit",
                "Enter password",
                vec![WorkflowStep {
                    action: "type".into(),
                    target_role: "TextArea".into(),
                    target_label: "secret".into(),
                    note: "".into(),
                    ..Default::default()
                }]
            )
            .is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_preconditions_and_legacy_json_roundtrip() {
        let dir = std::env::temp_dir().join(format!("apexuse_mem_pre_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory.json");

        // Legacy recipe JSON without expected_domain, expected_window_contains, or verify_change must load cleanly
        fs::write(
            &path,
            r#"{"recipes":[{"id":"com.apple.TextEdit:old","app":"com.apple.TextEdit","goal":"Old","updated_at":1,"steps":[{"action":"click","target_role":"Button","target_label":"Save","note":""}]}]}"#,
        )
        .unwrap();

        let mem = WorkflowMemory::new(path);
        let legacy = mem.load_all().unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].expected_domain, None);
        assert_eq!(legacy[0].expected_window_contains, None);
        assert!(!legacy[0].steps[0].verify_change);

        let rec = mem
            .record_with_preconditions(
                "com.google.Chrome",
                "Submit form",
                Some("Example.COM"),
                Some("Billing Portal"),
                vec![WorkflowStep {
                    action: "click".into(),
                    target_role: "Button".into(),
                    target_label: "Submit".into(),
                    note: "Commit".into(),
                    verify_change: true,
                }],
            )
            .unwrap();
        assert_eq!(rec.expected_domain.as_deref(), Some("example.com"));
        assert_eq!(rec.expected_window_contains.as_deref(), Some("Billing Portal"));
        assert!(rec.steps[0].verify_change);

        let loaded = mem.lookup("com.google.Chrome", Some("Submit")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].expected_domain.as_deref(), Some("example.com"));
        assert_eq!(loaded[0].expected_window_contains.as_deref(), Some("Billing Portal"));
        assert!(loaded[0].steps[0].verify_change);

        assert!(mem
            .record_with_preconditions(
                "com.google.Chrome",
                "Bad dom",
                Some("https://example.com"),
                None,
                vec![WorkflowStep {
                    action: "click".into(),
                    target_role: "Button".into(),
                    target_label: "Go".into(),
                    note: "".into(),
                    verify_change: false,
                }],
            )
            .is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
