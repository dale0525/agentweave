---
name: foundation-scheduler
description: Create, inspect, pause, resume, cancel, and explain persistent one-shot, interval, cron, or RRULE scheduled jobs with timezone and misfire policy. Use for background triggers, recurring work, timed automation, 定时任务、周期执行、计划、cron、稍后执行或自动运行.
---

# Foundation Scheduler

Use the host Scheduler Store. Keep triggers separate from tasks, notification delivery, and external action approval.

Read [references/contract.md](references/contract.md) before creating or changing a schedule.

## Follow the workflow

1. Make the schedule kind, source timezone, first occurrence, and misfire behavior explicit.
2. Show the next occurrence before creation.
3. Keep the scheduled payload bounded and free of credentials.
4. Use stable job and run identities through restart and lease recovery.
5. Report paused, active, completed, cancelled, failed, and recovered claims accurately.

## Respect boundaries

- This skill does not own task state, notifications, credentials, approval, or the external actions a run may request.
- A trigger authorizes only starting the declared work, not bypassing approval at execution time.
- Do not guess a timezone or silently reinterpret daylight-saving transitions.
- If background execution is disabled by App policy, provide a proposed schedule only.
