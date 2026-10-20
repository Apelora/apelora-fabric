// SPDX-License-Identifier: AGPL-3.0-only
//! Instance lifecycle jobs (`instance.create|start|stop|delete`).
//!
//! Level-triggered and idempotent: every step re-reads the instance from Incus and decides the
//! next call from what it finds, so a retried or reclaimed job converges instead of repeating
//! work. An operation started by an earlier attempt is waited on first. Instances without
//! Apelora's ownership marker are never changed.

use std::time::Duration;

use apelora_domain::{InstanceAction, InstanceSpec, is_valid_name};
use apelora_incus_adapter::{AdapterError, InstanceState, OperationOutcome, Provider};
use apelora_persistence::{ClaimedJob, Outcome, Pool, StoreError, record_operation};
use serde::Deserialize;

/// How long one attempt waits on an Incus operation (image downloads can be slow). The lease
/// is renewed meanwhile.
const OPERATION_BUDGET: Duration = Duration::from_secs(600);
/// Calls one job may make; each converges one step (e.g. stop, then delete).
const MAX_STEPS: usize = 4;

/// What a lifecycle job wants to be true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Goal {
    Exists(Box<InstanceSpec>),
    State(Target, InstanceAction),
    Absent(Target),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Target {
    pub project: String,
    pub name: String,
}

impl Goal {
    /// `None` for kinds this module does not handle.
    pub fn from_job(kind: &str, payload: &serde_json::Value) -> Option<Result<Goal, String>> {
        let target = || {
            serde_json::from_value::<Target>(payload.clone())
                .map_err(|e| format!("invalid payload: {e}"))
                .and_then(|t| {
                    if is_valid_name(&t.project) && is_valid_name(&t.name) {
                        Ok(t)
                    } else {
                        Err("invalid project or instance name".into())
                    }
                })
        };
        Some(match kind {
            "instance.create" => serde_json::from_value::<InstanceSpec>(payload.clone())
                .map_err(|e| format!("invalid payload: {e}"))
                .and_then(|s| s.validate().map(|()| Goal::Exists(Box::new(s)))),
            "instance.start" => target().map(|t| Goal::State(t, InstanceAction::Start)),
            "instance.stop" => target().map(|t| Goal::State(t, InstanceAction::Stop)),
            "instance.delete" => target().map(Goal::Absent),
            _ => return None,
        })
    }

    fn target(&self) -> (&str, &str) {
        match self {
            Goal::Exists(s) => (&s.project, &s.name),
            Goal::State(t, _) | Goal::Absent(t) => (&t.project, &t.name),
        }
    }
}

/// The next call to make, decided only from the goal and what Incus reports now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Done,
    /// Cannot be achieved by retrying.
    Reject(String),
    Create,
    Change(InstanceAction, bool),
    Delete,
}

pub fn next_step(goal: &Goal, now: Option<&InstanceState>) -> Step {
    let (project, name) = goal.target();
    let not_ours = || {
        Step::Reject(format!(
            "instance {project}/{name} exists but was not created by Apelora; it is left untouched"
        ))
    };
    match (goal, now) {
        (Goal::Exists(_), None) => Step::Create,
        (Goal::Exists(_), Some(s)) if !s.managed => not_ours(),
        (Goal::Exists(spec), Some(s)) if spec.start && s.status != "Running" => {
            Step::Change(InstanceAction::Start, false)
        }
        (Goal::Exists(_), Some(_)) => Step::Done,
        (Goal::State(..), None) => {
            Step::Reject(format!("instance {project}/{name} does not exist"))
        }
        (Goal::State(..), Some(s)) if !s.managed => not_ours(),
        (Goal::State(_, a), Some(s)) if s.status == a.target_status() => Step::Done,
        (Goal::State(_, a), Some(_)) => Step::Change(*a, false),
        (Goal::Absent(_), None) => Step::Done,
        (Goal::Absent(_), Some(s)) if !s.managed => not_ours(),
        // Incus refuses to delete a running instance; it is being deleted anyway.
        (Goal::Absent(_), Some(s)) if s.status != "Stopped" => {
            Step::Change(InstanceAction::Stop, true)
        }
        (Goal::Absent(_), Some(_)) => Step::Delete,
    }
}

enum Failure {
    Retry(String),
    Store(StoreError),
}

impl From<AdapterError> for Failure {
    fn from(e: AdapterError) -> Self {
        Failure::Retry(e.to_string())
    }
}

impl From<StoreError> for Failure {
    fn from(e: StoreError) -> Self {
        Failure::Store(e)
    }
}

/// Runs a lifecycle job to completion (or to a retryable failure). Store errors, including
/// losing the job to another worker, are returned so the caller stops touching the job.
pub async fn run(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    worker: &str,
    goal: Goal,
) -> Result<Outcome, StoreError> {
    match converge(pool, provider, job, worker, &goal).await {
        Ok(o) => Ok(o),
        Err(Failure::Retry(msg)) => Ok(Outcome::Failed(msg)),
        Err(Failure::Store(e)) => Err(e),
    }
}

