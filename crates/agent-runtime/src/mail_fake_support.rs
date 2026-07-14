use super::*;

pub(super) fn validate_nonempty(value: &str, label: &str) -> MailResult<()> {
    if value.trim().is_empty() {
        return Err(MailError::InvalidRequest(format!("{label} is empty")));
    }
    Ok(())
}

pub(super) fn validate_address(address: &MailAddress) -> MailResult<()> {
    let value = address.normalized();
    if !value.contains('@') || value.starts_with('@') || value.ends_with('@') {
        return Err(MailError::InvalidRequest(format!(
            "invalid mail address {}",
            address.address
        )));
    }
    Ok(())
}

pub(super) fn ensure_account<'a>(
    state: &'a FakeMailState,
    account_id: &str,
) -> MailResult<&'a MailAccount> {
    state
        .accounts
        .get(account_id)
        .ok_or_else(|| MailError::NotFound(account_id.into()))
}

pub(super) fn ensure_mailbox<'a>(
    state: &'a FakeMailState,
    account_id: &str,
    mailbox_id: &str,
) -> MailResult<&'a Mailbox> {
    state
        .mailboxes
        .get(mailbox_id)
        .filter(|mailbox| mailbox.account_id == account_id)
        .ok_or_else(|| MailError::NotFound(mailbox_id.into()))
}

pub(super) fn mailbox_for_role(
    state: &FakeMailState,
    account_id: &str,
    role: MailboxRole,
) -> MailResult<Mailbox> {
    state
        .mailboxes
        .values()
        .find(|mailbox| mailbox.account_id == account_id && mailbox.role == role)
        .cloned()
        .ok_or_else(|| MailError::NotFound(format!("{account_id}:{role:?}")))
}

pub(super) fn find_message<'a>(
    state: &'a FakeMailState,
    account_id: &str,
    message_id: &str,
) -> MailResult<&'a StoredMessage> {
    state
        .messages
        .get(message_id)
        .filter(|stored| stored.message.summary.account_id == account_id)
        .ok_or_else(|| MailError::NotFound(message_id.into()))
}

pub(super) fn find_message_mut<'a>(
    state: &'a mut FakeMailState,
    account_id: &str,
    message_id: &str,
) -> MailResult<&'a mut StoredMessage> {
    state
        .messages
        .get_mut(message_id)
        .filter(|stored| stored.message.summary.account_id == account_id)
        .ok_or_else(|| MailError::NotFound(message_id.into()))
}

pub(super) fn find_draft<'a>(
    state: &'a FakeMailState,
    account_id: &str,
    draft_id: &str,
) -> MailResult<&'a MailDraft> {
    state
        .drafts
        .get(draft_id)
        .filter(|draft| draft.account_id == account_id)
        .ok_or_else(|| MailError::NotFound(draft_id.into()))
}

pub(super) fn find_draft_mut<'a>(
    state: &'a mut FakeMailState,
    account_id: &str,
    draft_id: &str,
) -> MailResult<&'a mut MailDraft> {
    state
        .drafts
        .get_mut(draft_id)
        .filter(|draft| draft.account_id == account_id)
        .ok_or_else(|| MailError::NotFound(draft_id.into()))
}

pub(super) fn check_revision(actual: u64, expected: u64) -> MailResult<()> {
    if actual != expected {
        return Err(MailError::RevisionConflict { expected, actual });
    }
    Ok(())
}

pub(super) fn validate_approval(
    grant: &MailApprovalGrant,
    operation: MailApprovalOperation,
    resource_id: &str,
    revision: u64,
    preview_hash: Option<&str>,
) -> MailResult<()> {
    let hash_matches = match (preview_hash, grant.preview_hash.as_deref()) {
        (Some(expected), Some(actual)) => expected == actual,
        (None, None) => true,
        _ => false,
    };
    if grant.approval_id.trim().is_empty()
        || grant.operation != operation
        || grant.resource_id != resource_id
        || grant.revision != revision
        || !hash_matches
    {
        return Err(MailError::ApprovalMismatch);
    }
    Ok(())
}

pub(super) fn create_draft(
    state: &mut FakeMailState,
    account_id: String,
    content: DraftContent,
) -> MailResult<MailDraft> {
    ensure_account(state, &account_id)?;
    content.validate()?;
    state.next_draft += 1;
    let draft = MailDraft {
        id: format!("draft-{}", state.next_draft),
        account_id,
        revision: 1,
        content,
        provider_reference: None,
    };
    state.drafts.insert(draft.id.clone(), draft.clone());
    Ok(draft)
}

pub(super) fn prefixed_subject(prefix: &str, subject: &str) -> String {
    if subject
        .trim_start()
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        subject.into()
    } else {
        format!("{prefix} {subject}")
    }
}

pub(super) fn insert_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

pub(super) fn push_unique_address(values: &mut Vec<MailAddress>, value: MailAddress) {
    if !values
        .iter()
        .any(|existing| existing.normalized() == value.normalized())
    {
        values.push(value);
    }
}

