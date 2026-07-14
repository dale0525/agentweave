# Calendar Connector v1

The host owns scope, credentials, permissions, approval, idempotency, audit, and delivery. The skill owns workflow selection and clear user-facing state.

Required operations:

- list events within a bounded time range;
- read free-busy intervals;
- preview create, update, and cancel mutations;
- apply an approved immutable preview;
- inspect the final provider event.

Every event must preserve a stable ID, calendar ID, start and end instants, source timezone, attendees, recurrence, status, provider reference, and optimistic version.

Attendee-visible mutations require approval. A preview hash must bind the event version, time, timezone, attendees, location, recurrence, conflicts, account, and idempotency key.

Do not silently flatten unsupported recurrence rules, provider-specific calendars, or timezone transitions. Return a deterministic unsupported or conflict result.
