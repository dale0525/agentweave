# Messaging Connector v1

Read operations expose accounts, channels, threads, messages, stable sender and recipient identities, timestamps, provider references, and bounded attachment metadata.

Draft operations preserve a stable draft ID and optimistic version. Send preview binds the account, channel, thread, resolved recipients, draft version, body hash, attachments, idempotency key, and preview hash.

Approved send returns delivered, rejected, deferred, or uncertain. An uncertain result requires reconciliation and forbids blind retry.

The host owns credentials, contact resolution, authorization, approval, durable execution, idempotency, audit, and connector delivery truth.
