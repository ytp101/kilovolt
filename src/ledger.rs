use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AccountSnapshot {
    pub committed_spend: f64,
    pub reserved_spend: f64,
    pub total_spend: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetSnapshot {
    pub project: AccountSnapshot,
    pub user: AccountSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetScope {
    Project,
    User,
}

impl fmt::Display for BudgetScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project => formatter.write_str("project"),
            Self::User => formatter.write_str("user"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    BudgetExceeded(BudgetScope),
    InvalidAmount,
    InvalidBudgetLimit(BudgetScope),
    ArithmeticOverflow,
    ReservationAlreadyExists,
    ReservationNotFound,
    ReservationAlreadyFinalized,
    ReservationUserMismatch,
}

impl fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetExceeded(scope) => write!(formatter, "{scope} budget would be exceeded"),
            Self::InvalidAmount => {
                formatter.write_str("budget amount must be finite and non-negative")
            }
            Self::InvalidBudgetLimit(scope) => {
                write!(
                    formatter,
                    "{scope} budget limit must be finite and non-negative"
                )
            }
            Self::ArithmeticOverflow => {
                formatter.write_str("budget arithmetic produced a non-finite value")
            }
            Self::ReservationAlreadyExists => {
                formatter.write_str("prompt reservation already exists")
            }
            Self::ReservationNotFound => formatter.write_str("prompt reservation was not found"),
            Self::ReservationAlreadyFinalized => {
                formatter.write_str("prompt reservation was already finalized differently")
            }
            Self::ReservationUserMismatch => {
                formatter.write_str("prompt reservation belongs to another user")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finalization {
    Committed,
    Released,
}

#[derive(Debug, Default)]
struct AccountBudget {
    committed_spend: f64,
    reserved_spend: f64,
}

impl AccountBudget {
    fn snapshot(&self) -> AccountSnapshot {
        AccountSnapshot {
            committed_spend: self.committed_spend,
            reserved_spend: self.reserved_spend,
            total_spend: self.committed_spend + self.reserved_spend,
        }
    }
}

#[derive(Debug)]
struct PromptReservation {
    user_id: String,
    amount: f64,
}

#[derive(Debug)]
struct FinalizedReservation {
    user_id: String,
    finalization: Finalization,
}

#[derive(Debug, Default)]
struct LedgerState {
    project: AccountBudget,
    users: HashMap<String, AccountBudget>,
    reservations: HashMap<String, PromptReservation>,
    finalized_reservations: HashMap<String, FinalizedReservation>,
}

impl LedgerState {
    fn snapshot_for_user(&self, user_id: &str) -> BudgetSnapshot {
        BudgetSnapshot {
            project: self.project.snapshot(),
            user: self
                .users
                .get(user_id)
                .map(AccountBudget::snapshot)
                .unwrap_or_default(),
        }
    }
}

/// Atomically tracks project-wide and per-user spend and prompt reservations.
///
/// All financial comparisons and additions live here so callers cannot split a
/// budget check from its corresponding mutation. Both hierarchy levels share
/// one mutex, so a failed project or user check leaves the entire ledger
/// unchanged. Public amounts remain `f64` for compatibility; a future
/// pricing-ledger refactor should use integer monetary units to avoid binary
/// floating-point representation concerns.
#[derive(Debug, Default)]
pub struct BudgetLedger {
    inner: Mutex<LedgerState>,
}

impl BudgetLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve_prompt(
        &self,
        request_id: &str,
        user_id: &str,
        amount: f64,
        project_budget_limit: f64,
        user_budget_limit: f64,
    ) -> Result<BudgetSnapshot, BudgetError> {
        validate_amount(amount)?;
        validate_budget_limit(project_budget_limit, BudgetScope::Project)?;
        validate_budget_limit(user_budget_limit, BudgetScope::User)?;

        let mut state = self.lock();
        if state.reservations.contains_key(request_id) {
            return Err(BudgetError::ReservationAlreadyExists);
        }
        if state.finalized_reservations.contains_key(request_id) {
            return Err(BudgetError::ReservationAlreadyFinalized);
        }

        let new_project_reserved = checked_add(state.project.reserved_spend, amount)?;
        let new_project_total = checked_add(state.project.committed_spend, new_project_reserved)?;
        if new_project_total > project_budget_limit {
            return Err(BudgetError::BudgetExceeded(BudgetScope::Project));
        }

        let user = state.users.get(user_id);
        let current_user_committed = user.map_or(0.0, |account| account.committed_spend);
        let current_user_reserved = user.map_or(0.0, |account| account.reserved_spend);
        let new_user_reserved = checked_add(current_user_reserved, amount)?;
        let new_user_total = checked_add(current_user_committed, new_user_reserved)?;
        if new_user_total > user_budget_limit {
            return Err(BudgetError::BudgetExceeded(BudgetScope::User));
        }

        state.project.reserved_spend = new_project_reserved;
        state
            .users
            .entry(user_id.to_string())
            .or_default()
            .reserved_spend = new_user_reserved;
        state.reservations.insert(
            request_id.to_string(),
            PromptReservation {
                user_id: user_id.to_string(),
                amount,
            },
        );
        Ok(state.snapshot_for_user(user_id))
    }

    pub fn commit_prompt(
        &self,
        request_id: &str,
        user_id: &str,
    ) -> Result<BudgetSnapshot, BudgetError> {
        let mut state = self.lock();
        if let Some(finalized) = state.finalized_reservations.get(request_id) {
            if finalized.user_id != user_id {
                return Err(BudgetError::ReservationUserMismatch);
            }
            return match finalized.finalization {
                Finalization::Committed => Ok(state.snapshot_for_user(user_id)),
                Finalization::Released => Err(BudgetError::ReservationAlreadyFinalized),
            };
        }

        let reservation = state
            .reservations
            .get(request_id)
            .ok_or(BudgetError::ReservationNotFound)?;
        if reservation.user_id != user_id {
            return Err(BudgetError::ReservationUserMismatch);
        }
        let amount = reservation.amount;
        let new_project_committed = checked_add(state.project.committed_spend, amount)?;
        let user_committed = state
            .users
            .get(user_id)
            .ok_or(BudgetError::ReservationNotFound)?
            .committed_spend;
        let new_user_committed = checked_add(user_committed, amount)?;
        let last_project_reservation = state.reservations.len() == 1;
        let last_user_reservation = !state
            .reservations
            .iter()
            .any(|(id, active)| id != request_id && active.user_id == user_id);

        state.reservations.remove(request_id);
        state.project.reserved_spend -= amount;
        if last_project_reservation {
            state.project.reserved_spend = 0.0;
        }
        state.project.committed_spend = new_project_committed;
        let user = state
            .users
            .get_mut(user_id)
            .ok_or(BudgetError::ReservationNotFound)?;
        user.reserved_spend -= amount;
        if last_user_reservation {
            user.reserved_spend = 0.0;
        }
        user.committed_spend = new_user_committed;
        state.finalized_reservations.insert(
            request_id.to_string(),
            FinalizedReservation {
                user_id: user_id.to_string(),
                finalization: Finalization::Committed,
            },
        );
        Ok(state.snapshot_for_user(user_id))
    }

    pub fn release_prompt(
        &self,
        request_id: &str,
        user_id: &str,
    ) -> Result<BudgetSnapshot, BudgetError> {
        let mut state = self.lock();
        if let Some(finalized) = state.finalized_reservations.get(request_id) {
            if finalized.user_id != user_id {
                return Err(BudgetError::ReservationUserMismatch);
            }
            return match finalized.finalization {
                Finalization::Released => Ok(state.snapshot_for_user(user_id)),
                Finalization::Committed => Err(BudgetError::ReservationAlreadyFinalized),
            };
        }

        let reservation = state
            .reservations
            .get(request_id)
            .ok_or(BudgetError::ReservationNotFound)?;
        if reservation.user_id != user_id {
            return Err(BudgetError::ReservationUserMismatch);
        }
        let amount = reservation.amount;
        let last_project_reservation = state.reservations.len() == 1;
        let last_user_reservation = !state
            .reservations
            .iter()
            .any(|(id, active)| id != request_id && active.user_id == user_id);

        state.reservations.remove(request_id);
        state.project.reserved_spend -= amount;
        if last_project_reservation {
            state.project.reserved_spend = 0.0;
        }
        let user = state
            .users
            .get_mut(user_id)
            .ok_or(BudgetError::ReservationNotFound)?;
        user.reserved_spend -= amount;
        if last_user_reservation {
            user.reserved_spend = 0.0;
        }
        state.finalized_reservations.insert(
            request_id.to_string(),
            FinalizedReservation {
                user_id: user_id.to_string(),
                finalization: Finalization::Released,
            },
        );
        Ok(state.snapshot_for_user(user_id))
    }

    pub fn try_charge_output(
        &self,
        user_id: &str,
        amount: f64,
        project_budget_limit: f64,
        user_budget_limit: f64,
    ) -> Result<BudgetSnapshot, BudgetError> {
        validate_amount(amount)?;
        validate_budget_limit(project_budget_limit, BudgetScope::Project)?;
        validate_budget_limit(user_budget_limit, BudgetScope::User)?;

        let mut state = self.lock();
        let new_project_committed = checked_add(state.project.committed_spend, amount)?;
        let new_project_total = checked_add(new_project_committed, state.project.reserved_spend)?;
        if new_project_total > project_budget_limit {
            return Err(BudgetError::BudgetExceeded(BudgetScope::Project));
        }

        let user = state.users.get(user_id);
        let current_user_committed = user.map_or(0.0, |account| account.committed_spend);
        let current_user_reserved = user.map_or(0.0, |account| account.reserved_spend);
        let new_user_committed = checked_add(current_user_committed, amount)?;
        let new_user_total = checked_add(new_user_committed, current_user_reserved)?;
        if new_user_total > user_budget_limit {
            return Err(BudgetError::BudgetExceeded(BudgetScope::User));
        }

        state.project.committed_spend = new_project_committed;
        state
            .users
            .entry(user_id.to_string())
            .or_default()
            .committed_spend = new_user_committed;
        Ok(state.snapshot_for_user(user_id))
    }

    /// Returns committed spend only, preserving the dashboard's existing shape.
    pub fn snapshot(&self) -> HashMap<String, f64> {
        self.lock()
            .users
            .iter()
            .map(|(user_id, budget)| (user_id.clone(), budget.committed_spend))
            .collect()
    }

    pub fn project_snapshot(&self) -> AccountSnapshot {
        self.lock().project.snapshot()
    }

    pub fn user_snapshot(&self, user_id: &str) -> AccountSnapshot {
        self.lock()
            .users
            .get(user_id)
            .map(AccountBudget::snapshot)
            .unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LedgerState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn validate_amount(amount: f64) -> Result<(), BudgetError> {
    if amount.is_finite() && amount >= 0.0 {
        Ok(())
    } else {
        Err(BudgetError::InvalidAmount)
    }
}

fn validate_budget_limit(budget_limit: f64, scope: BudgetScope) -> Result<(), BudgetError> {
    if budget_limit.is_finite() && budget_limit >= 0.0 {
        Ok(())
    } else {
        Err(BudgetError::InvalidBudgetLimit(scope))
    }
}

fn checked_add(left: f64, right: f64) -> Result<f64, BudgetError> {
    let result = left + right;
    if result.is_finite() {
        Ok(result)
    } else {
        Err(BudgetError::ArithmeticOverflow)
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountSnapshot, BudgetError, BudgetLedger, BudgetScope};
    use std::sync::{Arc, Barrier};

    fn assert_account(snapshot: AccountSnapshot, committed: f64, reserved: f64, total: f64) {
        assert_eq!(snapshot.committed_spend, committed);
        assert_eq!(snapshot.reserved_spend, reserved);
        assert_eq!(snapshot.total_spend, total);
    }

    #[test]
    fn reservation_below_both_budgets_succeeds() {
        let ledger = BudgetLedger::new();

        let snapshot = ledger
            .reserve_prompt("request-1", "user", 0.25, 10.0, 1.0)
            .expect("reservation should succeed");

        assert_account(snapshot.project, 0.0, 0.25, 0.25);
        assert_account(snapshot.user, 0.0, 0.25, 0.25);
    }

    #[test]
    fn reservation_exceeding_user_budget_leaves_both_ledgers_unchanged() {
        let ledger = BudgetLedger::new();

        let result = ledger.reserve_prompt("request-1", "user", 1.25, 10.0, 1.0);

        assert_eq!(result, Err(BudgetError::BudgetExceeded(BudgetScope::User)));
        assert!(ledger.snapshot().is_empty());
        assert_account(ledger.project_snapshot(), 0.0, 0.0, 0.0);
        assert_account(ledger.user_snapshot("user"), 0.0, 0.0, 0.0);
    }

    #[test]
    fn reservation_exceeding_project_budget_leaves_both_ledgers_unchanged() {
        let ledger = BudgetLedger::new();

        let result = ledger.reserve_prompt("request-1", "user", 1.25, 1.0, 10.0);

        assert_eq!(
            result,
            Err(BudgetError::BudgetExceeded(BudgetScope::Project))
        );
        assert!(ledger.snapshot().is_empty());
        assert_account(ledger.project_snapshot(), 0.0, 0.0, 0.0);
        assert_account(ledger.user_snapshot("user"), 0.0, 0.0, 0.0);
    }

    #[test]
    fn reservation_exactly_at_both_limits_succeeds() {
        let ledger = BudgetLedger::new();

        let snapshot = ledger
            .reserve_prompt("request-1", "user", 1.0, 1.0, 1.0)
            .expect("reservation at the limit should succeed");

        assert_eq!(snapshot.project.total_spend, 1.0);
        assert_eq!(snapshot.user.total_spend, 1.0);
    }

    #[test]
    fn committing_moves_project_and_user_reservations_to_committed() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("request-1", "user", 0.25, 10.0, 1.0)
            .expect("reservation should succeed");

        let snapshot = ledger
            .commit_prompt("request-1", "user")
            .expect("commit should succeed");

        assert_account(snapshot.project, 0.25, 0.0, 0.25);
        assert_account(snapshot.user, 0.25, 0.0, 0.25);
    }

    #[test]
    fn releasing_removes_project_and_user_reservations_without_committing() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("request-1", "user", 0.25, 10.0, 1.0)
            .expect("reservation should succeed");

        let snapshot = ledger
            .release_prompt("request-1", "user")
            .expect("release should succeed");

        assert_account(snapshot.project, 0.0, 0.0, 0.0);
        assert_account(snapshot.user, 0.0, 0.0, 0.0);
    }

    #[test]
    fn repeated_commit_and_release_are_idempotent_at_both_levels() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("commit-request", "user", 0.25, 10.0, 1.0)
            .expect("reservation should succeed");
        let committed = ledger
            .commit_prompt("commit-request", "user")
            .expect("first commit should succeed");
        let committed_again = ledger
            .commit_prompt("commit-request", "user")
            .expect("repeated commit should be idempotent");
        assert_eq!(committed_again, committed);
        assert_eq!(
            ledger.release_prompt("commit-request", "user"),
            Err(BudgetError::ReservationAlreadyFinalized)
        );

        ledger
            .reserve_prompt("release-request", "user", 0.25, 10.0, 1.0)
            .expect("reservation should succeed");
        let released = ledger
            .release_prompt("release-request", "user")
            .expect("first release should succeed");
        let released_again = ledger
            .release_prompt("release-request", "user")
            .expect("repeated release should be idempotent");
        assert_eq!(released_again, released);
        assert_eq!(
            ledger.commit_prompt("release-request", "user"),
            Err(BudgetError::ReservationAlreadyFinalized)
        );
        assert_eq!(ledger.user_snapshot("user").committed_spend, 0.25);
        assert_eq!(ledger.project_snapshot().committed_spend, 0.25);
    }

    #[test]
    fn output_charge_below_both_remaining_budgets_succeeds() {
        let ledger = BudgetLedger::new();

        let snapshot = ledger
            .try_charge_output("user", 0.25, 10.0, 1.0)
            .expect("output charge should succeed");

        assert_account(snapshot.project, 0.25, 0.0, 0.25);
        assert_account(snapshot.user, 0.25, 0.0, 0.25);
    }

    #[test]
    fn output_charge_exceeding_user_budget_leaves_both_ledgers_unchanged() {
        let ledger = BudgetLedger::new();
        ledger
            .try_charge_output("user", 0.75, 10.0, 1.0)
            .expect("initial output charge should succeed");
        let project_before = ledger.project_snapshot();
        let user_before = ledger.user_snapshot("user");

        let result = ledger.try_charge_output("user", 0.5, 10.0, 1.0);

        assert_eq!(result, Err(BudgetError::BudgetExceeded(BudgetScope::User)));
        assert_eq!(ledger.project_snapshot(), project_before);
        assert_eq!(ledger.user_snapshot("user"), user_before);
    }

    #[test]
    fn output_charge_exceeding_project_budget_leaves_both_ledgers_unchanged() {
        let ledger = BudgetLedger::new();
        ledger
            .try_charge_output("user-a", 0.75, 1.0, 1.0)
            .expect("initial output charge should succeed");
        let project_before = ledger.project_snapshot();
        let user_a_before = ledger.user_snapshot("user-a");

        let result = ledger.try_charge_output("user-b", 0.5, 1.0, 1.0);

        assert_eq!(
            result,
            Err(BudgetError::BudgetExceeded(BudgetScope::Project))
        );
        assert_eq!(ledger.project_snapshot(), project_before);
        assert_eq!(ledger.user_snapshot("user-a"), user_a_before);
        assert_account(ledger.user_snapshot("user-b"), 0.0, 0.0, 0.0);
    }

    #[test]
    fn active_user_reservation_reduces_available_user_budget() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("request-1", "user", 0.75, 10.0, 1.0)
            .expect("reservation should succeed");

        let result = ledger.try_charge_output("user", 0.5, 10.0, 1.0);

        assert_eq!(result, Err(BudgetError::BudgetExceeded(BudgetScope::User)));
        assert_account(ledger.project_snapshot(), 0.0, 0.75, 0.75);
        assert_account(ledger.user_snapshot("user"), 0.0, 0.75, 0.75);
    }

    #[test]
    fn active_project_reservation_reduces_budget_for_other_users() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("request-1", "user-a", 0.75, 1.0, 1.0)
            .expect("reservation should succeed");

        let result = ledger.try_charge_output("user-b", 0.5, 1.0, 1.0);

        assert_eq!(
            result,
            Err(BudgetError::BudgetExceeded(BudgetScope::Project))
        );
        assert_account(ledger.project_snapshot(), 0.0, 0.75, 0.75);
        assert_account(ledger.user_snapshot("user-a"), 0.0, 0.75, 0.75);
        assert_account(ledger.user_snapshot("user-b"), 0.0, 0.0, 0.0);
    }

    #[test]
    fn mixed_limit_failures_are_atomic_and_project_is_checked_first() {
        let ledger = BudgetLedger::new();
        ledger
            .try_charge_output("project-consumer", 0.75, 1.0, 10.0)
            .expect("initial project charge should succeed");
        ledger
            .try_charge_output("user-consumer", 0.75, 10.0, 1.0)
            .expect("initial user charge should succeed");

        let project_before = ledger.project_snapshot();
        let project_user_before = ledger.user_snapshot("project-consumer");
        let user_before = ledger.user_snapshot("user-consumer");

        assert_eq!(
            ledger.reserve_prompt("user-block", "user-consumer", 0.5, 10.0, 1.0),
            Err(BudgetError::BudgetExceeded(BudgetScope::User))
        );
        assert_eq!(
            ledger.reserve_prompt("project-block", "new-user", 0.5, 1.5, 10.0),
            Err(BudgetError::BudgetExceeded(BudgetScope::Project))
        );
        assert_eq!(
            ledger.reserve_prompt("both-block", "user-consumer", 1.0, 1.5, 1.0),
            Err(BudgetError::BudgetExceeded(BudgetScope::Project))
        );

        assert_eq!(ledger.project_snapshot(), project_before);
        assert_eq!(
            ledger.user_snapshot("project-consumer"),
            project_user_before
        );
        assert_eq!(ledger.user_snapshot("user-consumer"), user_before);
        assert_account(ledger.user_snapshot("new-user"), 0.0, 0.0, 0.0);
    }

    #[test]
    fn exact_and_near_limit_floating_point_behavior_is_deterministic() {
        let ledger = BudgetLedger::new();
        ledger
            .try_charge_output("user", 0.1, 1.0, 0.3)
            .expect("first charge should succeed");
        let before_binary_rounding_edge = ledger.user_snapshot("user");
        assert_eq!(
            ledger.try_charge_output("user", 0.2, 1.0, 0.3),
            Err(BudgetError::BudgetExceeded(BudgetScope::User))
        );
        assert_eq!(ledger.user_snapshot("user"), before_binary_rounding_edge);

        let exact_ledger = BudgetLedger::new();
        exact_ledger
            .try_charge_output("user", 0.125, 1.0, 0.5)
            .expect("binary-exact charge should succeed");
        exact_ledger
            .try_charge_output("user", 0.375, 1.0, 0.5)
            .expect("binary-exact limit should succeed");
        let at_limit = exact_ledger.user_snapshot("user");
        assert_eq!(
            exact_ledger.try_charge_output("user", f64::EPSILON, 1.0, 0.5),
            Err(BudgetError::BudgetExceeded(BudgetScope::User))
        );
        assert_eq!(exact_ledger.user_snapshot("user"), at_limit);
    }

    #[test]
    fn invalid_ids_and_amounts_return_typed_errors_without_mutation() {
        let ledger = BudgetLedger::new();
        assert_eq!(
            ledger.commit_prompt("unknown", "user"),
            Err(BudgetError::ReservationNotFound)
        );
        assert_eq!(
            ledger.release_prompt("unknown", "user"),
            Err(BudgetError::ReservationNotFound)
        );
        assert_eq!(
            ledger.reserve_prompt("invalid", "user", f64::NAN, 1.0, 1.0),
            Err(BudgetError::InvalidAmount)
        );
        assert_eq!(
            ledger.try_charge_output("user", -0.1, 1.0, 1.0),
            Err(BudgetError::InvalidAmount)
        );
        assert_account(ledger.project_snapshot(), 0.0, 0.0, 0.0);
    }

    #[test]
    fn duplicate_request_and_user_mismatch_are_typed_errors() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("request", "owner", 0.25, 1.0, 1.0)
            .expect("first reservation should succeed");
        assert_eq!(
            ledger.reserve_prompt("request", "owner", 0.25, 1.0, 1.0),
            Err(BudgetError::ReservationAlreadyExists)
        );
        assert_eq!(
            ledger.commit_prompt("request", "other"),
            Err(BudgetError::ReservationUserMismatch)
        );
        assert_eq!(
            ledger.release_prompt("request", "other"),
            Err(BudgetError::ReservationUserMismatch)
        );
        assert_account(ledger.user_snapshot("owner"), 0.0, 0.25, 0.25);
    }

    #[test]
    fn output_charge_before_prompt_commit_counts_the_active_reservation() {
        let ledger = BudgetLedger::new();
        ledger
            .reserve_prompt("request", "user", 0.25, 1.0, 1.0)
            .expect("reservation should succeed");
        let charged = ledger
            .try_charge_output("user", 0.5, 1.0, 1.0)
            .expect("output plus active prompt reservation remains within budget");
        assert_account(charged.user, 0.5, 0.25, 0.75);

        let committed = ledger
            .commit_prompt("request", "user")
            .expect("prompt should still commit exactly once");
        assert_account(committed.user, 0.75, 0.0, 0.75);
    }

    #[test]
    fn concurrent_same_user_reservations_never_exceed_user_budget() {
        const ATTEMPTS: usize = 100;
        let ledger = Arc::new(BudgetLedger::new());
        let barrier = Arc::new(Barrier::new(ATTEMPTS));

        let handles: Vec<_> = (0..ATTEMPTS)
            .map(|index| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger
                        .reserve_prompt(&format!("request-{index}"), "user", 0.1, 10.0, 1.0)
                        .map(|snapshot| {
                            assert!(snapshot.project.total_spend <= 10.0);
                            assert!(snapshot.user.total_spend <= 1.0);
                        })
                })
            })
            .collect();

        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("reservation thread should not panic"))
            .filter(Result::is_ok)
            .count();

        assert_eq!(successes, 10);
        let snapshot = ledger.user_snapshot("user");
        assert!(snapshot.total_spend <= 1.0);
        assert!((snapshot.reserved_spend - 1.0).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn concurrent_distinct_users_never_exceed_project_budget() {
        const ATTEMPTS: usize = 100;
        let ledger = Arc::new(BudgetLedger::new());
        let barrier = Arc::new(Barrier::new(ATTEMPTS));

        let handles: Vec<_> = (0..ATTEMPTS)
            .map(|index| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger.reserve_prompt(
                        &format!("request-{index}"),
                        &format!("user-{index}"),
                        0.1,
                        1.0,
                        1.0,
                    )
                })
            })
            .collect();

        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("reservation thread should not panic"))
            .filter(Result::is_ok)
            .count();

        assert_eq!(successes, 10);
        let project = ledger.project_snapshot();
        assert!(project.total_spend <= 1.0);
        assert!((project.reserved_spend - 1.0).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn concurrent_output_charges_never_exceed_either_budget() {
        const ATTEMPTS: usize = 100;
        let ledger = Arc::new(BudgetLedger::new());
        let barrier = Arc::new(Barrier::new(ATTEMPTS));
        let handles: Vec<_> = (0..ATTEMPTS)
            .map(|_| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger.try_charge_output("user", 0.1, 1.0, 1.0)
                })
            })
            .collect();

        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("charge thread should not panic"))
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, 10);
        assert!(ledger.project_snapshot().total_spend <= 1.0);
        assert!(ledger.user_snapshot("user").total_spend <= 1.0);
    }

    #[test]
    fn concurrent_commit_and_release_finalize_each_reservation_once() {
        const RESERVATIONS: usize = 10;
        let ledger = Arc::new(BudgetLedger::new());
        for index in 0..RESERVATIONS {
            ledger
                .reserve_prompt(&format!("request-{index}"), "user", 0.1, 1.0, 1.0)
                .expect("setup reservation should succeed");
        }
        let barrier = Arc::new(Barrier::new(RESERVATIONS));
        let handles: Vec<_> = (0..RESERVATIONS)
            .map(|index| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    if index % 2 == 0 {
                        ledger.commit_prompt(&format!("request-{index}"), "user")
                    } else {
                        ledger.release_prompt(&format!("request-{index}"), "user")
                    }
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .expect("finalization thread should not panic")
                .expect("each reservation should finalize");
        }

        let project = ledger.project_snapshot();
        let user = ledger.user_snapshot("user");
        assert_eq!(project.reserved_spend, 0.0);
        assert_eq!(user.reserved_spend, 0.0);
        assert!((project.committed_spend - 0.5).abs() < f64::EPSILON * 10.0);
        assert_eq!(project, user);
    }

    #[test]
    fn concurrent_release_and_reserve_leave_a_consistent_user_ledger() {
        let ledger = Arc::new(BudgetLedger::new());
        ledger
            .reserve_prompt("old", "user", 0.9, 10.0, 1.0)
            .expect("setup reservation should succeed");
        let barrier = Arc::new(Barrier::new(2));

        let release = {
            let ledger = Arc::clone(&ledger);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                ledger.release_prompt("old", "user")
            })
        };
        let reserve = {
            let ledger = Arc::clone(&ledger);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                ledger.reserve_prompt("new", "user", 0.2, 10.0, 1.0)
            })
        };

        release
            .join()
            .expect("release thread should not panic")
            .expect("release should succeed");
        let reserve_result = reserve.join().expect("reserve thread should not panic");
        let snapshot = ledger.user_snapshot("user");
        assert_eq!(snapshot.committed_spend, 0.0);
        assert_eq!(
            snapshot.reserved_spend,
            if reserve_result.is_ok() { 0.2 } else { 0.0 }
        );
        assert!(snapshot.total_spend <= 1.0);
    }
}
