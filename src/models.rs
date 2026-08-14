use std::time::SystemTime;

#[derive(Clone, Debug, Default)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
    pub scoped_weekly: Vec<ModelUsageLimit>,
}

impl UsageData {
    pub fn scoped_weekly_for(&self, model_name: &str) -> Option<&UsageSection> {
        self.scoped_weekly
            .iter()
            .find(|limit| limit.model_name.eq_ignore_ascii_case(model_name))
            .map(|limit| &limit.usage)
    }
}

#[derive(Clone, Debug)]
pub struct ModelUsageLimit {
    pub model_name: String,
    pub usage: UsageSection,
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub claude_code: Option<UsageData>,
    pub codex: Option<UsageData>,
    pub antigravity: Option<UsageData>,
}
