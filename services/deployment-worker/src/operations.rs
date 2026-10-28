// SPDX-License-Identifier: AGPL-3.0-only
//! Application operations: start, stop, shut down, restart, migrate and update carried out phase
//! by phase.
//!
//! The `application.operation` job is a coordinator. It queues the ordinary per-instance jobs
//! of one phase, waits for them, confirms what Incus then reports, and only then moves on. It
//! never calls Incus to change anything itself, so every change keeps the ownership checks,
//! fencing and retries of the instance jobs. The operation row records each step, so a crash
//! of the worker resumes where it stopped, and a failure is shown as what actually failed.
//!
//! Success is declared only when the last phase's instances are confirmed in the target state
//! by Incus. HTTP-level readiness is not checked (see `docs/architecture/current-state.md`).
//!
//! An `update` (a rolling replacement, [`crate::rollout`]) has two phases that queue no
//! instance job: `healthy` and `drain` are looked at here on every pass until they pass or fail.

use apelora_incus_adapter::Provider;
use apelora_persistence::lifecycle::{get_operation, update_operation};
use apelora_persistence::{
    ClaimedJob, JobOrigin, Outcome, Pool, StoreError, enqueue_job_from, job_status,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Step {
    pub(crate) project: String,
    pub(crate) name: String,
    #[serde(alias = "appliance")]
    pub(crate) module: String,
    #[serde(default)]
    pub(crate) job: Option<Uuid>,
    /// `pending`, `running`, `succeeded`, `failed`, `skipped`.
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) error: Option<String>,
    /// What a `migrate` step asks of `instance.move`, or a `create` step of `instance.create`:
    /// the request the control plane checked when the operation was accepted. `healthy` and
    /// `drain` steps keep their progress here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) payload: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Phase {
    /// `start`, `stop` (graceful: each guest is asked to shut down; also what operations
    /// recorded before ADR 0015 say), `poweroff` (immediate), `migrate`, or one of an update's
    /// `create`, `healthy`, `drain`, `delete` and `rebuild`.
    action: String,
    state: String,
    steps: Vec<Step>,
}

/// How long to wait between looks at a phase's jobs.
const POLL_SECS: u32 = 2;

/// Marks the phase at `from` and every later one as skipped.
fn skip_from(phases: &mut [Phase], from: usize) {
    for p in phases.iter_mut().skip(from) {
        p.state = "skipped".into();
        for s in &mut p.steps {
            s.state = "skipped".into();
        }
    }
}

