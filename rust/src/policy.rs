use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DENIED_PATTERNS: &[&str] = &[
    "password",
    "1password",
    "onepassword",
    "bitwarden",
    "keychain",
    "dashlane",
    "lastpass",
    "nordpass",
    "proton.pass",
    "keepass",
    "bank",
    "finance",
    "wallet",
];

pub fn is_protected_app(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    DENIED_PATTERNS.iter().any(|pat| lower.contains(pat))
}

pub fn valid_bundle_id(bundle: &str) -> bool {
    if bundle.is_empty() || is_protected_app(bundle) {
        return false;
    }
    let parts: Vec<&str> = bundle.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    parts.iter().all(|seg| {
        let mut chars = seg.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphanumeric() => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

fn extract_outline_role(trimmed_line: &str) -> Option<&str> {
    let after_ref = if trimmed_line.starts_with("[e") {
        let idx = trimmed_line.find("] ")?;
        &trimmed_line[idx + 2..]
    } else {
        trimmed_line
    };
    let role = after_ref.split_whitespace().next()?;
    if role.chars().all(|c| c.is_ascii_alphabetic()) && !role.is_empty() {
        Some(role)
    } else {
        None
    }
}

fn has_repeated_bullet(s: &str) -> bool {
    let mut prev_bullet = false;
    for ch in s.chars() {
        let is_b = matches!(ch, '\u{2022}' | '\u{25cf}');
        if is_b && prev_bullet {
            return true;
        }
        prev_bullet = is_b;
    }
    false
}

pub fn contains_secure_marker(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim_start();
        let role = extract_outline_role(trimmed);
        let is_input_role = matches!(
            role,
            Some(
                "TextField"
                    | "SearchField"
                    | "ComboBox"
                    | "TextArea"
                    | "SecureTextField"
                    | "UnverifiedTextField"
            )
        );
        let is_display_role = role.is_some() && !is_input_role;
        let sanitized = if !is_input_role {
            line.replace(" • ", " ").replace(" ● ", " ")
        } else {
            line.to_string()
        };
        let lower = sanitized.to_ascii_lowercase();
        if lower.contains("securetextfield")
            || lower.contains("secure_text_field")
            || lower.contains("secure text field")
            || lower.contains("passwordfield")
            || lower.contains("password_field")
            || lower.contains("password field")
            || lower.contains("protectedcontent")
            || lower.contains("protected_content")
            || sanitized.contains('\u{f79a}')
        {
            return true;
        }
        if is_display_role {
            if has_repeated_bullet(&sanitized) {
                return true;
            }
        } else if sanitized.chars().any(|c| matches!(c, '\u{2022}' | '\u{25cf}')) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    ReadOnly,
    DryRun,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::ReadOnly => "read_only",
            Mode::DryRun => "dry_run",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicySettings {
    pub mode: Mode,
    pub require_confirmation: bool,
    pub rate_limit_per_minute: usize,
    pub confirmation_ttl_seconds: u64,
    pub allowed_apps: Vec<String>,
}

impl Default for PolicySettings {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            require_confirmation: false,
            rate_limit_per_minute: 1200,
            confirmation_ttl_seconds: 120,
            allowed_apps: vec!["*".into()],
        }
    }
}

impl PolicySettings {
    pub fn permits_app(&self, bundle: &str) -> bool {
        valid_bundle_id(bundle)
            && self
                .allowed_apps
                .iter()
                .any(|item| item == "*" || item.eq_ignore_ascii_case(bundle))
    }

    pub fn load_yaml(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("failed to read policy: {e}"))?;
        let mut settings = PolicySettings::default();
        for raw_line in text.lines() {
            // Strip comments outside quotes
            let line = strip_yaml_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            let (key, val) = line
                .split_once(':')
                .ok_or_else(|| format!("invalid policy line: {line}"))?;
            let key = key.trim();
            let val = val.trim();
            match key {
                "mode" => {
                    let clean = unquote(val);
                    settings.mode = match clean.as_str() {
                        "normal" => Mode::Normal,
                        "read_only" => Mode::ReadOnly,
                        "dry_run" => Mode::DryRun,
                        _ => return Err(format!("invalid mode: {clean}")),
                    };
                }
                "confirmation" => {
                    let clean = unquote(val).to_ascii_lowercase();
                    settings.require_confirmation = match clean.as_str() {
                        "required" | "always" => true,
                        "direct" | "none" => false,
                        _ => return Err(format!("invalid confirmation mode: {clean}")),
                    };
                }
                "require_confirmation" => {
                    let clean = unquote(val).to_ascii_lowercase();
                    settings.require_confirmation = match clean.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return Err(format!("invalid require_confirmation: {clean}")),
                    };
                }
                "rate_limit_per_minute" => {
                    let n: usize = val.parse().map_err(|_| "invalid rate_limit_per_minute")?;
                    if !(1..=3600).contains(&n) {
                        return Err("rate_limit_per_minute out of range".into());
                    }
                    settings.rate_limit_per_minute = n;
                }
                "confirmation_ttl_seconds" => {
                    let n: u64 = val.parse().map_err(|_| "invalid confirmation_ttl_seconds")?;
                    if !(1..=3600).contains(&n) {
                        return Err("confirmation_ttl_seconds out of range".into());
                    }
                    settings.confirmation_ttl_seconds = n;
                }
                "allowed_apps" => {
                    let list = parse_inline_yaml_list(val)?;
                    if list.len() > 100 {
                        return Err("too many allowed_apps".into());
                    }
                    for item in &list {
                        if item != "*" && !valid_bundle_id(item) {
                            return Err(format!("invalid or protected bundle id: {item}"));
                        }
                    }
                    settings.allowed_apps = list;
                }
                other => return Err(format!("unknown policy field: {other}")),
            }
        }
        Ok(settings)
    }
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn parse_inline_yaml_list(val: &str) -> Result<Vec<String>, String> {
    let t = val.trim();
    if !t.starts_with('[') || !t.ends_with(']') {
        return Err("allowed_apps must be an inline YAML list".into());
    }
    let inner = t[1..t.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner.split(',').map(|item| unquote(item)).collect())
}

pub struct RateLimiter {
    limit: usize,
    hits: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            hits: VecDeque::new(),
        }
    }

    pub fn allow(&mut self) -> bool {
        let now = Instant::now();
        let cutoff = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        while let Some(&front) = self.hits.front() {
            if front < cutoff {
                self.hits.pop_front();
            } else {
                break;
            }
        }
        if self.hits.len() >= self.limit {
            return false;
        }
        self.hits.push_back(now);
        true
    }
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    ClickRef {
        epoch: u64,
        ref_id: String,
        label: String,
        click_count: u8,
        button: String,
    },
    VisualClick {
        epoch: u64,
        mark_id: Option<String>,
        x: f64,
        y: f64,
        prefer_ax: bool,
        click_count: u8,
        button: String,
    },
    Hover {
        epoch: u64,
        x: f64,
        y: f64,
    },
    Drag {
        epoch: u64,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    TypeText {
        epoch: u64,
        ref_id: Option<String>,
        x: f64,
        y: f64,
        text: String,
        mode: String,
        click_first: bool,
    },
    PressKey {
        epoch: u64,
        key: String,
        modifiers: Vec<String>,
    },
    Scroll {
        epoch: u64,
        direction: String,
        amount: i32,
    },
}

