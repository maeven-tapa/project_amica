use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Days, Local, NaiveDate, Weekday};
use regex::Regex;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub key: String,
    pub category: &'static str,
    pub scope: String,
    pub claim: String,
    pub excerpt: String,
    pub state: &'static str,
    pub until: Option<String>,
    pub meaning: &'static str,
    pub correction: bool,
}

static UNSAFE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r#"(?i)(["“”\x60<>]|https?://|\\\\|(?:^|\s)[a-z]:\\|[\w.+-]+@[\w.-]+\.[a-z]{2,}|\d{5,}|\b(?:password|passphrase|token|secret|api[- ]?key|pin code|otp|credential|social security|address|phone number|credit card|bank account|diagnos\w*|medication|joke|kidding|hypothetical|imagine|pretend|roleplay|suppose|kunwari|biro|example|halimbawa|quoted?|transcript|clipboard|screenshot|article says)\b)"#,
).unwrap()
});
static PREFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)^(?:(?:actually|update|correction|ngayon|pero)[, ]+)?(?:i (?:really )?(?:prefer|like|enjoy|learn best|study best|focus best|no longer like|don't like)\b|mas gusto ko(?:ng)?\b|gusto ko(?:ng)?\b|hindi ko gusto\b|okay (?:sa akin|ang)\b)",
).unwrap()
});
static GOAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)^(?:(?:actually|update)[, ]+)?(?:i (?:have|need to prepare for|am preparing for|passed|finished|completed|am done with) (?:an? |my |the )?(?P<en>[\p{L}0-9 +#-]{0,50}?(?:exam|quiz|test|assessment))\b|may (?P<fil>[\p{L}0-9 +#-]{0,50}?(?:exam|quiz|test|assessment)) ako\b|(?:tapos|natapos) (?:na )?(?:ang|ko ang|ko na ang) (?P<done>[\p{L}0-9 +#-]{0,50}?(?:exam|quiz|test|assessment))\b)",
).unwrap()
});
static AIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)^(?:my goal is to |i(?:'m| am) working toward |ang goal ko ay |layunin kong |goal ko (?:na |ay )?)(?P<name>[\p{L}][\p{L}0-9 ',+#-]{2,80})$",
).unwrap()
});
static PROJECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)^(?:my project is |i(?:'m| am) working on a project (?:called|named) |ang proyekto ko ay |yung project ko ay )(?P<name>[\p{L}][\p{L}0-9 +#-]{1,60})$",
).unwrap()
});
static ROUTINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)^(?:i usually |i always |usually i |kadalasan ay |madalas akong |palagi akong )(?P<name>[\p{L}][\p{L}0-9 ',+#-]{2,85})$",
).unwrap()
});
static WEEKDAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)\b(monday|tuesday|wednesday|thursday|friday|saturday|sunday|lunes|martes|miyerkules|huwebes|biyernes|sabado|linggo|tomorrow|bukas|today|ngayon)\b",
).unwrap()
});

pub fn tokens(text: &str) -> Vec<&str> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect()
}

fn contains_any(words: &[&str], options: &[&str]) -> bool {
    words.iter().any(|word| options.contains(word))
}

pub fn study_scope(text: &str) -> Option<&'static str> {
    let words = tokens(text);
    if contains_any(
        &words,
        &[
            "memorize",
            "memorise",
            "memorizing",
            "memorising",
            "kabisaduhin",
            "kumakabisado",
            "memorization",
        ],
    ) {
        Some("memorizing")
    } else if contains_any(
        &words,
        &["read", "reading", "nagbabasa", "magbasa", "pagbabasa"],
    ) {
        Some("reading")
    } else if contains_any(
        &words,
        &[
            "study",
            "studying",
            "review",
            "reviewing",
            "nagaaral",
            "aaral",
            "aral",
            "magreview",
            "quiz",
            "exam",
        ],
    ) {
        Some("studying")
    } else if contains_any(
        &words,
        &[
            "exercise",
            "workout",
            "running",
            "jogging",
            "tumatakbo",
            "ehersisyo",
        ],
    ) {
        Some("exercise")
    } else {
        None
    }
}

fn due(text: &str, now: i64) -> Option<String> {
    let today = DateTime::from_timestamp_millis(now)?
        .with_timezone(&Local)
        .date_naive();
    if let Some(date) = text.split_whitespace().find_map(|word| {
        NaiveDate::parse_from_str(
            word.trim_matches(|ch: char| !ch.is_ascii_digit()),
            "%Y-%m-%d",
        )
        .ok()
    }) {
        return Some(date.to_string());
    }
    let day = WEEKDAY.find(text)?.as_str();
    let weekday = match day {
        "today" | "ngayon" => return Some(today.to_string()),
        "tomorrow" | "bukas" => return Some(today.succ_opt()?.to_string()),
        "monday" | "lunes" => Weekday::Mon,
        "tuesday" | "martes" => Weekday::Tue,
        "wednesday" | "miyerkules" => Weekday::Wed,
        "thursday" | "huwebes" => Weekday::Thu,
        "friday" | "biyernes" => Weekday::Fri,
        "saturday" | "sabado" => Weekday::Sat,
        _ => Weekday::Sun,
    };
    let days = weekday.days_since(today.weekday()).into();
    Some(today.checked_add_days(Days::new(days))?.to_string())
}

