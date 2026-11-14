use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderValue, ACCEPT};
use reqwest::{StatusCode, Url};
use serde_json::Value;

use super::scheduled::ScheduledIndex;
use crate::config::{base_dir, LiveDvrConnection};

const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
const INVALID_SCHEDULE: &str = "Plex returned an incomplete or unrecognised DVR schedule.";

pub(crate) fn validate_server_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim())
        .map_err(|_| "Enter the Plex server URL, including http:// or https://.")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Enter the server address without credentials, query parameters or a fragment.".into(),
        );
    }
    let host = url.host_str().ok_or("Enter a Plex server address.")?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if local_http_host(host) => Ok(url),
        "http" => Err("Use HTTPS for a remote server, or enter its local network address.".into()),
        _ => Err("Use an http:// or https:// Plex server address.".into()),
    }
}

fn local_http_host(host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.');
    match host.parse::<IpAddr>() {
        Ok(ip) => local_ip(ip),
        Err(_) => !host.contains('.') || host.ends_with(".local") || host.ends_with(".home.arpa"),
    }
}

fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || matches!(ip.octets(), [100, 64..=127, _, _])
        }
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

pub(super) fn fetch_schedule(connection: &LiveDvrConnection) -> Result<ScheduledIndex, String> {
    let mut endpoint = validate_server_url(&connection.server_url)?;
    endpoint
        .path_segments_mut()
        .map_err(|_| "Enter a Plex server address.")?
        .pop_if_empty()
        .extend(["media", "subscriptions", "scheduled"]);
    if connection.token.trim().is_empty() {
        return Err("Enter your Plex token to read the DVR schedule.".into());
    }
    let mut token =
        HeaderValue::from_str(connection.token.trim()).map_err(|_| "Enter a valid Plex token.")?;
    token.set_sensitive(true);
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(12));
    if endpoint.scheme() == "http" {
        let host = endpoint.host_str().ok_or("Enter a Plex server address.")?;
        if host.trim_matches(['[', ']']).parse::<IpAddr>().is_err() {
            // Pin the vetted addresses for this request. A short hostname can
            // resolve to a public address through a configured DNS search suffix.
            let addrs: Vec<_> = (host, endpoint.port_or_known_default().unwrap_or(80))
                .to_socket_addrs()
                .map_err(|_| "Could not resolve the Plex server name.")?
                .filter(|addr| local_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err("Use HTTPS for a server outside the local network.".into());
            }
            builder = builder.resolve_to_addrs(host, &addrs);
        }
    }
    let client = builder
        .build()
        .map_err(|_| "Could not initialise the Plex connection.")?;
    let client_id = format!(
        "pex-{:x}",
        md5::compute(base_dir().as_os_str().as_encoded_bytes())
    );
    let response = client
        .get(endpoint)
        .header(ACCEPT, "application/json")
        .header("X-Plex-Token", token)
        .header("X-Plex-Client-Identifier", client_id)
        .header("X-Plex-Product", "PEX")
        .header("X-Plex-Version", env!("CARGO_PKG_VERSION"))
        .send()
        .map_err(|_| "Could not reach Plex. Check the server address and connection.")?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err("Plex denied DVR access. Check the token and its DVR permissions.".into())
        }
        status if status.is_redirection() => {
            return Err("Plex redirected this request. Enter the final server URL.".into())
        }
        status => {
            return Err(format!(
                "Plex returned HTTP {} when reading the DVR schedule.",
                status.as_u16()
            ))
        }
    }
    if response.content_length().is_some_and(|n| n > MAX_RESPONSE) {
        return Err("The Plex DVR schedule exceeded the download limit.".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not finish reading the Plex DVR schedule.")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("The Plex DVR schedule exceeded the download limit.".into());
    }
    parse_schedule(&bytes)
}

