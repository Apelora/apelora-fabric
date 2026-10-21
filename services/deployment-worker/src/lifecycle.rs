// SPDX-License-Identifier: AGPL-3.0-only
//! Instance lifecycle jobs (`instance.create|start|stop|restart|delete|resize`).
//!
//! Level-triggered and idempotent: every step re-reads the instance from Incus and decides the
//! next call from what it finds, so a retried or reclaimed job converges instead of repeating
//! work. An operation started by an earlier attempt is waited on first. Instances without
//! Apelora's ownership marker are never changed.

use std::time::Duration;

use apelora_domain::{ClusterMember, InstanceAction, InstanceSpec, is_valid_name};
use apelora_incus_adapter::{AdapterError, InstanceState, OperationOutcome, Provider};
use apelora_persistence::{ClaimedJob, Outcome, Pool, StoreError, record_operation};
use serde::Deserialize;

/// How long one attempt waits on an Incus operation (image downloads can be slow). The lease
/// is renewed meanwhile.
const OPERATION_BUDGET: Duration = Duration::from_secs(600);
/// Calls one job may make; each converges one step (e.g. create, add volumes, start).
const MAX_STEPS: usize = 4;

/// What a lifecycle job wants to be true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Goal {
    Exists(Box<InstanceSpec>),
    State(Target, InstanceAction),
    /// Powered off at once, without waiting for the guest to shut down (`instance.stop` with
    /// `force`): an application's immediate Stop.
    PoweredOff(Target),
    /// Restarted once by this job (a stopped instance is started): an event, not a state, so
    /// the job is done as soon as one restart operation it started has finished.
    Restarted(Target),
    Absent(Target),
    /// Its resource limits are these (`None` removes one): CPU and memory changed in place.
    Limits(Target, std::collections::BTreeMap<String, Option<String>>),
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
            "instance.stop" if payload["force"].as_bool() == Some(true) => {
                target().map(Goal::PoweredOff)
            }
            "instance.stop" => target().map(|t| Goal::State(t, InstanceAction::Stop)),
            "instance.restart" => target().map(Goal::Restarted),
            "instance.delete" => target().map(Goal::Absent),
            "instance.resize" => target().and_then(|t| {
                let limits = serde_json::from_value::<
                    std::collections::BTreeMap<String, Option<String>>,
                >(payload["limits"].clone())
                .map_err(|e| format!("invalid payload: {e}"))?;
                if limits.is_empty()
                    || limits
                        .keys()
                        .any(|k| !apelora_domain::plan::RESIZABLE_KEYS.contains(&k.as_str()))
                {
                    return Err(
                        "limits must name only limits.cpu, limits.cpu.allowance or limits.memory"
                            .into(),
                    );
                }
                Ok(Goal::Limits(t, limits))
            }),
            _ => return None,
        })
    }

    fn target(&self) -> (&str, &str) {
        match self {
            Goal::Exists(s) => (&s.project, &s.name),
            Goal::State(t, _)
            | Goal::PoweredOff(t)
            | Goal::Restarted(t)
            | Goal::Absent(t)
            | Goal::Limits(t, _) => (&t.project, &t.name),
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
    /// Create the instance's missing volumes on its member and attach them.
    Volumes,
    Change(InstanceAction, bool),
    Delete,
    /// Change the instance's resource limits in place.
    Limits,
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
        (Goal::Exists(spec), Some(s)) => match image_mismatch(spec, s) {
            Some(why) => Step::Reject(format!(
                "instance {project}/{name} {why}; delete it to create it again"
            )),
            None if spec
                .volumes
                .iter()
                .any(|v| !s.volume_devices.contains(&v.device())) =>
            {
                Step::Volumes
            }
            None if spec.start && s.status != "Running" => {
                Step::Change(InstanceAction::Start, false)
            }
            None => Step::Done,
        },
        (Goal::State(..), None) => {
            Step::Reject(format!("instance {project}/{name} does not exist"))
        }
        (Goal::State(..), Some(s)) if !s.managed => not_ours(),
        (Goal::State(_, a), Some(s)) if s.status == a.target_status() => Step::Done,
        (Goal::State(_, a), Some(_)) => Step::Change(*a, false),
        (Goal::PoweredOff(_), None) => {
            Step::Reject(format!("instance {project}/{name} does not exist"))
        }
        (Goal::PoweredOff(_), Some(s)) if !s.managed => not_ours(),
        (Goal::PoweredOff(_), Some(s)) if s.status == "Stopped" => Step::Done,
        (Goal::PoweredOff(_), Some(_)) => Step::Change(InstanceAction::Stop, true),
        (Goal::Restarted(_), None) => {
            Step::Reject(format!("instance {project}/{name} does not exist"))
        }
        (Goal::Restarted(_), Some(s)) if !s.managed => not_ours(),
        (Goal::Restarted(_), Some(s)) if s.status != "Running" => {
            Step::Change(InstanceAction::Start, false)
        }
        // A replica restarted for failing its check is already out of its service: no grace.
        (Goal::Restarted(_), Some(_)) => Step::Change(InstanceAction::Restart, true),
        (Goal::Absent(_), None) => Step::Done,
        (Goal::Absent(_), Some(s)) if !s.managed => not_ours(),
        // Incus refuses to delete a running instance; it is being deleted anyway.
        (Goal::Absent(_), Some(s)) if s.status != "Stopped" => {
            Step::Change(InstanceAction::Stop, true)
        }
        (Goal::Absent(_), Some(_)) => Step::Delete,
        (Goal::Limits(..), None) => {
            Step::Reject(format!("instance {project}/{name} does not exist"))
        }
        (Goal::Limits(..), Some(s)) if !s.managed => not_ours(),
        (Goal::Limits(_, wanted), Some(s)) => {
            // What the limits would be after the change; nothing to do when that is what they are.
            let mut after = s.limits.clone();
            for (k, v) in wanted {
                match v {
                    Some(v) => after.insert(k.clone(), v.clone()),
                    None => after.remove(k),
                };
            }
            if apelora_domain::plan::limit_changes(&after, &s.limits).is_empty() {
                Step::Done
            } else {
                Step::Limits
            }
        }
    }
}