pub fn extract(user: &str, now: i64) -> Vec<Candidate> {
    let normalized = user.to_lowercase().replace('’', "'");
    if normalized.len() > 2_000
        || UNSAFE.is_match(&normalized)
        || user.contains('?')
        || user.lines().count() > 6
        || normalized.contains("if i ")
        || normalized.contains("kung ")
        || normalized.contains("i said")
        || normalized.contains("said i")
        || normalized.contains("sabi ng")
        || normalized.contains("sabi ko")
        || normalized.contains("just for today")
        || normalized.contains("ngayon lang")
        || normalized.contains("don't remember")
        || normalized.contains("huwag tandaan")
    {
        return Vec::new();
    }
    user.split(['.', '!', '\n', ';'])
        .filter_map(|segment| {
            let excerpt = segment.trim();
            if excerpt.len() > 280 || excerpt.contains("''") {
                return None;
            }
            let lower = excerpt.to_lowercase().replace('’', "'");
            let correction = [
                "actually",
                "correction",
                "update",
                "from now on",
                "mas gusto ko na",
            ]
            .iter()
            .any(|prefix| lower.starts_with(prefix));
            let weak = ["i think ", "probably ", "siguro ", "baka "]
                .iter()
                .find_map(|prefix| lower.strip_prefix(prefix));
            let candidate = |key: String, category, scope: String, claim, state, until| Candidate {
                key,
                category,
                scope,
                claim,
                excerpt: excerpt.to_owned(),
                state,
                until,
                meaning: "",
                correction,
            };
            if let Some(found) = GOAL.captures(&lower) {
                let topic = found
                    .name("en")
                    .or_else(|| found.name("fil"))
                    .or_else(|| found.name("done"))?
                    .as_str()
                    .trim();
                let completed = found.name("done").is_some()
                    || ["passed", "finished", "completed", "done with"]
                        .iter()
                        .any(|term| lower.contains(term));
                let display = excerpt
                    .get(lower.find(topic)?..lower.find(topic)? + topic.len())
                    .unwrap_or(topic);
                return Some(candidate(
                    format!("goal:{topic}"),
                    "Goal",
                    "studying".into(),
                    if completed {
                        format!("Completed {display}.")
                    } else {
                        format!("Prepare for {display}.")
                    },
                    if completed { "completed" } else { "active" },
                    due(&lower, now),
                ));
            }
            if PREFERENCE.is_match(weak.unwrap_or(&lower))
                && let Some(scope) = study_scope(&lower)
            {
                let words = tokens(&lower);
                let quiet = contains_any(&words, &["quiet", "silence", "tahimik", "katahimikan"]);
                let music =
                    contains_any(&words, &["music", "musika", "lofi", "lo", "instrumental"]);
                if quiet || music {
                    let mut found = candidate(
                        format!("preference:environment:{scope}"),
                        "Preference",
                        scope.into(),
                        excerpt.into(),
                        if weak.is_some() {
                            "tentative"
                        } else {
                            "active"
                        },
                        None,
                    );
                    let negative = lower.contains("don't like")
                        || lower.contains("no longer")
                        || lower.contains("hindi ko gusto");
                    found.meaning = match (quiet, music, negative) {
                        (true, false, false) | (false, true, true) => "quiet",
                        (false, true, false) | (true, false, true) => "music",
                        _ => "both",
                    };
                    return Some(found);
                }
            }
            for (rule, category) in [
                (&*AIM, "Goal"),
                (&*PROJECT, "Project"),
                (&*ROUTINE, "Routine"),
            ] {
                if let Some(found) = rule
                    .captures(&lower)
                    .and_then(|captures| captures.name("name"))
                {
                    let name = found.as_str().trim();
                    return Some(candidate(
                        format!("{}:{name}", category.to_lowercase()),
                        category,
                        study_scope(&lower).unwrap_or(category).to_lowercase(),
                        excerpt.into(),
                        "active",
                        None,
                    ));
                }
            }
            if PREFERENCE.is_match(&lower) {
                let marker = PREFERENCE.find(&lower)?;
                let subject = lower[marker.end()..].trim();
                const DURABLE: &[&str] = &[
                    "drawing",
                    "sketching",
                    "painting",
                    "writing",
                    "reading",
                    "cooking",
                    "gardening",
                    "chess",
                    "cycling",
                    "running",
                    "pagdodrowing",
                    "pagpipinta",
                    "pagsusulat",
                    "pagbabasa",
                    "pagluluto",
                    "paghahardin",
                    "pagbibisikleta",
                    "pagtakbo",
                ];
                let normalized = match subject {
                    "pagdodrowing" | "sketching" => "drawing",
                    "pagpipinta" => "painting",
                    "pagsusulat" => "writing",
                    "pagbabasa" => "reading",
                    "pagluluto" => "cooking",
                    "paghahardin" => "gardening",
                    "pagbibisikleta" => "cycling",
                    "pagtakbo" => "running",
                    _ => subject,
                };
                if DURABLE.contains(&subject) {
                    return Some(candidate(
                        format!("interest:{normalized}"),
                        "Interest",
                        normalized.into(),
                        excerpt.into(),
                        "active",
                        None,
                    ));
                }
            }
            None
        })
        .collect()
}
