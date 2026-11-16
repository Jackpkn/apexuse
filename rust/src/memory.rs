use crate::json::{parse_json, JsonValue};
use crate::policy::{contains_secure_marker, valid_bundle_id};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct WorkflowStep {
    pub action: String,
    pub target_role: String,
    pub target_label: String,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowRecipe {
    pub id: String,
    pub app: String,
    pub goal: String,
    pub steps: Vec<WorkflowStep>,
    pub updated_at: u64,
}

impl WorkflowRecipe {
    pub fn to_json(&self) -> JsonValue {
        let steps_json: Vec<JsonValue> = self
            .steps
            .iter()
            .map(|s| {
                JsonValue::object(vec![
                    ("action", JsonValue::String(s.action.clone())),
                    ("target_role", JsonValue::String(s.target_role.clone())),
                    ("target_label", JsonValue::String(s.target_label.clone())),
                    ("note", JsonValue::String(s.note.clone())),
                ])
            })
            .collect();
        JsonValue::object(vec![
            ("id", JsonValue::String(self.id.clone())),
            ("app", JsonValue::String(self.app.clone())),
            ("goal", JsonValue::String(self.goal.clone())),
            ("steps", JsonValue::Array(steps_json)),
            ("updated_at", JsonValue::Number(self.updated_at as f64)),
        ])
    }

    fn from_json(v: &JsonValue) -> Option<Self> {
        let id = v.get("id")?.as_str()?.to_string();
        let app = v.get("app")?.as_str()?.to_string();
        let goal = v.get("goal")?.as_str()?.to_string();
        let updated_at = v.get("updated_at").and_then(|x| x.as_i64()).unwrap_or(0) as u64;
        let arr = v.get("steps")?.as_array()?;
        let mut steps = Vec::new();
        for item in arr {
            steps.push(WorkflowStep {
                action: item.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                target_role: item.get("target_role").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                target_label: item.get("target_label").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                note: item.get("note").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            });
        }
        Some(Self {
            id,
            app,
            goal,
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
        if !valid_bundle_id(app) {
            return Err("invalid or protected app bundle id".into());
        }
        let clean_goal = sanitize_field(goal, 120)?;
        if clean_goal.is_empty() || steps.is_empty() || steps.len() > 20 {
            return Err("invalid recipe goal or step count".into());
        }
        let mut clean_steps = Vec::with_capacity(steps.len());
        for s in steps {
            clean_steps.push(WorkflowStep {
                action: sanitize_field(&s.action, 32)?,
                target_role: sanitize_field(&s.target_role, 32)?,
                target_label: sanitize_field(&s.target_label, 80)?,
                note: sanitize_field(&s.note, 120)?,
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
        let dir = std::env::temp_dir().join(format!("handsmcp_mem_{}", std::process::id()));
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
                }]
            )
            .is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
