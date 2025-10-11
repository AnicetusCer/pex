// src/app/types.rs
use eframe::egui::TextureHandle;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

// ---- cross-thread messages / data ----
pub enum OwnedMsg {
    Info(String),
    Done {
        keys: HashSet<String>,
        modified: HashMap<String, Option<u64>>,
    },
    Error(String),
}

#[derive(Clone, Debug)]
pub struct PrepItem {
    pub title: String,
    pub thumb_url: String,
    pub key: String,
    pub begins_at: Option<i64>,
    pub year: Option<i32>,
    pub tags_genre: Option<String>,
    pub channel_call_sign: Option<String>,
    pub channel_title: Option<String>,
    pub channel_thumb: Option<String>,
    pub guid: Option<String>,
    pub summary: Option<String>,
    pub audience_rating: Option<f32>,
    pub critic_rating: Option<f32>,
}

pub enum PrepMsg {
    Info(String),
    Done(Vec<PrepItem>),
    Error(String),
}

pub struct PrefetchDone {
    pub row_idx: usize,
    pub result: Result<PathBuf, String>,
}

// ---- app phases / states ----
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Prefetching,
    Ready,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BootPhase {
    Starting,
    CheckingNew, // phase 2
    Caching,     // phase 3
    Ready,       // phase 4
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PosterState {
    Pending, // queued or downloading
    Cached,  // file present on disk (ready to upload)
    Ready,   // texture uploaded
    Failed,  // permanent failure
}

// ---- UI controls ----
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayRange {
    Two,
    Four,
    Five,
    Seven,
    Fourteen,
}

impl DayRange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Two => "2",
            Self::Four => "4",
            Self::Five => "5",
            Self::Seven => "7",
            Self::Fourteen => "14",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "2" => Some(Self::Two),
            "4" => Some(Self::Four),
            "5" => Some(Self::Five),
            "7" => Some(Self::Seven),
            "14" => Some(Self::Fourteen),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Time,
    Title,
    Channel,
    Genre,
}

impl SortKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Title => "title",
            Self::Channel => "channel",
            Self::Genre => "genre",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "time" => Some(Self::Time),
            "title" => Some(Self::Title),
            "channel" => Some(Self::Channel),
            "genre" => Some(Self::Genre),
            _ => None,
        }
    }
}

// ---- core row backing each grid card ----
pub struct PosterRow {
    pub title: String,
    pub url: String,
    pub key: String,
    pub airing: Option<SystemTime>,
    pub year: Option<i32>,
    pub channel: Option<String>,
    pub channel_raw: Option<String>,
    pub channel_title: Option<String>,
    pub channel_thumb: Option<String>,
    pub genres: Vec<String>,
    pub guid: Option<String>,
    pub summary: Option<String>,
    pub audience_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub path: Option<PathBuf>,
    pub tex: Option<TextureHandle>, // UI thread only
    pub state: PosterState,
    pub owned: bool,
    pub owned_modified: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum RatingState {
    Idle,
    Pending,
    Success(String),
    NotFound,
    Error(String),
    MissingApiKey,
}

#[derive(Clone, Debug)]
pub struct RatingMsg {
    pub key: String,
    pub state: RatingState,
}