pub struct ConfirmationStore {
    ttl: Duration,
    entries: HashMap<String, (Instant, PendingAction)>,
    counter: u64,
}

impl ConfirmationStore {
    pub fn new(ttl_seconds: u64) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_seconds),
            entries: HashMap::new(),
            counter: 0,
        }
    }

    pub fn issue(&mut self, action: PendingAction) -> String {
        self.counter += 1;
        let now = Instant::now();
        self.entries
            .retain(|_, (exp, _)| *exp > now);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let token = format!("tok_{}_{:08x}", self.counter, nanos ^ 0x5A5A_3C3C);
        self.entries.insert(token.clone(), (now + self.ttl, action));
        token
    }

    pub fn consume(&mut self, token: &str) -> Option<PendingAction> {
        let now = Instant::now();
        self.entries.retain(|_, (exp, _)| *exp > now);
        self.entries.remove(token).map(|(_, act)| act)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn record(&self, action: &str, decision: &str, code: &str) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("audit dir error: {e}"))?;
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!(
            "{{\"time\":{secs},\"action\":\"{}\",\"decision\":\"{}\",\"code\":\"{}\"}}\n",
            sanitize_token(action),
            sanitize_token(decision),
            sanitize_token(code),
        );
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("audit open error: {e}"))?;
        f.write_all(line.as_bytes())
            .map_err(|e| format!("audit write error: {e}"))
    }
}