pub async fn run(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
) -> Result<Outcome, StoreError> {
    let Some(op_id) = job
        .payload
        .get("operation")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok())
    else {
        return Ok(Outcome::Rejected("the job names no operation".into()));
    };
    let Some(op) = get_operation(pool, op_id).await? else {
        return Ok(Outcome::Rejected(format!(
            "operation {op_id} does not exist"
        )));
    };
    if matches!(op.state.as_str(), "succeeded" | "failed" | "cancelled") {
        return Ok(Outcome::Succeeded);
    }
    let Ok(mut phases) = serde_json::from_value::<Vec<Phase>>(op.phases.clone()) else {
        tracing::error!(operation = %op_id, "operation: its plan is unreadable; marked failed");
        update_operation(
            pool,
            op_id,
            "failed",
            &op.phases,
            Some("the operation's plan is unreadable"),
        )
        .await?;
        return Ok(Outcome::Rejected("unreadable phases".into()));
    };
    let Some(at) = phases.iter().position(|p| p.state != "succeeded") else {
        update_operation(pool, op_id, "succeeded", &json!(phases), None).await?;
        return Ok(Outcome::Succeeded);
    };

    let save = |state: &str, phases: &[Phase], error: Option<&str>| {
        let pool = pool.clone();
        let value = json!(phases);
        let state = state.to_owned();
        let error = error.map(str::to_owned);
        let id = op_id;
        async move { update_operation(&pool, id, &state, &value, error.as_deref()).await }
    };

    let kind = match phases[at].action.as_str() {
        "start" => "instance.start",
        "migrate" => "instance.move",
        "create" => "instance.create",
        "rebuild" => "instance.rebuild",
        "delete" => "instance.delete",
        _ => "instance.stop",
    };
    let target_status = if matches!(phases[at].action.as_str(), "start" | "create" | "rebuild") {
        "Running"
    } else {
        "Stopped"
    };
    // An update's checks: no instance job, looked at on every pass.
    let checks = matches!(phases[at].action.as_str(), "healthy" | "drain");

    // Cancellation stops between phases: a phase not started yet is skipped with all after it;
    // a running phase's instance jobs are left to finish (each stops at its own safe point).
    let cancel_by = if job.cancel_requested {
        apelora_persistence::jobs::cancel_requested(pool, job.id).await?
    } else {
        None
    };
    if let Some(by) = &cancel_by
        && phases[at].state == "pending"
    {
        let note = format!(
            "cancelled by {by} before phase {} ({})",
            at + 1,
            phases[at].action
        );
        skip_from(&mut phases, at);
        save("cancelled", &phases, Some(&note)).await?;
        tracing::info!(operation = %op_id, phase = at, "operation: cancelled between phases");
        return Ok(Outcome::Cancelled(note));
    }

    // First look at this phase: queue the instance jobs, under the request that asked for the
    // operation.
    if phases[at].state == "pending" && checks {
        phases[at].state = "running".into();
    } else if phases[at].state == "pending" {
        let phases_action = phases[at].action.clone();
        for (i, s) in phases[at].steps.iter_mut().enumerate() {
            let key = format!("application.operation:{op_id}:{at}:{i}");
            let mut conn = pool.acquire().await?;
            let id = enqueue_job_from(
                &mut conn,
                kind,
                Some(op.application_id),
                &match phases_action.as_str() {
                    "migrate" | "create" | "rebuild" => s
                        .payload
                        .clone()
                        .unwrap_or_else(|| json!({ "project": s.project, "name": s.name })),
                    "poweroff" => json!({ "project": s.project, "name": s.name, "force": true }),
                    _ => json!({ "project": s.project, "name": s.name }),
                },
                Some(&key),
                &JobOrigin {
                    request_id: job.request_id.as_deref(),
                    queued_by: Some(&op.requested_by),
                    retry_of: None,
                },
            )
            .await?
            .id();
            s.job = Some(id);
            s.state = "running".into();
        }
        phases[at].state = "running".into();
        save("running", &phases, None).await?;
        tracing::info!(operation = %op_id, phase = at, action = %phases[at].action, instances = phases[at].steps.len(), "operation: phase started; instance jobs queued");
        return Ok(Outcome::Waiting(POLL_SECS));
    }

    // Later looks: read the jobs, and confirm against Incus what they report as done.
    let mut changed = false;
    if checks {
        let policy = crate::rollout::UpdatePolicy::from_env(|k| std::env::var(k).ok());
        let draining = phases[at].action == "drain";
        for (i, s) in phases[at]
            .steps
            .iter_mut()
            .enumerate()
            .filter(|(_, s)| matches!(s.state.as_str(), "pending" | "running"))
        {
            s.state = "running".into();
            let key = format!("application.operation:{op_id}:{at}:{i}");
            let check = if draining {
                crate::rollout::drain(pool, provider, job, &op, s, &key, policy).await?
            } else {
                crate::rollout::healthy(pool, provider, job, &op, s, &key, policy).await?
            };
            match check {
                crate::rollout::Check::Waiting => {}
                crate::rollout::Check::Passed => s.state = "succeeded".into(),
                crate::rollout::Check::Failed(why) => {
                    s.state = "failed".into();
                    s.error = Some(why);
                }
            }
            changed = true;
        }
    }
    let migrating = phases[at].action == "migrate";
    let deleting = phases[at].action == "delete";
    for s in phases[at]
        .steps
        .iter_mut()
        .filter(|s| !checks && s.state == "running")
    {
        let Some(job_id) = s.job else { continue };
        let (status, _, err) = job_status(pool, job_id).await?;
        match status.as_str() {
            "succeeded" => {
                let seen = provider.instance(&s.project, &s.name).await;
                // A migrated instance is confirmed on its new member, running again if it was.
                let moved_to = migrating
                    .then(|| {
                        s.payload
                            .as_ref()
                            .and_then(|p| p["target"].as_str())
                            .unwrap_or("")
                    })
                    .map(str::to_owned);
                let expected = if moved_to.is_some() {
                    if s.payload
                        .as_ref()
                        .is_some_and(|p| p["startAfter"] == json!(true))
                    {
                        "Running"
                    } else {
                        "Stopped"
                    }
                } else {
                    target_status
                };
                match seen {
                    // A deleted instance is confirmed by its absence.
                    Ok(None) if deleting => s.state = "succeeded".into(),
                    Ok(Some(_)) if deleting => {
                        s.state = "failed".into();
                        s.error = Some("Incus still reports the instance".into());
                    }
                    Ok(Some(i))
                        if i.status == expected
                            && moved_to.as_deref().is_none_or(|m| i.location == m) =>
                    {
                        s.state = "succeeded".into();
                    }
                    Ok(Some(i)) => {
                        s.state = "failed".into();
                        s.error = Some(match &moved_to {
                            Some(m) if i.location != *m => format!(
                                "Incus reports it on {} (expected {m})",
                                if i.location.is_empty() {
                                    "no member"
                                } else {
                                    &i.location
                                }
                            ),
                            _ => format!("Incus reports {} (expected {expected})", i.status),
                        });
                    }
                    Ok(None) => {
                        s.state = "failed".into();
                        s.error = Some("the instance no longer exists in Incus".into());
                    }
                    // Cannot confirm yet: look again on the next pass rather than assume.
                    Err(e) => {
                        tracing::warn!(operation = %op_id, instance = %format!("{}/{}", s.project, s.name), error = %e, "operation: cannot read the instance from Incus to confirm its state; looking again");
                        continue;
                    }
                }
                changed = true;
            }
            "failed" => {
                s.state = "failed".into();
                s.error = Some(err.unwrap_or_else(|| "the instance job failed".into()));
                changed = true;
            }
            _ => {}
        }
    }
    if let Some(bad) = phases[at]
        .steps
        .iter()
        .find(|s| s.state == "failed")
        .cloned()
    {
        let mut message = format!("{}: {}", bad.name, bad.error.clone().unwrap_or_default());
        // An update stops here: what this phase left half done is undone, and the replicas
        // not replaced yet keep serving.
        if op.kind == "update" {
            let action = phases[at].action.clone();
            let key = format!("application.operation:{op_id}:{at}");
            let note =
                crate::rollout::stopped(pool, job, &op, &action, &mut phases[at].steps, &key)
                    .await?;
            message = format!("update stopped: {message}; {note}");
        }
        let instance = format!("{}/{}", bad.project, bad.name);
        phases[at].state = "failed".into();
        for later in phases.iter_mut().skip(at + 1) {
            later.state = "skipped".into();
            for s in &mut later.steps {
                s.state = "skipped".into();
            }
        }
        save("failed", &phases, Some(&message)).await?;
        // The coordinating job did its work: the operation (not the job) records the failure.
        tracing::warn!(operation = %op_id, phase = at, action = %phases[at].action, %instance, reason = %message, "operation: phase failed; later phases skipped");
        return Ok(Outcome::Succeeded);
    }
    if phases[at].steps.iter().all(|s| s.state == "succeeded") {
        phases[at].state = "succeeded".into();
        let done = phases.iter().all(|p| p.state == "succeeded");
        if !done && let Some(by) = &cancel_by {
            let note = format!(
                "cancelled by {by} after phase {} ({}); later phases were not started",
                at + 1,
                phases[at].action
            );
            skip_from(&mut phases, at + 1);
            save("cancelled", &phases, Some(&note)).await?;
            tracing::info!(operation = %op_id, phase = at, "operation: cancelled between phases");
            return Ok(Outcome::Cancelled(note));
        }
        save(if done { "succeeded" } else { "running" }, &phases, None).await?;
        tracing::info!(operation = %op_id, phase = at, action = %phases[at].action, done, "operation: phase succeeded");
        return Ok(if done {
            Outcome::Succeeded
        } else {
            Outcome::Waiting(1)
        });
    }
    if changed {
        save("running", &phases, None).await?;
    }
    Ok(Outcome::Waiting(POLL_SECS))
}