/// Why an existing instance does not match the image the spec pins, if it does not.
fn image_mismatch(spec: &InstanceSpec, now: &InstanceState) -> Option<String> {
    if let Some(fp) = &spec.source.fingerprint
        && now.base_image.as_deref() != Some(fp.as_str())
    {
        return Some(format!(
            "was built from image {}, not the pinned {fp}",
            now.base_image.as_deref().unwrap_or("(unknown)")
        ));
    }
    if !spec.architectures.is_empty() && !spec.architectures.contains(&now.architecture) {
        return Some(format!(
            "runs on {}, which the image does not support ({})",
            now.architecture,
            spec.architectures.join(", ")
        ));
    }
    None
}

/// Before creating: will Incus put the instance on an architecture the image supports?
/// Incus places instances; this refuses, before anything is created, specs it cannot be sure
/// of. `Err((true, _))` may succeed later (suitable members exist but none is online).
///
/// `server_architectures` is used for a standalone server (no members). In a cluster that
/// mixes architectures, Incus may place an image offered for several architectures on any of
/// them, so only a fingerprint pin (one image, one architecture) is accepted there.
pub fn placeable(
    spec: &InstanceSpec,
    server_architectures: &[String],
    members: &[ClusterMember],
) -> Result<(), (bool, String)> {
    if spec.architectures.is_empty() {
        return Ok(());
    }
    let wanted = spec.architectures.join(", ");
    let supported = |arch: &String| spec.architectures.contains(arch);
    if members.is_empty() {
        return if server_architectures.iter().any(supported) {
            Ok(())
        } else {
            Err((
                false,
                format!("the server does not run a supported architecture ({wanted})"),
            ))
        };
    }
    let online = || members.iter().filter(|m| m.status == "Online");
    if !members.iter().any(|m| supported(&m.architecture)) {
        return Err((
            false,
            format!("no cluster member has a supported architecture ({wanted})"),
        ));
    }
    if !online().any(|m| supported(&m.architecture)) {
        return Err((true, format!("no {wanted} member is online")));
    }
    if spec.source.fingerprint.is_none() && online().any(|m| !supported(&m.architecture)) {
        return Err((
            false,
            format!(
                "the cluster also has members of other architectures and Incus could place the \
                 instance on one; pin the image by fingerprint to run it only on {wanted}"
            ),
        ));
    }
    Ok(())
}

