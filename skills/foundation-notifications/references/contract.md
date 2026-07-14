# Notification Host v1

Queue binds App, tenant, user, channel, title, body, deduplication key, earliest delivery time, optional quiet hours, and bounded structured data.

Scheduler execution may enqueue a notification, but its run state remains separate from notification delivery state.

The host claims due notifications with a lease and records delivered, retryable failure, permanent failure, or uncertain. An expired delivering lease becomes uncertain and requires reconciliation.

The host owns operating-system permission, presentation, privacy policy, channel registration, and platform delivery identifiers.
