# Queue Start Objectives

Yorishiro records a lifecycle row for each job it submits through its workers.

## Semantics

`enqueue_at` is the timestamp recorded before Yorishiro submits a job to the queue.

`claim_at` and `start_at` record when a worker first receives the job.
They have the same value because the queue does not report a separate claim time.

Queue-start latency is `start_at - enqueue_at`.
Retries and redeliveries keep the original `enqueue_at`, so start latency includes time spent waiting for another attempt.

The lifecycle status is `queued`, `running`, `retrying`, `completed`, `failed`, `cancelled`, or `unavailable`.
Missing queue capacity and provider failures are not counted as successful starts.
Duplicate delivery is ignored after the lifecycle row is running or terminal.
Running rows carry a five-minute lease.
Redelivery during the lease is ignored as a duplicate.
After the lease expires, the next delivery increments `attempt`, and the previous worker cannot complete or retry the job.

## Capacity and diagnostics

Each job has one worker target: `worker-target:shared`, `worker-target:official`, or `worker-target:tenant:<tenant-id>`.
The lifecycle row records the class, plan (if present), target key, and limit (if enabled) alongside the timing data.
An empty limit means no target admission cap applies.
The `attempt` counter increments only when a delivery is admitted to running work.

Lifecycle records remain in the application database, even when Redis stores the queue.
Check both the queue's job status and the lifecycle record when diagnosing a job.

## Job Execution And Backend Selection

Embedding sync, workspace reindex (including startup and scheduled reindex), query embedding, fill-defaults migration, and Enterprise infer-fill execute as registered Loco `BackgroundWorker` jobs.
The selected PostgreSQL, SQLite, or Valkey queue provider stores and delivers those jobs to Loco handlers.
The transactional outbox records delivery intent alongside application writes; it is not where the business job executes.
Switching to Valkey moves delivery and queue polling away from SQL queue tables, but application data, audit history, lifecycle admission and transactional publication remain in the application database.
It does not eliminate database access from a job that reads or changes entities.

`POST /api/migration-jobs/fill-defaults` now returns 202 with `schema_name` and `job_id`, instead of waiting for updates and returning counts.
The MCP tool returns the same acceptance payload.
Completed counts appear in the fill-defaults audit record, and undo uses the accepted `job_id` after completion.

Operational CLI tasks remain tasks when the caller intentionally waits for their result.
The reindex scheduler is a thin task calling the schedule model, which dispatches reindex jobs rather than embedding documents itself.
Queue consumers, outbox publication, lifecycle heartbeats and database-load monitoring remain infrastructure loops: making them depend on the queue they support would prevent recovery when that queue is unavailable.

If an entity write fails after a lifecycle record is saved, the dispatch failure appears in lifecycle diagnostics and logs.
If the queue is unavailable, the job is marked `unavailable` and the request receives an error.

## Operations

Run workers with explicit target tags.
A worker started without a target is rejected.
When a job starts, logs include `queue_start_seconds` and `lifecycle_id`.
Filter these tracing events in your logging system to monitor queue start times.
When Enterprise plan policy is enabled, a job delayed by its target limit emits `worker capacity saturated`.
Retries emit `retrying`; provider or policy failures emit `unavailable` with a diagnostic field.
Inspect lifecycle rows grouped by `worker_class` and `status`, then compare recorded start times with the plan objective.
If `queued`, `retrying`, or `unavailable` keeps increasing, check worker capacity and queue health before assessing start times.


## Priority and capacity

PostgreSQL and SQLite select eligible jobs by priority, scheduled time, then ID.
Redis checks only the first 1000 jobs by priority before matching worker targets.
If earlier jobs belong to other targets, a matching job beyond that window can wait for another polling cycle.

Yorishiro assigns `tenant_private` priority 300, `official` priority 200, and `shared` priority 100.

When Enterprise plan policy is enabled, Shared and Official limits apply across the deployment; tenant-private limits apply separately to each tenant.
Without plan policy, target admission has no concurrency cap.

The `enterprise` plan uses the Team limits.

When a target reaches its limit, the job waits for another attempt at its original priority and target instead of moving to another worker target.

Run target-specific processes to reserve worker capacity: `--worker=worker-target:shared`, `--worker=worker-target:official`, and `--worker=worker-target:tenant:<tenant-id>`.

A worker serving several targets shares its `num_workers` slots across them; it does not reserve a slot per target.

When a lower-priority job waits in `queued` or `retrying` for one minute, new higher-priority jobs sharing its workers receive priority 50 until the waiting job starts.
This does not change the priority of jobs already in the queue.
Separate workers for each target reserve worker capacity for that target.

Logs include scheduling decisions, job class, priority, configured limit, active jobs, and any capacity deferral.
