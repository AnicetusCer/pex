use chrono::{DateTime, Utc};
#[derive(Clone, Debug, Default)]
pub struct Film {
    pub title: String,
    pub user_thumb_url: Option<String>,
    pub summary: Option<String>,
    pub tags_genre: Option<String>,
    pub extra_data: Option<String>,
    pub begins_at: Option<DateTime<Utc>>,
    pub year: Option<i32>,
    pub owned: bool,
}
