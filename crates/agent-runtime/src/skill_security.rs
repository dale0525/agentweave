use anyhow::bail;

pub const RESERVED_SKILL_URI_PREFIXES: [&str; 5] = [
    "app://builtin-skills",
    "app://managed-skills",
    "app://skill-staging",
    "app://skill-quarantine",
    "app://skill-state",
];

pub const RESERVED_SKILL_URI_ERROR: &str =
    "skill control-plane path is not available to generic tools";

pub fn reject_reserved_skill_uri(uri: &str) -> anyhow::Result<()> {
    if reserved_skill_uri(uri) {
        bail!(RESERVED_SKILL_URI_ERROR);
    }
    Ok(())
}

fn reserved_skill_uri(value: &str) -> bool {
    let decoded = repeatedly_percent_decode(value.as_bytes());
    let Ok(decoded) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let Some(rest) = decoded
        .get(..6)
        .filter(|scheme| scheme.eq_ignore_ascii_case("app://"))
    else {
        return false;
    };
    let authority = decoded[rest.len()..]
        .split(['/', '\\', '?', '#'])
        .next()
        .unwrap_or_default();
    RESERVED_SKILL_URI_PREFIXES.iter().any(|prefix| {
        prefix
            .strip_prefix("app://")
            .is_some_and(|reserved| authority.eq_ignore_ascii_case(reserved))
    })
}

fn repeatedly_percent_decode(input: &[u8]) -> Vec<u8> {
    let mut current = input.to_vec();
    for _ in 0..4 {
        let next = percent_decode_once(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn percent_decode_once(input: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%'
            && index + 2 < input.len()
            && let (Some(high), Some(low)) =
                (hex_value(input[index + 1]), hex_value(input[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
            continue;
        }
        decoded.push(input[index]);
        index += 1;
    }
    decoded
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