async fn converge(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    worker: &str,
    goal: &Goal,
) -> Result<Outcome, Failure> {
    if let Some(op) = &job.external_operation {
        tracing::info!(job = %job.id, operation = %op, "waiting on the operation an earlier attempt started");
        match provider.wait_operation(op, OPERATION_BUDGET).await {
            Ok(OperationOutcome::StillRunning) => {
                return Ok(Outcome::Failed(format!(
                    "Incus operation {op} is still running"
                )));
            }
            // Finished, failed or no longer known to Incus: decide again from the instance.
            Ok(_) | Err(AdapterError::NotFound(_)) => {
                record_operation(pool, job, worker, None).await?
            }
            Err(e) => return Err(e.into()),
        }
    }
    let (project, name) = goal.target();
    for _ in 0..MAX_STEPS {
        let now = provider.instance(project, name).await?;
        let op = match next_step(goal, now.as_ref()) {
            Step::Done => return Ok(Outcome::Succeeded),
            Step::Reject(msg) => return Ok(Outcome::Rejected(msg)),
            Step::Create => {
                let Goal::Exists(spec) = goal else {
                    unreachable!("create is only decided for Exists")
                };
                provider.create_instance(spec).await?
            }
            Step::Change(action, force) => {
                provider.change_state(project, name, action, force).await?
            }
            Step::Delete => provider.delete_instance(project, name).await?,
        };
        record_operation(pool, job, worker, Some(&op)).await?;
        tracing::info!(job = %job.id, operation = %op, instance = %format!("{project}/{name}"), provider = ?provider.mode(), "started operation");
        match provider.wait_operation(&op, OPERATION_BUDGET).await? {
            OperationOutcome::Succeeded => record_operation(pool, job, worker, None).await?,
            OperationOutcome::Failed(e) => {
                record_operation(pool, job, worker, None).await?;
                return Err(Failure::Retry(format!("Incus operation {op} failed: {e}")));
            }
            OperationOutcome::StillRunning => {
                return Ok(Outcome::Failed(format!(
                    "Incus operation {op} is still running"
                )));
            }
        }
    }
    Ok(Outcome::Failed(format!(
        "instance {project}/{name} did not converge in {MAX_STEPS} steps"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_domain::{ImageProtocol, ImageSource, InstanceKind};

    fn spec(start: bool) -> Goal {
        Goal::Exists(Box::new(InstanceSpec {
            project: "apelora".into(),
            name: "web-01".into(),
            kind: InstanceKind::Container,
            source: ImageSource {
                protocol: ImageProtocol::Simplestreams,
                server: "https://images.example".into(),
                alias: "debian/13".into(),
            },
            profiles: vec![],
            config: Default::default(),
            start,
        }))
    }
    fn t() -> Target {
        Target {
            project: "apelora".into(),
            name: "web-01".into(),
        }
    }
    fn st(status: &str, managed: bool) -> Option<InstanceState> {
        Some(InstanceState {
            status: status.into(),
            managed,
        })
    }

    #[test]
    fn decisions_converge_and_never_touch_foreign_instances() {
        use InstanceAction::*;
        let cases: Vec<(Goal, Option<InstanceState>, Step)> = vec![
            (spec(true), None, Step::Create),
            (spec(true), st("Stopped", true), Step::Change(Start, false)),
            (spec(true), st("Running", true), Step::Done),
            (spec(false), st("Stopped", true), Step::Done),
            (Goal::State(t(), Start), None, Step::Reject(String::new())),
            (
                Goal::State(t(), Start),
                st("Stopped", true),
                Step::Change(Start, false),
            ),
            (Goal::State(t(), Stop), st("Stopped", true), Step::Done),
            (
                Goal::State(t(), Stop),
                st("Frozen", true),
                Step::Change(Stop, false),
            ),
            (Goal::Absent(t()), None, Step::Done),
            (
                Goal::Absent(t()),
                st("Running", true),
                Step::Change(Stop, true),
            ),
            (Goal::Absent(t()), st("Stopped", true), Step::Delete),
        ];
        for (goal, now, want) in cases {
            let got = next_step(&goal, now.as_ref());
            match (&want, &got) {
                (Step::Reject(_), Step::Reject(_)) => {}
                _ => assert_eq!(got, want, "{goal:?} with {now:?}"),
            }
        }
        for goal in [spec(true), Goal::State(t(), Stop), Goal::Absent(t())] {
            assert!(
                matches!(next_step(&goal, st("Running", false).as_ref()), Step::Reject(m) if m.contains("not created by Apelora")),
                "{goal:?}"
            );
        }
    }

    #[test]
    fn payloads_are_validated() {
        let ok = serde_json::json!({ "project": "apelora", "name": "web-01" });
        assert_eq!(
            Goal::from_job("instance.stop", &ok),
            Some(Ok(Goal::State(t(), InstanceAction::Stop)))
        );
        let bad = serde_json::json!({ "project": "apelora", "name": "../x" });
        assert!(matches!(
            Goal::from_job("instance.delete", &bad),
            Some(Err(_))
        ));
        assert!(matches!(
            Goal::from_job("instance.create", &serde_json::json!({})),
            Some(Err(_))
        ));
        assert_eq!(Goal::from_job("noop", &ok), None);
    }
}
