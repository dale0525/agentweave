---
name: foundation-contacts
description: Resolve contact identities, inspect provider-backed contact facts, and safely propose approved contact updates through a Contacts Connector. Use for recipient lookup, duplicate or ambiguous names, email or phone identity resolution, relationship context, 联系人、收件人、电话号码、邮箱匹配或通讯录更新.
---

# Foundation Contacts

Use the host Contacts Connector as the source of identity truth. Resolve ambiguity before using a contact in any external action.

Read [references/contract.md](references/contract.md) before updating a contact or passing an identity to Mail, Calendar, or Messaging.

## Resolve before acting

1. Search with the minimum useful name or identifier.
2. When multiple contacts match, show concise distinguishing facts and ask the user to choose.
3. Pass stable channel identities, not a display name, to downstream connectors.
4. Read the current contact and version before proposing an update.
5. Apply only the exact host-approved preview.

## Respect boundaries

- This skill does not own credentials, authorization, approval, audit, or provider synchronization.
- Relationship context is sensitive; retrieve only what the task requires.
- Never merge duplicate contacts or select a same-name person silently.
- If the connector is unavailable, ask for an explicit address and do not claim it was saved.
