// SPDX-License-Identifier: AGPL-3.0-only
//! The error codes each operation can answer, by status, for the OpenAPI document: the
//! [`super::ErrorResponses`] modifier documents every status listed here and names its codes, so a
//! client knows which `error.code` values to handle. 401 and 429 apply to every operation and are
//! added there. A test keeps the table to operations that exist and codes the handlers use.
//! Read from the handlers and the helpers they call; update it with the handler.

pub(super) type Codes = &'static [(&'static str, &'static [&'static str])];

pub(super) const ERROR_CODES: &[(&str, Codes)] = &[
    (
        "acknowledge_alert",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["already_acknowledged", "not_firing"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "add_group_member",
        &[
            ("403", &["forbidden", "membership_refused", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["last_super_admin", "managed_by_provider"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "add_sector",
        &[
            ("400", &["invalid_request", "invalid_sector"]),
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["sector_exists"]),
            ("500", &["internal", "invalid_place"]),
            ("503", &["no_database"]),
        ],
    ),
    (
        "admit_machine",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["admission_blocked", "not_a_member"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "alert_channels",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "alert_history",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "alert_rules",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "alerts",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_access",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_draft",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["no_draft", "not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_events",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_networks",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_operation",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_operations",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_status",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
        ],
    ),
    (
        "application_version",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_versions",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "application_volumes",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "applications",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "approvals",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "approve_machine",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &[
                    "already_a_member",
                    "approval_invalid",
                    "approval_pending",
                    "approval_required",
                ],
            ),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "approve_request",
        &[
            (
                "403",
                &["approval_needs_a_person", "forbidden", "mfa_required"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "archive_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["still_running"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "assignments",
        &[
            ("400", &["invalid_request"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "audit",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "audit_export",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "back_up_volume",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["backup_pool_missing"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "cancel_job",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            ("409", &["job_finished"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("cancel_request", &[("503", &["database_not_configured"])]),
    ("capacity", &[("403", &["forbidden", "mfa_required"])]),
    (
        "change_cluster_settings",
        &[
            ("400", &["invalid_request", "invalid_setting"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &["approval_invalid", "approval_required", "nothing_to_change"],
            ),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "change_password",
        &[
            (
                "400",
                &["invalid_request", "weak_password", "wrong_password"],
            ),
            ("409", &["account_locked", "not_a_local_account"]),
            ("500", &["internal"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "change_state",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "check_directory",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
        ],
    ),
    (
        "check_identity_provider",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("502", &["provider_unreachable"]),
        ],
    ),
    (
        "cluster_members",
        &[
            ("500", &["invalid_place"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "cluster_settings",
        &[("503", &["cluster_admin_not_configured"])],
    ),
    (
        "cluster_upgrade",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "evacuation_blocked",
                    "no_upgrade",
                    "not_upgraded",
                    "upgrade_blocked",
                    "upgrade_in_progress",
                    "upgrade_step",
                ],
            ),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "complete_password_setup",
        &[
            (
                "400",
                &["invalid_request", "setup_invalid", "weak_password"],
            ),
            ("500", &["internal"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "confirm_fenced",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "member_not_offline",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_alert_channel",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["name_taken"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_alert_silence",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["draft_conflict"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_assignment",
        &[
            ("400", &["invalid_request"]),
            ("403", &["assignment_refused", "forbidden"]),
            ("404", &["not_found"]),
            ("409", &["user_disabled"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_directory",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "conflict",
                    "directory_exists",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_environment",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["incus_project_exists", "reserved_name"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_group",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_identity_provider",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "conflict",
                    "provider_exists",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_instance",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            ("409", &["incus_project_exists", "quota_exceeded"]),
            (
                "503",
                &[
                    "database_not_configured",
                    "incus_project_not_ready",
                    "quota_unverifiable",
                ],
            ),
        ],
    ),
    (
        "create_join_token",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["approval_invalid", "approval_required"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "create_local_account",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &["approval_invalid", "approval_required", "username_taken"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_my_token",
        &[
            ("400", &["invalid_request"]),
            ("403", &["people_only"]),
            ("409", &["too_many_tokens"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_organization",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_project",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_role",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "would_escalate"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_service_identity",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "create_token",
        &[
            ("400", &["invalid_request"]),
            (
                "403",
                &[
                    "approval_unavailable",
                    "forbidden",
                    "mfa_required",
                    "token_exceeds_caller",
                ],
            ),
            ("409", &["approval_invalid", "approval_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_alert_channel",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_application_volume",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &["approval_invalid", "approval_required", "volume_in_use"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_directory",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["approval_invalid", "approval_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_identity_provider",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &["approval_invalid", "approval_required", "would_lock_out"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_instance",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_policy",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_role",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_volume_backup",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["approval_invalid", "approval_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "delete_volume_snapshot",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "deploy_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "confirmation_required",
                    "incus_project_exists",
                    "not_published",
                    "operation_in_progress",
                    "plan_conflict",
                    "project_mismatch",
                    "quota_exceeded",
                    "service_range_exhausted",
                ],
            ),
            (
                "503",
                &[
                    "database_not_configured",
                    "incus_project_not_ready",
                    "quota_unverifiable",
                ],
            ),
        ],
    ),
    (
        "disable_service_identity",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "disable_user",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["last_super_admin", "self_lockout"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "effective_settings",
        &[("403", &["forbidden", "mfa_required"])],
    ),
    (
        "enable_service_identity",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "enable_user",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["last_super_admin", "self_lockout"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "end_alert_silence",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "end_my_sign_in",
        &[
            ("403", &["people_only"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "end_sign_in",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "environments",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "evacuate_member",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "evacuation_blocked",
                ],
            ),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "grant_emergency_access",
        &[
            ("400", &["invalid_request"]),
            ("403", &["assignment_refused"]),
            ("404", &["not_found"]),
            ("409", &["self_grant", "user_disabled"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "group_members",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("groups", &[("503", &["database_not_configured"])]),
    (
        "grow_application_volume",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "health_components",
        &[("403", &["forbidden", "mfa_required"])],
    ),
    ("health_servers", &[("403", &["forbidden", "mfa_required"])]),
    (
        "identity_audit",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "instance_console_log",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "instance_log",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "instance_logs",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "instance_page",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "job",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "job_detail",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "jobs",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "list_directories",
        &[("403", &["forbidden", "mfa_required"])],
    ),
    (
        "list_join_tokens",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["cluster_admin_not_configured"]),
        ],
    ),
    (
        "list_machines",
        &[(
            "503",
            &["cluster_admin_not_configured", "database_not_configured"],
        )],
    ),
    (
        "lookup_user",
        &[
            ("400", &["invalid_request"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "maas_commission",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("500", &["store_error"]),
            ("502", &["provisioner_unavailable"]),
            (
                "503",
                &["database_not_configured", "provisioner_not_configured"],
            ),
        ],
    ),
    (
        "maas_deploy",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("500", &["store_error"]),
            ("502", &["provisioner_unavailable"]),
            (
                "503",
                &["database_not_configured", "provisioner_not_configured"],
            ),
        ],
    ),
    (
        "maas_machines",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("502", &["provisioner_unavailable"]),
        ],
    ),
    (
        "maas_release",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("500", &["store_error"]),
            ("502", &["provisioner_unavailable"]),
            (
                "503",
                &["database_not_configured", "provisioner_not_configured"],
            ),
        ],
    ),
    (
        "maas_test",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["machine_in_use"]),
            ("500", &["store_error"]),
            ("502", &["provisioner_unavailable"]),
            (
                "503",
                &["database_not_configured", "provisioner_not_configured"],
            ),
        ],
    ),
    (
        "machine_admission",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("409", &["not_a_member"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    ("metrics", &[("403", &["forbidden", "mfa_required"])]),
    ("monitoring", &[("403", &["forbidden", "mfa_required"])]),
    (
        "move_instance",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            ("409", &["move_blocked"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "my_sign_ins",
        &[
            ("403", &["people_only"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "my_tokens",
        &[
            ("403", &["people_only"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "network_settings",
        &[("403", &["forbidden", "mfa_required"])],
    ),
    (
        "open_session",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "instance_state_unknown",
                    "session_unavailable",
                ],
            ),
            (
                "503",
                &[
                    "console_disabled",
                    "database_not_configured",
                    "no_randomness",
                ],
            ),
        ],
    ),
    (
        "operate_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "action_not_available",
                    "already_running",
                    "already_stopped",
                    "confirmation_required",
                    "member_unreachable",
                    "migration_blocked",
                    "not_deployed",
                    "nothing_deployed",
                    "nothing_to_do",
                    "operation_in_progress",
                ],
            ),
            ("503", &["database_not_configured", "incus_unreachable"]),
        ],
    ),
    ("organizations", &[("503", &["database_not_configured"])]),
    (
        "orphan_volumes",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "personal_tokens",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "plan_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("409", &["project_mismatch"]),
            ("503", &["quota_unverifiable"]),
        ],
    ),
    (
        "policies",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "prepare_incus_project",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["incus_project_exists"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "preview_application_migration",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            ("409", &["not_deployed"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "preview_assignment",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "preview_evacuation",
        &[("400", &["invalid_request"]), ("404", &["not_found"])],
    ),
    (
        "preview_instance_move",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "preview_member_removal",
        &[("400", &["invalid_request"]), ("404", &["not_found"])],
    ),
    (
        "preview_volume_restore",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("projects_list", &[("503", &["database_not_configured"])]),
    (
        "publish_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["no_draft", "not_found"]),
            ("409", &["already_published"]),
            ("422", &["draft_invalid"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "quota",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "register_machine",
        &[
            (
                "400",
                &[
                    "invalid_request",
                    "provisioner_not_configured",
                    "unknown_machine",
                ],
            ),
            ("403", &["forbidden", "mfa_required"]),
            ("502", &["provisioner_unavailable"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "reject_request",
        &[
            (
                "403",
                &["approval_needs_a_person", "forbidden", "mfa_required"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "remove_group_member",
        &[
            ("403", &["forbidden", "membership_refused", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["last_super_admin", "managed_by_provider"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "remove_machine",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &["approval_invalid", "approval_required", "still_a_member"],
            ),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "remove_member",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &["approval_invalid", "approval_required", "removal_blocked"],
            ),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "remove_sector",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["no_database"]),
        ],
    ),
    (
        "request_approval",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "reset_local_account",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &["approval_invalid", "approval_required", "self_reset"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "restore_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["still_running"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "restore_member",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "restore_volume_backup",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &["approval_invalid", "approval_required", "restore_blocked"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "restore_volume_snapshot",
        &[
            ("400", &["confirmation_mismatch", "invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "volume_in_use",
                    "volume_missing",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "retry_job",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "project_not_allowed"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "already_retried",
                    "not_failed",
                    "not_retryable",
                    "superseded",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "revoke_assignment",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required", "revocation_refused"]),
            ("404", &["not_found"]),
            ("409", &["last_super_admin"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "revoke_join_token",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
    (
        "revoke_my_token",
        &[
            ("403", &["people_only"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "revoke_personal_token",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("revoke_token", &[("503", &["database_not_configured"])]),
    ("roles", &[("503", &["database_not_configured"])]),
    (
        "rotate_my_token",
        &[
            ("400", &["invalid_request"]),
            ("403", &["people_only"]),
            ("409", &["already_rotated", "too_many_tokens"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "save_application_draft",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "save_policy",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "sectors",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("500", &["invalid_place"]),
        ],
    ),
    (
        "service_identities",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("sessions", &[("403", &["forbidden", "mfa_required"])]),
    (
        "set_api_tokens",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "locked_by_environment",
                    "would_lock_out",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "set_group_external",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "membership_refused", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "set_local_accounts",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "409",
                &["approval_invalid", "approval_required", "would_lock_out"],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "set_quota",
        &[
            ("400", &["invalid_request", "quota_above_parent"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "set_volume_schedules",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["backup_pool_missing"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "sign_in_settings",
        &[("403", &["forbidden", "mfa_required"])],
    ),
    (
        "sign_in_with_password",
        &[
            (
                "400",
                &[
                    "directory_unavailable",
                    "invalid_request",
                    "local_accounts_disabled",
                ],
            ),
            ("403", &["account_disabled"]),
            ("502", &["directory_entry_unreadable"]),
            ("503", &["database_not_configured", "directory_unreachable"]),
        ],
    ),
    (
        "sign_ins",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("sign_out", &[("503", &["database_not_configured"])]),
    (
        "storage_pool_usage",
        &[("403", &["forbidden", "mfa_required"])],
    ),
    (
        "storage_volumes",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "support_bundle",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "take_volume_snapshot",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["snapshots_not_allowed", "volume_missing"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "tenancy",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "terminate_session",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "test_alert_channel",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["channel_disabled"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("tokens", &[("503", &["database_not_configured"])]),
    (
        "update_alert_channel",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("409", &["name_taken"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "update_alert_rule",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "update_application",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "update_directory",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "conflict",
                    "directory_exists",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "update_identity_provider",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            (
                "409",
                &[
                    "approval_invalid",
                    "approval_required",
                    "conflict",
                    "provider_exists",
                    "would_lock_out",
                ],
            ),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "users",
        &[
            ("403", &["forbidden", "mfa_required"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "volume_backups",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    (
        "volume_snapshots",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            ("404", &["not_found"]),
            ("503", &["database_not_configured"]),
        ],
    ),
    ("whoami", &[("403", &["forbidden", "mfa_required"])]),
    (
        "withdraw_machine_approval",
        &[
            ("400", &["invalid_request"]),
            ("403", &["forbidden", "mfa_required"]),
            (
                "503",
                &["cluster_admin_not_configured", "database_not_configured"],
            ),
        ],
    ),
];

/// The codes `operation` can answer with `status`, if the table lists any.
pub(super) fn codes(operation: &str, status: &str) -> &'static [&'static str] {
    ERROR_CODES
        .iter()
        .find(|(op, _)| *op == operation)
        .and_then(|(_, by)| by.iter().find(|(s, _)| *s == status))
        .map_or(&[], |(_, c)| c)
}

#[cfg(test)]
mod tests {
    use utoipa::OpenApi;

    use super::*;

    fn sources(dir: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !path.to_string_lossy().ends_with("error_codes.rs")
            {
                out.push_str(&std::fs::read_to_string(&path).expect("read"));
            }
        }
    }

    #[test]
    fn every_listed_operation_exists_and_every_code_is_one_the_code_uses() {
        let api = super::super::ApiDoc::openapi();
        let operations: Vec<String> = api
            .paths
            .paths
            .values()
            .flat_map(|i| [&i.get, &i.put, &i.post, &i.delete, &i.patch])
            .flatten()
            .filter_map(|o| o.operation_id.clone())
            .collect();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut code = String::new();
        sources(&root.join("src"), &mut code);
        sources(&root.join("../../crates"), &mut code);
        for (op, by) in ERROR_CODES {
            assert!(
                operations.iter().any(|o| o == op),
                "{op} is not an operation"
            );
            for (status, codes) in *by {
                assert!(
                    status.len() == 3 && !matches!(*status, "401" | "429"),
                    "{op}: {status}"
                );
                for c in *codes {
                    assert!(
                        code.contains(&format!("\"{c}\"")),
                        "{op}: {c} is used nowhere"
                    );
                }
            }
        }
        // Documented: the status and its codes.
        let ack = api
            .paths
            .paths
            .values()
            .filter_map(|i| i.post.as_ref())
            .find(|o| o.operation_id.as_deref() == Some("acknowledge_alert"))
            .expect("acknowledge_alert");
        let utoipa::openapi::RefOr::T(r) = &ack.responses.responses["409"] else {
            panic!("409")
        };
        assert!(
            r.description
                .ends_with("`error.code`: `already_acknowledged`, `not_firing`."),
            "{}",
            r.description
        );
    }
}
