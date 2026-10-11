//! What the tools hand back of what Strava answers.

use serde_json::{Map, Value, json};

/// Strava responses are shaped for apps: avatar URLs, encoded map polylines,
/// and client flags. Agents pay for every token, so drop what they cannot use.
const NOISE_KEYS: [&str; 19] = [
    "available_zones",
    "badge_type_id",
    "cover_photo",
    "cover_photo_small",
    "display_hide_heartrate_option",
    "embed_token",
    "external_id",
    "from_accepted_tag",
    "has_kudoed",
    "heartrate_opt_out",
    "map",
    "map_urls",
    "photos",
    "profile",
    "profile_medium",
    "resource_state",
    "stats_visibility",
    "upload_id",
    "upload_id_str",
];

/// Summary fields that matter when scanning a training log.
const ACTIVITY_SUMMARY_KEYS: [&str; 33] = [
    "id",
    "name",
    "sport_type",
    "start_date_local",
    "timezone",
    "distance",
    "moving_time",
    "elapsed_time",
    "total_elevation_gain",
    "elev_high",
    "elev_low",
    "average_speed",
    "max_speed",
    "average_heartrate",
    "max_heartrate",
    "average_watts",
    "weighted_average_watts",
    "device_watts",
    "kilojoules",
    "average_cadence",
    "average_temp",
    "suffer_score",
    "pr_count",
    "achievement_count",
    "kudos_count",
    "comment_count",
    "athlete_count",
    "workout_type",
    "gear_id",
    "trainer",
    "commute",
    "manual",
    "private",
];

/// Remove noise keys and nulls at every depth.
pub fn compact_strava_payload(value: Value) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().map(compact_strava_payload).collect())
        }
        Value::Object(object) => {
            let id_str = match object.get("id_str") {
                Some(Value::String(id)) => Some(id.clone()),
                _ => None,
            };
            let mut compacted = Map::new();
            for (key, entry) in object {
                if entry.is_null() || NOISE_KEYS.contains(&key.as_str()) || key == "id_str" {
                    continue;
                }
                compacted.insert(key, compact_strava_payload(entry));
            }
            // Route identifiers exceed 2^53, more than a JSON number holds for
            // most of what reads one: the identifier is the string Strava also sends.
            if let Some(id) = id_str {
                compacted.insert("id".to_owned(), Value::String(id));
            }
            Value::Object(compacted)
        }
        other => other,
    }
}

/// The activities of a list, each with its summary fields only.
pub fn activity_summaries(value: Value) -> Value {
    let Value::Array(activities) = value else {
        return Value::Array(Vec::new());
    };

    let summaries = activities.into_iter().filter_map(|activity| {
        let Value::Object(mut activity) = activity else {
            return None;
        };
        let mut summary = Map::new();
        for key in ACTIVITY_SUMMARY_KEYS {
            if let Some(entry) = activity.remove(key).filter(|entry| !entry.is_null()) {
                summary.insert(key.to_owned(), entry);
            }
        }
        Some(Value::Object(summary))
    });
    Value::Array(summaries.collect())
}

fn sample_indexes(length: usize, max_points: usize) -> Vec<usize> {
    if length <= max_points {
        return (0..length).collect();
    }
    // Computed as JavaScript does, in the same order and on the same numbers,
    // so that a position halfway between two samples goes to the same one.
    (0..max_points)
        .map(|index| ((index * (length - 1)) as f64 / (max_points - 1) as f64).round() as usize)
        .collect()
}

/// Reduce `key_by_type` streams to evenly spaced samples. A one-hour activity
/// recorded every second is 3,600 values per stream.
///
/// The result is `{ original_points, returned_points, streams }`. `max_points`
/// is at least 2, as the tool that takes it requires.
pub fn downsample_streams(value: &Value, max_points: usize) -> Value {
    let mut streams = Map::new();
    let mut original_points = 0;
    let mut returned_points = 0;

    if let Value::Object(by_type) = value {
        for (stream_type, stream) in by_type {
            let Some(Value::Array(data)) = stream.as_object().and_then(|stream| stream.get("data"))
            else {
                continue;
            };
            let sampled: Vec<Value> = sample_indexes(data.len(), max_points)
                .into_iter()
                .filter_map(|index| data.get(index).cloned())
                .collect();
            original_points = original_points.max(data.len());
            returned_points = returned_points.max(sampled.len());
            streams.insert(stream_type.clone(), Value::Array(sampled));
        }
    }

    json!({
        "original_points": original_points,
        "returned_points": returned_points,
        "streams": streams,
    })
}
