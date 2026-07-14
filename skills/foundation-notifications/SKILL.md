---
name: foundation-notifications
description: Queue, inspect, cancel, and report host notifications with channel policy, deduplication, quiet hours, and delivery state. Use when a completed or scheduled action should alert the user, for reminders, background results, delivery status, 通知、提醒、免打扰时间或消息送达状态.
---

# Foundation Notifications

Use the host notification outbox. Keep notification delivery separate from the work or trigger that produced it.

Read [references/contract.md](references/contract.md) before queueing, cancelling, retrying, or reconciling delivery.

## Follow the workflow

1. Confirm the target channel is available for the active user and host.
2. Use a stable deduplication key and the narrowest useful notification content.
3. Apply quiet hours and `not_before` before host delivery.
4. Distinguish pending, delivering, delivered, failed, cancelled, and uncertain.
5. Never retry an uncertain delivery blindly.

## Respect boundaries

- This skill does not own scheduler triggers, system permission prompts, credentials, authorization, or host delivery APIs.
- Do not put secrets or unnecessary private text in lock-screen-visible content.
- A queued notification does not prove that the underlying task succeeded.
- If notification tools are unavailable, report the result in the current conversation only.