/// Whether the job may stop on request before taking `step`, and what it would have done. Each
/// Incus change is one operation that is always waited on to its end; between them the
/// instance is in a state Incus reports as complete. A create stops only before the instance
/// exists: once it does, its volumes are attached and it is started, never left half-built.
pub fn safe_point(goal: &Goal, step: &Step) -> Option<&'static str> {
    match (goal, step) {
        (_, Step::Done | Step::Reject(_)) => None,
        (Goal::Exists(_), Step::Create) => Some("creating"),
        (Goal::Exists(_), _) => None,
        (_, Step::Change(InstanceAction::Start, _)) => Some("starting"),
        (_, Step::Change(InstanceAction::Stop, _)) => Some("stopping"),
        (_, Step::Change(InstanceAction::Restart, _)) => Some("restarting"),
        (_, Step::Delete) => Some("deleting"),
        (_, Step::Limits) => Some("resizing"),
        (_, Step::Create | Step::Volumes) => None,
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
            // A restart an earlier attempt started has happened (or been overtaken): done.
            Ok(OperationOutcome::Succeeded) if matches!(goal, Goal::Restarted(_)) => {
                record_operation(pool, job, worker, None).await?;
                return Ok(Outcome::Succeeded);
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
        let step = next_step(goal, now.as_ref());
        tracing::debug!(instance = %format!("{project}/{name}"), observed = now.as_ref().map_or("absent", |s| s.status.as_str()), ?step, "lifecycle: next step");
        if let Some(what) = safe_point(goal, &step)
            && let Some(by) = apelora_persistence::jobs::cancel_requested(pool, job.id).await?
        {
            return Ok(Outcome::Cancelled(format!(
                "cancelled by {by} before {what} {project}/{name}"
            )));
        }
        let op = match step {
            Step::Done => return Ok(Outcome::Succeeded),
            Step::Reject(msg) => return Ok(Outcome::Rejected(msg)),
            Step::Create => {
                let Goal::Exists(spec) = goal else {
                    unreachable!("create is only decided for Exists")
                };
                let fits = if spec.architectures.is_empty() {
                    Ok(())
                } else {
                    let server = provider.server_info().await?;
                    let members = if server.clustered {
                        provider.cluster_members().await?
                    } else {
                        Vec::new()
                    };
                    placeable(spec, &server.architectures, &members)
                };
                // The NIC joins the application's network with its own ACL: both must exist.
                if let Some(n) = &spec.network {
                    let app = spec
                        .owner
                        .as_ref()
                        .map_or(n.network.as_str(), |o| o.application.as_str());
                    provider
                        .ensure_network(project, &n.network, &n.uplink, app)
                        .await?;
                    provider.ensure_acl(project, &n.acl, app).await?;
                    provider.ensure_address_set(project, &n.acl, app).await?;
                }
                // Incus does not place an instance next to a member-local volume: create it
                // stopped, then its volumes where Incus put it, then start it.
                let spec = if spec.volumes.is_empty() {
                    std::borrow::Cow::Borrowed(spec.as_ref())
                } else {
                    std::borrow::Cow::Owned(InstanceSpec {
                        start: false,
                        ..spec.as_ref().clone()
                    })
                };
                match fits {
                    Ok(()) => provider.create_instance(&spec).await?,
                    Err((false, msg)) => return Ok(Outcome::Rejected(msg)),
                    Err((true, msg)) => return Err(Failure::Retry(msg)),
                }
            }
            Step::Volumes => {
                let (Goal::Exists(spec), Some(now)) = (goal, now.as_ref()) else {
                    unreachable!("volumes are only decided for an existing instance")
                };
                let Some(owner) = &spec.owner else {
                    return Ok(Outcome::Rejected(
                        "volumes can only be created for an application's instance".into(),
                    ));
                };
                for v in spec
                    .volumes
                    .iter()
                    .filter(|v| !now.volume_devices.contains(&v.device()))
                {
                    // A volume on the shared pool belongs to no member (ADR 0013).
                    let member = if v.class.shared() {
                        ""
                    } else {
                        now.location.as_str()
                    };
                    provider
                        .ensure_volume(project, member, name, v, owner)
                        .await?;
                    // Recorded as soon as it exists: data is never forgotten, attached or not.
                    apelora_persistence::record_volume(
                        pool,
                        &apelora_persistence::NewVolume {
                            application: &owner.application,
                            project,
                            module: &owner.module,
                            instance: name,
                            name: &v.name,
                            incus_name: &apelora_domain::storage::volume_name(name, &v.name),
                            storage_pool: v.pool.as_deref().unwrap_or_default(),
                            member,
                            size_mib: v.size_mib,
                            content: v.content.as_str(),
                            class: v.class.as_str(),
                        },
                    )
                    .await?;
                    provider.attach_volume(project, name, v).await?;
                    tracing::info!(job = %job.id, instance = %format!("{project}/{name}"), volume = %v.name, member = %now.location, "volume attached");
                }
                continue;
            }
            Step::Change(action, force) => {
                provider.change_state(project, name, action, force).await?
            }
            Step::Delete => provider.delete_instance(project, name).await?,
            Step::Limits => {
                let Goal::Limits(_, wanted) = goal else {
                    unreachable!("limits are only decided for a Limits goal")
                };
                // Incus may have applied it already: the next pass reads the instance again.
                match provider.set_instance_limits(project, name, wanted).await? {
                    Some(op) => op,
                    None => continue,
                }
            }
        };
        record_operation(pool, job, worker, Some(&op)).await?;
        tracing::info!(job = %job.id, operation = %op, instance = %format!("{project}/{name}"), provider = ?provider.mode(), "started operation");
        match provider.wait_operation(&op, OPERATION_BUDGET).await? {
            OperationOutcome::Succeeded => {
                record_operation(pool, job, worker, None).await?;
                if matches!(goal, Goal::Restarted(_)) {
                    return Ok(Outcome::Succeeded);
                }
            }
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
                fingerprint: None,
            },
            profiles: vec![],
            config: Default::default(),
            start,
            architectures: vec![],
            owner: None,
            network: None,
            volumes: Vec::new(),
            root_pool: None,
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
            architecture: "aarch64".into(),
            base_image: Some(FP.into()),
            location: "member-1".into(),
            volume_devices: Vec::new(),
            kind: apelora_domain::InstanceKind::Container,
            stateful: false,
            limits: Default::default(),
        })
    }

    const FP: &str = "aa11bb22cc33dd44ee55ff6600778899aabbccddeeff00112233445566778899";

    fn pinned(fingerprint: &str, architectures: &[&str]) -> InstanceSpec {
        let Goal::Exists(mut s) = spec(true) else {
            unreachable!()
        };
        s.source.fingerprint = Some(fingerprint.into());
        s.architectures = architectures.iter().map(|a| (*a).to_owned()).collect();
        *s
    }

    #[test]
    fn existing_instances_must_match_the_pinned_image() {
        let ok = Goal::Exists(Box::new(pinned(FP, &["aarch64"])));
        assert_eq!(next_step(&ok, st("Running", true).as_ref()), Step::Done);
        let other = Goal::Exists(Box::new(pinned(&"0".repeat(64), &[])));
        assert!(
            matches!(next_step(&other, st("Stopped", true).as_ref()), Step::Reject(m) if m.contains("not the pinned")),
            "a stopped instance from another image is not started"
        );
        let arch = Goal::Exists(Box::new(pinned(FP, &["x86_64"])));
        assert!(
            matches!(next_step(&arch, st("Running", true).as_ref()), Step::Reject(m) if m.contains("runs on aarch64"))
        );
    }

    #[test]
    fn creation_needs_a_member_of_a_supported_architecture() {
        let member = |arch: &str, status: &str| ClusterMember {
            name: format!("m-{arch}"),
            url: String::new(),
            roles: vec![],
            status: status.into(),
            message: String::new(),
            architecture: arch.into(),
            failure_domain: String::new(),
            database: false,
            groups: vec![],
            scheduler: "all".into(),
        };
        let x86 = pinned(FP, &["x86_64"]);
        let mut x86_by_alias = x86.clone();
        x86_by_alias.source.fingerprint = None;
        let arm = ["aarch64".to_owned()];
        let mixed = [member("aarch64", "Online"), member("x86_64", "Online")];
        // Standalone servers are checked against their own architectures.
        assert!(matches!(placeable(&x86, &arm, &[]), Err((false, _))));
        assert_eq!(placeable(&x86, &["x86_64".to_owned()], &[]), Ok(()));
        assert_eq!(
            placeable(&pinned(FP, &[]), &[], &[member("aarch64", "Online")]),
            Ok(())
        );
        assert_eq!(
            placeable(&x86, &[], &mixed),
            Ok(()),
            "a fingerprint fixes the architecture"
        );
        assert!(
            matches!(placeable(&x86_by_alias, &[], &mixed), Err((false, m)) if m.contains("pin the image by fingerprint")),
            "an alias could be placed on the other architecture"
        );
        assert_eq!(
            placeable(&x86_by_alias, &[], &[member("x86_64", "Online")]),
            Ok(())
        );
        assert!(matches!(
            placeable(&x86, &[], &[member("aarch64", "Online")]),
            Err((false, _))
        ));
        assert!(matches!(
            placeable(&x86, &[], &[member("x86_64", "Offline")]),
            Err((true, _))
        ));
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
            (Goal::PoweredOff(t()), None, Step::Reject(String::new())),
            (Goal::PoweredOff(t()), st("Stopped", true), Step::Done),
            (
                Goal::PoweredOff(t()),
                st("Running", true),
                Step::Change(Stop, true),
            ),
            (
                Goal::PoweredOff(t()),
                st("Running", false),
                Step::Reject(String::new()),
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
        // `force` makes the stop an immediate power-off; anything else stays graceful.
        let forced = serde_json::json!({ "project": "apelora", "name": "web-01", "force": true });
        assert_eq!(
            Goal::from_job("instance.stop", &forced),
            Some(Ok(Goal::PoweredOff(t())))
        );
        let not_forced =
            serde_json::json!({ "project": "apelora", "name": "web-01", "force": false });
        assert_eq!(
            Goal::from_job("instance.stop", &not_forced),
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

    #[test]
    fn a_restart_restarts_a_running_instance_and_starts_a_stopped_one() {
        let goal = Goal::Restarted(t());
        assert_eq!(
            next_step(&goal, st("Running", true).as_ref()),
            Step::Change(InstanceAction::Restart, true)
        );
        assert_eq!(
            next_step(&goal, st("Stopped", true).as_ref()),
            Step::Change(InstanceAction::Start, false)
        );
        assert!(matches!(next_step(&goal, None), Step::Reject(_)));
        assert!(matches!(
            next_step(&goal, st("Running", false).as_ref()),
            Step::Reject(_)
        ));
        let payload = serde_json::json!({ "project": "apelora", "name": "web-01" });
        assert!(matches!(
            Goal::from_job("instance.restart", &payload),
            Some(Ok(Goal::Restarted(_)))
        ));
    }

    fn limits_goal(pairs: &[(&str, Option<&str>)]) -> Goal {
        Goal::Limits(
            Target {
                project: "apelora".into(),
                name: "web-01".into(),
            },
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
                .collect(),
        )
    }

    #[test]
    fn a_resize_changes_limits_until_they_are_what_was_asked_and_touches_nothing_else() {
        let goal = limits_goal(&[
            ("limits.memory", Some("1GiB")),
            ("limits.cpu.allowance", None),
        ]);
        let mut now = st("Running", true).expect("state");
        now.limits.insert("limits.memory".into(), "512MiB".into());
        now.limits
            .insert("limits.cpu.allowance".into(), "50ms/100ms".into());
        assert_eq!(next_step(&goal, Some(&now)), Step::Limits);
        // One limit done, one to go.
        now.limits.insert("limits.memory".into(), "1024MiB".into());
        assert_eq!(
            next_step(&goal, Some(&now)),
            Step::Limits,
            "1GiB is 1024MiB; the quota is still set"
        );
        now.limits.remove("limits.cpu.allowance");
        assert_eq!(
            next_step(&goal, Some(&now)),
            Step::Done,
            "a retried or repeated job changes nothing"
        );
        // Not Apelora's, or gone: never changed.
        assert!(matches!(
            next_step(&goal, st("Running", false).as_ref()),
            Step::Reject(_)
        ));
        assert!(matches!(next_step(&goal, None), Step::Reject(_)));
        // A stopped instance is resized too.
        let mut stopped = st("Stopped", true).expect("state");
        stopped.limits.insert("limits.cpu".into(), "1".into());
        assert_eq!(
            next_step(&limits_goal(&[("limits.cpu", Some("2"))]), Some(&stopped)),
            Step::Limits
        );
        assert_eq!(safe_point(&goal, &Step::Limits), Some("resizing"));
    }

    #[test]
    fn a_resize_job_names_only_resizable_limits() {
        let ok = serde_json::json!({ "project": "apelora", "name": "web-01", "limits": { "limits.memory": "2GiB", "limits.cpu.allowance": null } });
        assert!(matches!(
            Goal::from_job("instance.resize", &ok),
            Some(Ok(Goal::Limits(..)))
        ));
        for bad in [
            serde_json::json!({ "project": "apelora", "name": "web-01", "limits": {} }),
            serde_json::json!({ "project": "apelora", "name": "web-01" }),
            serde_json::json!({ "project": "apelora", "name": "web-01", "limits": { "security.privileged": "true" } }),
            serde_json::json!({ "project": "apelora", "name": "../x", "limits": { "limits.cpu": "1" } }),
            serde_json::json!({ "project": "apelora", "name": "web-01", "limits": { "limits.cpu": 2 } }),
        ] {
            assert!(
                matches!(Goal::from_job("instance.resize", &bad), Some(Err(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_instance_gets_its_volumes_before_it_is_started() {
        let Goal::Exists(mut s) = spec(true) else {
            unreachable!()
        };
        s.volumes = vec![apelora_domain::storage::VolumeMount {
            name: "data".into(),
            size_mib: 1024,
            content: Default::default(),
            class: Default::default(),
            path: Some("/data".into()),
            pool: Some("local".into()),
        }];
        let goal = Goal::Exists(s);
        let mut now = InstanceState {
            base_image: None,
            ..st("Stopped", true).expect("state")
        };
        assert_eq!(next_step(&goal, None), Step::Create);
        assert_eq!(next_step(&goal, Some(&now)), Step::Volumes);
        now.volume_devices.push("vol-data".into());
        assert_eq!(
            next_step(&goal, Some(&now)),
            Step::Change(InstanceAction::Start, false)
        );
        now.status = "Running".into();
        assert_eq!(next_step(&goal, Some(&now)), Step::Done);
        // A volume added to the design of a running instance is attached to it.
        now.volume_devices.clear();
        assert_eq!(next_step(&goal, Some(&now)), Step::Volumes);
    }
}