pub(super) fn matches_search(message: &MailMessage, search: &MailSearch) -> bool {
    let contains = |candidate: &str, needle: &Option<String>| {
        needle.as_ref().is_none_or(|needle| {
            candidate
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
        })
    };
    let text = format!(
        "{} {} {}",
        message.summary.subject,
        message.summary.from.address,
        message
            .summary
            .to
            .iter()
            .map(|address| address.address.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    contains(&text, &search.text)
        && contains(&message.summary.from.address, &search.from)
        && search.to.as_ref().is_none_or(|needle| {
            message
                .summary
                .to
                .iter()
                .chain(&message.cc)
                .any(|address| contains(&address.address, &Some(needle.clone())))
        })
        && contains(&message.summary.subject, &search.subject)
        && search
            .after
            .is_none_or(|after| message.summary.sent_at >= after)
        && search
            .before
            .is_none_or(|before| message.summary.sent_at <= before)
        && search
            .has_attachment
            .is_none_or(|value| message.summary.has_attachments == value)
        && search
            .is_read
            .is_none_or(|value| message.summary.is_read == value)
}

pub(super) fn thread_from_messages(
    id: String,
    account_id: &str,
    messages: &[&MailMessage],
) -> MailThread {
    let last = messages.last().expect("thread must contain a message");
    let mut participants = Vec::new();
    for message in messages {
        push_unique_address(&mut participants, message.summary.from.clone());
        for address in message.summary.to.iter().chain(&message.cc) {
            push_unique_address(&mut participants, address.clone());
        }
    }
    MailThread {
        id,
        account_id: account_id.into(),
        subject: last.summary.subject.clone(),
        message_ids: messages
            .iter()
            .map(|message| message.summary.id.clone())
            .collect(),
        participants,
        last_message_at: last.summary.sent_at,
        unread_count: messages
            .iter()
            .filter(|message| !message.summary.is_read)
            .count() as u32,
        provider_reference: None,
    }
}

pub(super) fn paginate<T: Clone>(items: Vec<T>, page: &PageRequest) -> MailResult<Page<T>> {
    let offset = match &page.cursor {
        None => 0,
        Some(cursor) => cursor
            .strip_prefix("offset:")
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| MailError::InvalidRequest("invalid page cursor".into()))?,
    };
    if offset > items.len() {
        return Err(MailError::InvalidRequest(
            "page cursor exceeds result length".into(),
        ));
    }
    let end = offset.saturating_add(page.limit as usize).min(items.len());
    Ok(Page {
        items: items[offset..end].to_vec(),
        next_cursor: (end < items.len()).then(|| format!("offset:{end}")),
    })
}

pub(super) struct TextChunk {
    pub content: String,
    pub next_offset: Option<u64>,
    pub truncated: bool,
}

pub(super) fn text_chunk(source: &str, offset: u64, max_bytes: u32) -> MailResult<TextChunk> {
    let offset = usize::try_from(offset)
        .map_err(|_| MailError::InvalidRequest("body offset is too large".into()))?;
    if offset > source.len() || !source.is_char_boundary(offset) {
        return Err(MailError::InvalidRequest(
            "body offset is outside content or not a UTF-8 boundary".into(),
        ));
    }
    let mut end = offset.saturating_add(max_bytes as usize).min(source.len());
    while end > offset && !source.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = end < source.len();
    Ok(TextChunk {
        content: source[offset..end].into(),
        next_offset: truncated.then_some(end as u64),
        truncated,
    })
}

pub(super) fn materialize_sent_message(
    state: &mut FakeMailState,
    record: &PreviewRecord,
    receipt: &DeliveryReceipt,
) -> MailResult<()> {
    if state.messages.contains_key(&receipt.message_id) {
        return Ok(());
    }
    let sent_mailbox = mailbox_for_role(state, &record.preview.account_id, MailboxRole::Sent)?;
    let plain = SeedBodyPart::plain("plain", record.draft.content.body.plain_text.clone());
    let mut body_parts = vec![plain.metadata.clone()];
    let mut bodies = HashMap::from([(plain.metadata.id.clone(), plain)]);
    if let Some(html) = &record.draft.content.body.html {
        let html = SeedBodyPart::untrusted_html(
            "html",
            html.clone(),
            record.draft.content.body.plain_text.clone(),
        );
        body_parts.push(html.metadata.clone());
        bodies.insert(html.metadata.id.clone(), html);
    }
    let attachments = record
        .draft
        .content
        .attachments
        .iter()
        .enumerate()
        .map(|(index, attachment)| AttachmentMetadata {
            id: format!("sent-attachment-{}", index + 1),
            file_name: attachment.file_name.clone(),
            mime_type: attachment.mime_type.clone(),
            size_bytes: attachment.size_bytes,
            disposition: ContentDisposition::Attachment,
            content_id: None,
            provider_reference: None,
        })
        .collect::<Vec<_>>();
    let thread_id = record
        .draft
        .content
        .reply_context
        .as_ref()
        .and_then(|context| context.thread_id.clone());
    let in_reply_to = record
        .draft
        .content
        .reply_context
        .as_ref()
        .map(|context| context.internet_message_id.clone());
    let message = MailMessage {
        summary: MailMessageSummary {
            id: receipt.message_id.clone(),
            account_id: record.preview.account_id.clone(),
            thread_id,
            internet_message_id: receipt.internet_message_id.clone(),
            from: record.preview.from.clone(),
            to: record.preview.to.clone(),
            subject: record.preview.subject.clone(),
            sent_at: receipt.submitted_at,
            is_read: true,
            has_attachments: !attachments.is_empty(),
            mailbox_ids: vec![sent_mailbox.id],
            provider_reference: None,
        },
        reply_to: Vec::new(),
        cc: record.preview.cc.clone(),
        bcc: record.preview.bcc.clone(),
        in_reply_to,
        references: record
            .draft
            .content
            .reply_context
            .as_ref()
            .map(|context| vec![context.internet_message_id.clone()])
            .unwrap_or_default(),
        body_parts,
        attachments,
    };
    state.messages.insert(
        receipt.message_id.clone(),
        StoredMessage {
            message,
            bodies,
            attachments: HashMap::new(),
        },
    );
    Ok(())
}
