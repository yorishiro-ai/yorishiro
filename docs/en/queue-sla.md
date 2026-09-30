# Queue Start Objectives

Yorishiro records one lifecycle row for every job dispatched through the application worker adapters.

## Semantics

`enqueue_at` is the application timestamp written before the provider enqueue call.

`claim_at` and `start_at` are the first timestamp recorded when the worker handler receives the job.
Loco does not expose a provider claim hook, so these values are deliberately identical and represent the observable claim/start boundary.

Queue-start latency is `start_at - enqueue_at`.
The original enqueue timestamp is retained across provider retry and redelivery, so retries do not create false fast-start observations.

The lifecycle status is `queued`, `running`, `retrying`, `completed`, `failed`, `cancelled`, or `unavailable`.
Missing queue capacity and provider failures are not counted as successful starts.
Duplicate delivery is ignored after the lifecycle row is running or terminal.

## Capacity and diagnostics

Worker-class tags remain `worker-class:tenant-private`, `worker-class:official`, and `worker-class:shared`.
The lifecycle row stores the class, optional plan, concurrency key, and configured limit beside the timing data.
The `attempt` counter increments only when a delivery is admitted to running work.

PostgreSQL and SQLite use the application database for lifecycle durability.
Redis supplies provider job state, while lifecycle durability remains in the application database because Redis job inspection is provider-specific.
Provider-native queued, processing, completed, failed, and cancelled states should be combined with lifecycle rows for operations dashboards.

If the lifecycle table cannot be written, dispatch fails rather than reporting an unobserved SLA success.
If the queue provider is absent or unreachable, the row is marked `unavailable` by the caller and the request receives the provider error.

## Operations

Run workers with the existing class tags.
Do not start an untagged worker and expect it to consume class-routed jobs.
The admission path emits structured tracing fields `queue_start_seconds` and `lifecycle_id` when a job starts.
Export or filter those existing tracing events in the deployment's log/observability system; no separate metrics endpoint is added by this issue.
Capacity deferrals emit `worker capacity saturated`, retry transitions emit `retrying`, and provider or policy failures emit `unavailable` with a diagnostic field.
Inspect lifecycle rows grouped by `worker_class` and `status`, then compare recorded start times with the plan objective.
An increasing `queued`, `retrying`, or `unavailable` count means capacity or provider health must be restored before an SLA conclusion is made.

This issue does not add numeric priority, preemption, burst credits, RabbitMQ, SQS, contributed compute, or WASM execution.
