use super::*;

pub(super) fn mail_address(address: &mail_parser::Addr<'_>) -> MailAddress {
    MailAddress {
        name: address.name.as_deref().map(str::to_owned),
        address: address
            .address
            .as_deref()
            .unwrap_or("unknown@invalid")
            .to_string(),
    }
}

pub(super) fn mail_addresses(addresses: &mail_parser::Address<'_>) -> Vec<MailAddress> {
    addresses.iter().map(mail_address).collect()
}

pub(super) fn to_lettre_mailbox(address: &MailAddress) -> MailResult<LettreMailbox> {
    let email = address
        .address
        .parse()
        .map_err(|_| MailError::InvalidRequest("mail address is invalid".into()))?;
    Ok(LettreMailbox::new(address.name.clone(), email))
}

pub(super) fn imap_search_query(search: &MailSearch) -> String {
    let mut terms = vec!["ALL".to_string()];
    if let Some(value) = &search.text {
        terms.push(format!("TEXT {}", quote_imap(value)));
    }
    if let Some(value) = &search.from {
        terms.push(format!("FROM {}", quote_imap(value)));
    }
    if let Some(value) = &search.to {
        terms.push(format!("TO {}", quote_imap(value)));
    }
    if let Some(value) = &search.subject {
        terms.push(format!("SUBJECT {}", quote_imap(value)));
    }
    if let Some(value) = search.is_read {
        terms.push(if value { "SEEN" } else { "UNSEEN" }.into());
    }
    if let Some(value) = search.has_attachment {
        terms.push(
            if value {
                "HEADER Content-Disposition attachment"
            } else {
                "NOT HEADER Content-Disposition attachment"
            }
            .into(),
        );
    }
    if let Some(value) = search.after {
        terms.push(format!("SINCE {}", value.format("%d-%b-%Y")));
    }
    if let Some(value) = search.before {
        terms.push(format!("BEFORE {}", value.format("%d-%b-%Y")));
    }
    terms.join(" ")
}

fn quote_imap(value: &str) -> String {
    format!("\"{}\"", value.replace(['\\', '\"'], " "))
}

pub(super) fn encode_cursor(offset: usize) -> String {
    offset.to_string()
}

pub(super) fn decode_cursor(cursor: Option<&str>) -> MailResult<usize> {
    cursor
        .unwrap_or("0")
        .parse()
        .map_err(|_| MailError::InvalidRequest("mail cursor is invalid".into()))
}

pub(super) fn encode_message_id(mailbox: &str, uid: u32) -> String {
    format!("imap:{}:{uid}", hex::encode(mailbox.as_bytes()))
}

pub(super) fn decode_message_id(id: &str) -> MailResult<(String, u32)> {
    let value = id
        .strip_prefix("imap:")
        .ok_or_else(|| MailError::InvalidRequest("message id is not an IMAP identifier".into()))?;
    let (mailbox, uid) = value
        .rsplit_once(':')
        .ok_or_else(|| MailError::InvalidRequest("message id is malformed".into()))?;
    let mailbox = String::from_utf8(
        hex::decode(mailbox)
            .map_err(|_| MailError::InvalidRequest("message mailbox is malformed".into()))?,
    )
    .map_err(|_| MailError::InvalidRequest("message mailbox is not UTF-8".into()))?;
    let uid = uid
        .parse()
        .map_err(|_| MailError::InvalidRequest("message UID is malformed".into()))?;
    Ok((mailbox, uid))
}

pub(super) fn ensure_account_id(expected: &str, actual: &str) -> MailResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(MailError::NotFound(actual.into()))
    }
}

pub(super) fn mailbox_role(name: &str, config: &ImapSmtpMailConfig) -> MailboxRole {
    if name.eq_ignore_ascii_case("INBOX") {
        MailboxRole::Inbox
    } else if config.sent_mailbox.as_deref() == Some(name) {
        MailboxRole::Sent
    } else if config.drafts_mailbox.as_deref() == Some(name) {
        MailboxRole::Drafts
    } else if config.archive_mailbox.as_deref() == Some(name) {
        MailboxRole::Archive
    } else if config.trash_mailbox.as_deref() == Some(name) {
        MailboxRole::Trash
    } else if name.to_ascii_lowercase().contains("junk")
        || name.to_ascii_lowercase().contains("spam")
    {
        MailboxRole::Junk
    } else {
        MailboxRole::Custom
    }
}

pub(super) fn text_chunk(
    content: &str,
    offset: u64,
    max_bytes: u32,
) -> MailResult<(String, Option<u64>, bool)> {
    let start = usize::try_from(offset)
        .map_err(|_| MailError::InvalidRequest("body offset is invalid".into()))?;
    if start > content.len() || !content.is_char_boundary(start) {
        return Err(MailError::InvalidRequest(
            "body offset is not a UTF-8 boundary".into(),
        ));
    }
    let mut end = (start + max_bytes as usize).min(content.len());
    while end > start && !content.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = end < content.len();
    Ok((
        content[start..end].to_string(),
        truncated.then_some(end as u64),
        truncated,
    ))
}

pub(super) fn is_localhost(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

pub(super) fn redacted_connector_error(_: anyhow::Error) -> MailError {
    MailError::Connector("credential authorization failed".into())
}

pub(super) fn imap_error(_: async_imap::error::Error) -> MailError {
    MailError::Connector("IMAP command failed".into())
}
