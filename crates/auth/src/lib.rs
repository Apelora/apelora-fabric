// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora authorisation: permissions, roles, scopes and the decision engine. No I/O.
//!
//! The control plane loads a caller's assignments from PostgreSQL, builds an [`Engine`] and asks
//! it. Keeping the logic here, pure and exhaustively tested, means no route or screen has its
//! own idea of who may do what.

pub mod approval;
pub mod assign;
pub mod backend;
pub mod evaluate;
pub mod permission;
pub mod role;
pub mod scope;
pub mod service;

pub use approval::{Approval, ApprovalError, ApprovalState};
pub use assign::{AssignError, AssignRequest, check_assignment};
pub use evaluate::{
    AccessFilter, Assignment, AssignmentKind, Conditions, Decision, Denial, Engine, Grant,
    Policies, PrincipalRef, Subject, confers,
};
pub use permission::{Class, PERMISSIONS, PermissionDef, needs_approval, permission};
pub use role::{
    Builtin, CustomRole, ResolvedRole, RoleError, RoleRef, custom_scopes, validate_custom,
};
pub use scope::{EnvKind, Level, Resource, Scope, covers};