fn count(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn parse_schedule(bytes: &[u8]) -> Result<ScheduledIndex, String> {
    let response: Value = serde_json::from_slice(bytes).map_err(|_| INVALID_SCHEDULE)?;
    let container = response
        .get("MediaContainer")
        .and_then(Value::as_object)
        .ok_or(INVALID_SCHEDULE)?;
    let operations: &[Value] = match container.get("MediaGrabOperation") {
        Some(value) => value.as_array().ok_or(INVALID_SCHEDULE)?,
        None if container.get("size").and_then(count) == Some(0) => &[],
        None => return Err(INVALID_SCHEDULE.into()),
    };
    // A partial response cannot be used as the complete schedule to clear old badges.
    for (key, expected) in [
        ("size", operations.len()),
        ("totalSize", operations.len()),
        ("offset", 0),
    ] {
        if container
            .get(key)
            .is_some_and(|v| count(v) != Some(expected))
        {
            return Err(INVALID_SCHEDULE.into());
        }
    }
    let mut schedule = ScheduledIndex::default();
    for operation in operations {
        match operation
            .get("status")
            .and_then(Value::as_str)
            .ok_or(INVALID_SCHEDULE)?
        {
            "scheduled" | "inprogress" => {}
            "inactive" | "complete" | "cancelled" | "canceled" | "error" | "postprocessing"
            | "paused" | "conflict" | "conflicted" => continue,
            _ => return Err(INVALID_SCHEDULE.into()),
        }
        let metadata = operation
            .get("Metadata")
            .or_else(|| operation.get("Video"))
            .ok_or(INVALID_SCHEDULE)?;
        // PMS may wrap a single metadata object in an array like other containers.
        let metadata = match metadata.as_array() {
            Some(items) if items.len() == 1 => &items[0],
            Some(_) => return Err(INVALID_SCHEDULE.into()),
            None => metadata,
        };
        if metadata
            .get("type")
            .and_then(Value::as_str)
            .ok_or(INVALID_SCHEDULE)?
            == "movie"
        {
            let media = metadata
                .get("Media")
                .and_then(Value::as_array)
                .ok_or(INVALID_SCHEDULE)?;
            schedule.add_live_movie(metadata, selected_airing(operation, media));
        }
    }
    Ok(schedule)
}

fn selected_airing<'a>(operation: &Value, media: &'a [Value]) -> Option<&'a Value> {
    let mut selected = media.iter().filter(|m| {
        matches!(&m["selected"], Value::Bool(true))
            || m["selected"].as_i64() == Some(1)
            || m["selected"].as_str() == Some("1")
    });
    let marked = selected.next();
    if selected.next().is_some() {
        return None;
    }
    if let Some(index) = operation
        .get("mediaIndex")
        .filter(|v| v.as_i64() != Some(-1) && v.as_str() != Some("-1"))
    {
        let indexed = media.get(count(index)?)?;
        return marked
            .is_none_or(|m| std::ptr::eq(m, indexed))
            .then_some(indexed);
    }
    marked.or(if let [only] = media { Some(only) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    fn serve_once(response: String) -> (LiveDvrConnection, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/plex/", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        (
            LiveDvrConnection {
                server_url: base,
                token: "fixture-only-token".into(),
            },
            worker,
        )
    }

    #[test]
    fn reads_json_using_the_token_only_in_a_header() {
        let body = json!({"MediaContainer":{"size":1,"MediaGrabOperation":[{
            "status":"scheduled","Metadata":{"type":"movie","ratingKey":"plex%3A%2F%2Fmovie%2Fexample",
            "Media":[{"beginsAt":1794952800,"channelIdentifier":"rewind","id":12}]}
        }]}}).to_string();
        let (connection, worker) = serve_once(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        ));
        assert!(!fetch_schedule(&connection).unwrap().is_empty());
        let request = worker.join().unwrap().to_ascii_lowercase();
        assert_eq!(
            request.lines().next().unwrap(),
            "get /plex/media/subscriptions/scheduled http/1.1"
        );
        assert!(request.contains("x-plex-token: fixture-only-token"));
        assert!(request.contains("accept: application/json"));
    }

    #[test]
    fn refuses_redirects_and_never_displays_response_secrets() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        for status in ["302 Found", "401 Unauthorized", "403 Forbidden"] {
            let (connection, worker) = serve_once(format!(
                "HTTP/1.1 {status}\r\nLocation: http://{}/?private=fixture-only-token\r\nContent-Length: 18\r\n\r\nfixture-only-token",
                target.local_addr().unwrap()
            ));
            let error = fetch_schedule(&connection).err().unwrap();
            assert!(!error.contains("fixture-only-token"));
            assert!(worker.join().unwrap().contains("fixture-only-token"));
            assert_eq!(
                target.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn validates_server_addresses_and_empty_or_partial_schedules() {
        for address in [
            "http://fixture-nas:32400",
            "http://fixture-nas.local:32400",
            "http://192.168.1.10",
            "http://[::1]",
            "http://100.64.5.6",
            "https://my-server.example/plex",
        ] {
            assert!(validate_server_url(address).is_ok(), "{address}");
        }
        for address in [
            "http://www.example.org",
            "http://8.8.8.8",
            "file:///library.db",
            "https://user:pass@localhost",
            "https://localhost?token=secret",
            "https://localhost/#secret",
        ] {
            assert!(validate_server_url(address).is_err(), "{address}");
        }
        assert!(parse_schedule(br#"{"MediaContainer":{"size":0}}"#)
            .unwrap()
            .is_empty());
        for invalid in [
            json!({}),
            json!({"MediaContainer":{}}),
            json!({"MediaContainer":{"size":1}}),
            json!({"MediaContainer":{"size":0,"totalSize":2}}),
            json!({"MediaContainer":{"offset":1,"MediaGrabOperation":[]}}),
            json!({"MediaContainer":{"MediaGrabOperation":[{"status":"new-schema"}]}}),
        ] {
            assert!(parse_schedule(invalid.to_string().as_bytes()).is_err());
        }
        let ignored =
            ["cancelled", "conflict", "error", "complete", "paused"].map(|s| json!({"status":s}));
        assert!(parse_schedule(
            json!({"MediaContainer":{"MediaGrabOperation":ignored}})
                .to_string()
                .as_bytes()
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn uses_only_the_selected_media_from_each_grab() {
        let mut media = vec![json!({"id":1}), json!({"id":2})];
        assert!(selected_airing(&json!({}), &media).is_none());
        assert_eq!(
            selected_airing(&json!({"mediaIndex":1}), &media),
            Some(&media[1])
        );
        assert_eq!(
            selected_airing(&json!({"mediaIndex":-1}), &media[..1]),
            Some(&media[0])
        );
        media[0]["selected"] = json!(true);
        assert_eq!(selected_airing(&json!({}), &media), Some(&media[0]));
        assert!(selected_airing(&json!({"mediaIndex":1}), &media).is_none());
        media[1]["selected"] = json!(1);
        assert!(selected_airing(&json!({}), &media).is_none());
    }

    #[test]
    fn live_grabs_resolve_padded_start_using_film_station_and_media_id() {
        use crate::app::scheduled::{GuideChannel, RecordingStatus::*};
        use crate::app::types::{AiringQuality, PosterRow, PosterState};
        use std::time::{SystemTime, UNIX_EPOCH};

        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86_400;
        let recording = start - 600;
        let guid = "plex://movie/rescheduled";
        let listing = |id: i64, guid: &str, time: u64, station: &str| PosterRow {
            film_key: format!("guid:{guid}"),
            guid: Some(guid.to_owned()),
            guide_media_id: id,
            title: "Fixture".into(),
            year: Some(1984),
            guide_channel: GuideChannel::read(&json!({"at:channelIdentifier":station})),
            airing: Some(UNIX_EPOCH + Duration::from_secs(time)),
            movie_subscription_id: Option::None,
            url: String::new(),
            key: String::new(),
            channel: Option::None,
            channel_raw: Option::None,
            channel_title: Option::None,
            channel_thumb: Option::None,
            genres: vec![],
            summary: Option::None,
            audience_rating: Option::None,
            critic_rating: Option::None,
            path: Option::None,
            state: PosterState::Pending,
            owned: false,
            owned_modified: Option::None,
            owned_quality: Default::default(),
            airing_quality: AiringQuality::Unknown,
            scheduled: None,
        };
        let grab = |id: i64, time: u64, station: Option<&str>| {
            let json = json!({"MediaContainer":{"MediaGrabOperation":[{"status":"inprogress","Video":[{
                "type":"movie","guid":guid, "ratingKey":"123", "title":"Fixture","year":1984,
                "Media":[{"beginsAt":time.to_string(),"channelIdentifier":station,"id":id}]
            }]}]}});
            parse_schedule(&serde_json::to_vec(&json).unwrap()).unwrap()
        };
        let statuses = |schedule: &ScheduledIndex, rows: &mut Vec<PosterRow>| {
            let stats = schedule.apply(rows);
            assert_eq!(
                stats.misses + stats.by_guid + stats.by_title_slot,
                rows.len()
            );
            rows.iter().map(|r| r.scheduled).collect::<Vec<_>>()
        };
        let mut rows = vec![
            listing(12, guid, start, "rewind"),
            listing(13, guid, start + 1800, "rewind"),
            listing(14, "plex://movie/remake", start, "rewind"),
            listing(15, guid, start, "other"),
        ];
        assert_eq!(
            statuses(&grab(12, recording, Some("rewind")), &mut rows),
            [Exact, None, None, None]
        );
        // An ID reassigned to a different film/station cannot time-shift a booking onto it.
        for bad_id in [14, 15, 16] {
            assert_eq!(
                statuses(&grab(bad_id, recording, Some("rewind")), &mut rows),
                [Film, Film, None, Film]
            );
        }
        assert_eq!(
            statuses(&grab(12, start - 7200, Some("rewind")), &mut rows),
            [Film, Film, None, Film]
        );
        // A programme airing at the precise returned time takes precedence over any nearby reused ID.
        assert_eq!(
            statuses(&grab(13, start, Some("rewind")), &mut rows),
            [Exact, None, None, None]
        );
        assert_eq!(
            statuses(&grab(12, start, Option::None), &mut rows),
            [Slot, None, None, Slot]
        );
    }
}
