#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod maintenance_tests;

use std::time::Duration;

use app::agent_tab::RecoveryReadiness;
use nmt_agent::session::lifecycle::{RecoverySnapshot, RestorationReadiness};
use nmt_agent::update::{UpdateError, UpdateErrorKind, UpdatePhase, UpdateProgress, VersionStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateMode {
    WhenIdle,
    StopNow,
}

impl UpdateMode {
    pub(super) fn interrupts_active_work(self) -> bool {
        self == Self::StopNow
    }
}

/// Execution operations for one fixed group of sessions. Indices keep their
/// meaning until recovery finishes, including when a view closes meanwhile.
pub(super) trait RecoveryEnvironment {
    fn identity_failure(&mut self) -> Option<String>;

    fn prepare(&mut self, mode: UpdateMode);

    fn readiness(&mut self) -> Vec<RecoveryReadiness>;

    fn cancel_wait(&mut self);

    async fn suspend(&mut self, mode: UpdateMode) -> Vec<Result<(), String>>;

    fn restore(&mut self, snapshots: &[RecoverySnapshot], suspended: &[usize]);

    fn restoration_readiness(&mut self, suspended: &[usize]) -> Vec<RestorationReadiness>;

    fn recovery_timed_out(&mut self, pending: &[usize]);

    fn publish(&mut self, phase: UpdatePhase, progress: Option<UpdateProgress>);

    fn now(&self) -> Duration;

    async fn wait(&mut self, duration: Duration);
}

/// The vendor update that runs between suspending and restoring sessions.
pub(super) trait UpdateEnvironment: RecoveryEnvironment {
    async fn update(&mut self) -> Result<(), UpdateError>;

    async fn verify(&mut self) -> Result<VersionStatus, UpdateError>;
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PreflightFailure {
    MissingIdentity(String),
    InterruptionTimeout,
}

pub(super) enum PreflightResolution {
    Ready(Vec<RecoverySnapshot>),
    Wait,
    Failed(PreflightFailure),
}

pub(super) fn resolve_preflight(
    assessments: Vec<RecoveryReadiness>,
    mode: UpdateMode,
    stop_timeout_elapsed: bool,
) -> PreflightResolution {
    let mut snapshots = Vec::with_capacity(assessments.len());
    let mut busy = false;

    for assessment in assessments {
        match assessment {
            RecoveryReadiness::Ready(snapshot) => snapshots.push(snapshot),
            RecoveryReadiness::Busy(_) => busy = true,
            RecoveryReadiness::MissingIdentity(message) => {
                return PreflightResolution::Failed(PreflightFailure::MissingIdentity(message));
            }
        }
    }

    if !busy {
        PreflightResolution::Ready(snapshots)
    } else if mode.interrupts_active_work() && stop_timeout_elapsed {
        PreflightResolution::Failed(PreflightFailure::InterruptionTimeout)
    } else {
        PreflightResolution::Wait
    }
}

/// The sessions one cycle stopped. `error` keeps the first suspension failure;
/// the sessions that did stop are still listed so they come back.
struct Suspension {
    snapshots: Vec<RecoverySnapshot>,
    suspended: Vec<usize>,
    error: Option<String>,
}

async fn suspend_sessions(
    environment: &mut impl RecoveryEnvironment,
    mode: UpdateMode,
) -> Result<Suspension, PreflightFailure> {
    if mode.interrupts_active_work()
        && let Some(message) = environment.identity_failure()
    {
        return Err(PreflightFailure::MissingIdentity(message));
    }

    environment.prepare(mode);

    let started = environment.now();

    let snapshots = loop {
        let readiness = environment.readiness();

        match resolve_preflight(
            readiness,
            mode,
            environment.now() - started >= Duration::from_secs(15),
        ) {
            PreflightResolution::Ready(snapshots) => break snapshots,
            PreflightResolution::Failed(error) => {
                environment.cancel_wait();

                return Err(error);
            }
            PreflightResolution::Wait => environment.wait(Duration::from_millis(100)).await,
        }
    };

    environment.publish(
        UpdatePhase::Suspending,
        Some(UpdateProgress {
            completed: 0,
            total: snapshots.len(),
        }),
    );

    let results = environment.suspend(mode).await;

    let mut suspended = Vec::new();
    let mut error = None;

    for (index, result) in results.into_iter().enumerate() {
        match result {
            Ok(()) => suspended.push(index),
            Err(message) => {
                error.get_or_insert(message);
            }
        }
    }

    Ok(Suspension {
        snapshots,
        suspended,
        error,
    })
}

#[derive(Default)]
pub(super) struct TransactionOutcome {
    pub(super) verified: Option<VersionStatus>,
    pub(super) operation_error: Option<UpdateError>,
    pub(super) restore_failures: usize,
}

/// Every successfully suspended session is restored even when suspension of
/// another session, the update, or verification fails.
pub(super) async fn run_transaction(
    environment: &mut impl UpdateEnvironment,
    mode: UpdateMode,
) -> Result<TransactionOutcome, PreflightFailure> {
    let suspension = suspend_sessions(environment, mode).await?;

    let mut outcome = TransactionOutcome {
        operation_error: suspension
            .error
            .map(|message| UpdateError::new(UpdateErrorKind::Recovery, message)),
        ..TransactionOutcome::default()
    };

    if outcome.operation_error.is_none() {
        environment.publish(UpdatePhase::Updating, None);

        match environment.update().await {
            Ok(()) => {
                environment.publish(UpdatePhase::Verifying, None);

                match environment.verify().await {
                    Ok(status) => outcome.verified = Some(status),
                    Err(error) => outcome.operation_error = Some(error),
                }
            }
            Err(error) => outcome.operation_error = Some(error),
        }
    }

    outcome.restore_failures =
        restore_sessions(environment, &suspension.snapshots, &suspension.suspended).await;

    Ok(outcome)
}

pub(super) struct RestartOutcome {
    pub(super) restarted: usize,
    pub(super) suspend_error: Option<String>,
    pub(super) restore_failures: usize,
}

/// Stops every session and brings each one back on its saved conversation,
/// with no provider work in between. A session that failed to stop keeps its
/// backend, so only the stopped ones are restored.
pub(super) async fn run_restart(
    environment: &mut impl RecoveryEnvironment,
    mode: UpdateMode,
) -> Result<RestartOutcome, PreflightFailure> {
    let suspension = suspend_sessions(environment, mode).await?;

    let restore_failures =
        restore_sessions(environment, &suspension.snapshots, &suspension.suspended).await;

    Ok(RestartOutcome {
        restarted: suspension.suspended.len() - restore_failures.min(suspension.suspended.len()),
        suspend_error: suspension.error,
        restore_failures,
    })
}

async fn restore_sessions(
    environment: &mut impl RecoveryEnvironment,
    snapshots: &[RecoverySnapshot],
    suspended: &[usize],
) -> usize {
    environment.publish(
        UpdatePhase::Restoring,
        Some(UpdateProgress {
            completed: 0,
            total: suspended.len(),
        }),
    );

    environment.restore(snapshots, suspended);

    let started = environment.now();

    let mut failures = 0;

    loop {
        let readiness = environment.restoration_readiness(suspended);

        let mut pending = Vec::new();
        let mut reported_failures = 0;

        for (index, state) in suspended.iter().copied().zip(readiness) {
            match state {
                RestorationReadiness::Pending => pending.push(index),
                RestorationReadiness::Failed(_) => reported_failures += 1,
                RestorationReadiness::Ready => {}
            }
        }

        failures = failures.max(reported_failures);

        environment.publish(
            UpdatePhase::Restoring,
            Some(UpdateProgress {
                completed: suspended.len() - pending.len(),
                total: suspended.len(),
            }),
        );

        if pending.is_empty() {
            return failures;
        }

        if environment.now() - started >= Duration::from_secs(30) {
            environment.recovery_timed_out(&pending);

            environment.publish(
                UpdatePhase::Restoring,
                Some(UpdateProgress {
                    completed: suspended.len(),
                    total: suspended.len(),
                }),
            );

            return failures + pending.len();
        }

        environment.wait(Duration::from_millis(100)).await;
    }
}