fn sanitize_token(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(40)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_and_protected_checks() {
        assert!(valid_bundle_id("com.apple.TextEdit"));
        assert!(valid_bundle_id("com.google.Chrome"));
        assert!(!valid_bundle_id("Calculator"));
        assert!(!valid_bundle_id("com.agilebits.onepassword7"));
        assert!(!valid_bundle_id("com.bank.app"));
        assert!(!valid_bundle_id("com.example.keychain"));
    }

    #[test]
    fn secure_marker_detection_allows_spaced_static_bullet() {
        assert!(!contains_secure_marker("StaticText \"Today • 10:30\""));
        assert!(!contains_secure_marker("  [e25] Group \"~/handsmcp/docs • Untracked\""));
        assert!(!contains_secure_marker("  [e26] StaticText value=\"Workspace • Ready\""));
        assert!(!contains_secure_marker("  [e27] ListMarker value=\"• \""));
        assert!(!contains_secure_marker("  [e28] StaticText value=\"Uses copyright protected content\""));
        assert!(contains_secure_marker("AXProtectedContent"));
        assert!(contains_secure_marker("TextField \"protected_content\""));
        assert!(contains_secure_marker("TextField \"••••\""));
        assert!(contains_secure_marker("TextField \"•\""));
        assert!(contains_secure_marker("StaticText value=\"•••\""));
        assert!(contains_secure_marker("AXSecureTextField"));
    }

    #[test]
    fn policy_yaml_parses_repo_policy() {
        let dir = std::env::temp_dir().join(format!("handsmcp_pol_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.yaml");
        fs::write(
            &path,
            "mode: normal # comment\nrate_limit_per_minute: 90\nconfirmation_ttl_seconds: 60\nallowed_apps: [\"*\"]\n",
        )
        .unwrap();
        let s = PolicySettings::load_yaml(&path).unwrap();
        assert_eq!(s.mode, Mode::Normal);
        assert!(!s.require_confirmation);
        assert_eq!(s.rate_limit_per_minute, 90);
        assert!(s.permits_app("com.google.Chrome"));
        assert!(!s.permits_app("com.1password.app"));

        fs::write(
            &path,
            "mode: normal\nconfirmation: required\nallowed_apps: [\"*\"]\n",
        )
        .unwrap();
        let req = PolicySettings::load_yaml(&path).unwrap();
        assert!(req.require_confirmation);

        fs::write(
            &path,
            "mode: normal\nconfirmation: direct\nallowed_apps: [\"*\"]\n",
        )
        .unwrap();
        let d = PolicySettings::load_yaml(&path).unwrap();
        assert!(!d.require_confirmation);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirmation_token_is_single_use() {
        let mut store = ConfirmationStore::new(60);
        let tok = store.issue(PendingAction::Scroll {
            epoch: 1,
            direction: "down".into(),
            amount: 4,
        });
        assert!(store.consume(&tok).is_some());
        assert!(store.consume(&tok).is_none());
    }
}
