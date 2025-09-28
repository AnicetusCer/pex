use super::data::Film;

pub fn normalize_title(s: &str) -> String { s.trim().to_lowercase() }

pub fn collect_genres(films: &[Film]) -> Vec<String> {
    use std::collections::BTreeSet; let mut set=BTreeSet::new();
    for f in films { if let Some(tags)=&f.tags_genre { for g in tags.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) { set.insert(g.to_string()); } } }
    set.into_iter().collect()
}

pub fn filtered_indices(films: &[Film], selected_genre: Option<&str>) -> Vec<usize> {
    let mut out=Vec::new();
    for (i,f) in films.iter().enumerate() { if let Some(g)=selected_genre { let ok=f.tags_genre.as_deref().map(|t| t.contains(g)).unwrap_or(false); if ok { out.push(i);} } else { out.push(i);} }
    out
}
