// SPDX-License-Identifier: AGPL-3.0-only
//! Application operations: start, stop and restart carried out phase by phase.
//!
//! The `application.operation` job is a coordinator. It queues the ordinary per-instance jobs
//! of one phase, waits for them, confirms what Incus then reports, and only then moves on. It
//! never calls Incus to change anything itself, so every change keeps the ownership checks,
//! fencing and retries of the instance jobs. The operation row records each step, so a crash
//! of the worker resumes where it stopped, and a failure is shown as what actually failed.
//!
//! Success is declared only when the last phase's instances are confirmed in the target state
//! by Incus. HTTP-level readiness is not checked (see `docs/architecture/current-state.md`).

use apelora_incus_adapter::Provider;
use apelora_persistence::lifecycle::{get_operation, update_operation};
use apelora_persistence::{ClaimedJob, Outcome, Pool, StoreError, enqueue_job, job_status};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Step {
    project: String,
    name: String,
    appliance: String,
    #[serde(default)]
    job: Option<Uuid>,
    /// `pending`, `running`, `succeeded`, `failed`, `skipped`.
    state: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Phase {
    /// `start` or `stop`.
    action: String,
    state: String,
    steps: Vec<Step>,
}

/// How long to wait between looks at a phase's jobs.
const POLL_SECS: u32 = 2;

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
    if op.state == "succeeded" || op.state == "failed" {
        return Ok(Outcome::Succeeded);
    }
    let Ok(mut phases) = serde_json::from_value::<Vec<Phase>>(op.phases.clone()) else {
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

    let kind = if phases[at].action == "start" {
        "instance.start"
    } else {
        "instance.stop"
    };
    let target_status = if phases[at].action == "start" {
        "Running"
    } else {
        "Stopped"
    };

    // First look at this phase: queue the instance jobs.
    if phases[at].state == "pending" {
        for (i, s) in phases[at].steps.iter_mut().enumerate() {
            let key = format!("application.operation:{op_id}:{at}:{i}");
            let id = enqueue_job(
                pool,
                kind,
                Some(op.application_id),
                &json!({ "project": s.project, "name": s.name }),
                Some(&key),
            )
            .await?
            .id();
            s.job = Some(id);
            s.state = "running".into();
        }
        phases[at].state = "running".into();
        save("running", &phases, None).await?;
        return Ok(Outcome::Waiting(POLL_SECS));
    }

    // Later looks: read the jobs, and confirm against Incus what they report as done.
    let mut changed = false;
    for s in phases[at].steps.iter_mut().filter(|s| s.state == "running") {
        let Some(job_id) = s.job else { continue };
        let (status, _, err) = job_status(pool, job_id).await?;
        match status.as_str() {
            "succeeded" => {
                let seen = provider.instance(&s.project, &s.name).await;
                match seen {
                    Ok(Some(i)) if i.status == target_status => s.state = "succeeded".into(),
                    Ok(Some(i)) => {
                        s.state = "failed".into();
                        s.error = Some(format!(
                            "Incus reports {} (expected {target_status})",
                            i.status
                        ));
                    }
                    Ok(None) => {
                        s.state = "failed".into();
                        s.error = Some("the instance no longer exists in Incus".into());
                    }
                    // Cannot confirm yet: look again on the next pass rather than assume.
                    Err(_) => continue,
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
    if let Some(bad) = phases[at].steps.iter().find(|s| s.state == "failed") {
        let message = format!("{}: {}", bad.name, bad.error.clone().unwrap_or_default());
        phases[at].state = "failed".into();
        for later in phases.iter_mut().skip(at + 1) {
            later.state = "skipped".into();
            for s in &mut later.steps {
                s.state = "skipped".into();
            }
        }
        save("failed", &phases, Some(&message)).await?;
        return Ok(Outcome::Succeeded);
    }
    if phases[at].steps.iter().all(|s| s.state == "succeeded") {
        phases[at].state = "succeeded".into();
        let done = phases.iter().all(|p| p.state == "succeeded");
        save(if done { "succeeded" } else { "running" }, &phases, None).await?;
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
