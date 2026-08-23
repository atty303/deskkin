use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use application_core::{
    Availability, Command, Core, Effect, EffectRequest, Input, ReadCompleted, ReadError,
    RefreshDue, TimerArmCompleted, TimerArmError,
};
use slint::{ComponentHandle, Timer, TimerMode};

use crate::StatusWindow;
use crate::diagnostics::{
    ClosedValue, Completeness, DiagnosticRun, Operation, Recorder, RecordingHealth, RecordingMode,
    RunOutcome, SemanticRecord, finalize_operation_records, in_progress_run, new_run_id,
    now_unix_ms, resource_identity,
};
use crate::presenter::apply_view;

struct NativeRuntime {
    core: Core,
    ui: StatusWindow,
    read_timer: Timer,
    read_timeout_timer: Timer,
    refresh_timer: Timer,
    read_index: usize,
    logical_time_ms: u64,
    session_run_id: String,
    recorder: Recorder,
    active_read: Option<ActiveRead>,
}

struct ActiveRead {
    effect: Effect,
    run_id: String,
    started_ms: u64,
    started_at: Instant,
}

/// Runs the native Linux status window until it is closed.
///
/// # Errors
///
/// Returns an error when the Slint component, core startup, timer adapter, or
/// native event loop cannot start.
pub fn run_desktop(recording: RecordingMode) -> Result<(), String> {
    let ui = StatusWindow::new().map_err(|error| error.to_string())?;
    apply_view(&ui, application_core::StatusView::Unknown);
    let runtime = Rc::new(RefCell::new(NativeRuntime {
        core: Core::new(),
        ui: ui.clone_strong(),
        read_timer: Timer::default(),
        read_timeout_timer: Timer::default(),
        refresh_timer: Timer::default(),
        read_index: 0,
        logical_time_ms: 0,
        session_run_id: new_run_id("native"),
        recorder: Recorder::from_environment(recording),
        active_read: None,
    }));
    let transition = runtime
        .borrow_mut()
        .core
        .transition(Input::Command(Command::Start))
        .map_err(|error| format!("core start: {error:?}"))?;
    dispatch_effect(
        &runtime,
        transition.effect.ok_or("start did not request read")?,
    )?;
    let result = ui.run().map_err(|error| error.to_string());
    let mut state = runtime.borrow_mut();
    state.read_timer.stop();
    state.read_timeout_timer.stop();
    state.refresh_timer.stop();
    if let Some(active) = state.active_read.take() {
        let elapsed_ms = u64::try_from(active.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        state.logical_time_ms = active.started_ms.saturating_add(elapsed_ms);
        let _ = publish_interrupted_refresh(
            &state.recorder,
            &state.session_run_id,
            active,
            state.logical_time_ms,
            RunOutcome::Cancel,
        );
    }
    result
}

fn dispatch_effect(runtime: &Rc<RefCell<NativeRuntime>>, effect: Effect) -> Result<(), String> {
    match effect.request {
        EffectRequest::ReadAvailability => {
            let started_ms = runtime.borrow().logical_time_ms;
            let run_id = new_run_id("refresh");
            {
                let state = runtime.borrow();
                let _ = state.recorder.publish(in_progress_run(
                    run_id.clone(),
                    state.session_run_id.clone(),
                    effect.id.get(),
                    started_ms,
                ));
            }
            runtime.borrow_mut().active_read = Some(ActiveRead {
                effect,
                run_id,
                started_ms,
                started_at: Instant::now(),
            });
            let weak = Rc::downgrade(runtime);
            runtime.borrow().read_timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(250),
                move || complete_native_read(&weak),
            );
            let weak = Rc::downgrade(runtime);
            runtime.borrow().read_timeout_timer.start(
                TimerMode::SingleShot,
                Duration::from_secs(2),
                move || timeout_native_read(&weak),
            );
            Ok(())
        }
        EffectRequest::ArmRefreshTimer { delay_ms } => arm_native_timer(runtime, effect, delay_ms),
    }
}

fn complete_native_read(weak: &Weak<RefCell<NativeRuntime>>) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    let result = {
        let mut state = runtime.borrow_mut();
        if state.active_read.is_none() {
            return;
        }
        let result = match state.read_index % 3 {
            0 => Ok(Availability::Available),
            1 => Ok(Availability::Unavailable),
            _ => Err(ReadError::Unavailable),
        };
        state.read_index += 1;
        result
    };
    finish_native_read(&runtime, result, None, 250);
}

fn timeout_native_read(weak: &Weak<RefCell<NativeRuntime>>) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    finish_native_read(
        &runtime,
        Err(ReadError::Unavailable),
        Some(RunOutcome::Timeout),
        2_000,
    );
}

fn finish_native_read(
    runtime: &Rc<RefCell<NativeRuntime>>,
    result: Result<Availability, ReadError>,
    outcome_override: Option<RunOutcome>,
    elapsed_ms: u64,
) {
    let (effect, next_effect) = {
        let mut state = runtime.borrow_mut();
        let Some(active) = state.active_read.take() else {
            return;
        };
        state.read_timer.stop();
        state.read_timeout_timer.stop();
        state.logical_time_ms = active.started_ms.saturating_add(elapsed_ms);
        let Ok(transition) = state.core.transition(Input::ReadCompleted(ReadCompleted {
            effect_id: active.effect.id,
            result,
        })) else {
            return;
        };
        apply_view(&state.ui, transition.view);
        (active, transition.effect)
    };
    if let Some(timer) = next_effect {
        if arm_native_timer(runtime, timer, 5_000).is_err() {
            let mut state = runtime.borrow_mut();
            let _ = state
                .core
                .transition(Input::TimerArmCompleted(TimerArmCompleted {
                    effect_id: timer.id,
                    result: Err(TimerArmError::Unavailable),
                }));
            apply_view(&state.ui, state.core.view());
            publish_native_refresh(&state, &effect, timer, result, false, outcome_override);
        } else {
            publish_native_refresh(
                &runtime.borrow(),
                &effect,
                timer,
                result,
                true,
                outcome_override,
            );
        }
    }
}

fn arm_native_timer(
    runtime: &Rc<RefCell<NativeRuntime>>,
    effect: Effect,
    delay_ms: u32,
) -> Result<(), String> {
    let weak = Rc::downgrade(runtime);
    runtime.borrow().refresh_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(u64::from(delay_ms)),
        move || refresh_due(&weak, effect),
    );
    if !runtime.borrow().refresh_timer.running() {
        return Err("Slint refresh timer did not start".into());
    }
    runtime
        .borrow_mut()
        .core
        .transition(Input::TimerArmCompleted(TimerArmCompleted {
            effect_id: effect.id,
            result: Ok(()),
        }))
        .map_err(|error| format!("timer arm completion: {error:?}"))?;
    Ok(())
}

fn refresh_due(weak: &Weak<RefCell<NativeRuntime>>, timer: Effect) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    let next_effect = {
        let mut state = runtime.borrow_mut();
        state.logical_time_ms = state.logical_time_ms.saturating_add(5_000);
        state
            .core
            .transition(Input::RefreshDue(RefreshDue {
                effect_id: timer.id,
            }))
            .ok()
            .and_then(|transition| transition.effect)
    };
    if let Some(effect) = next_effect {
        let _ = dispatch_effect(&runtime, effect);
    }
}

fn publish_native_refresh(
    state: &NativeRuntime,
    active: &ActiveRead,
    timer: Effect,
    result: Result<Availability, ReadError>,
    arm_succeeded: bool,
    outcome_override: Option<RunOutcome>,
) {
    let (read_outcome, read_value, view_value) = match result {
        Ok(Availability::Available) => (
            RunOutcome::Success,
            ClosedValue::Available,
            ClosedValue::Available,
        ),
        Ok(Availability::Unavailable) => (
            RunOutcome::Success,
            ClosedValue::Unavailable,
            ClosedValue::Unavailable,
        ),
        Err(ReadError::Unavailable) => (
            RunOutcome::Error,
            outcome_override.map_or(ClosedValue::ReadFailed, |outcome| match outcome {
                RunOutcome::Timeout => ClosedValue::Timeout,
                RunOutcome::Cancel => ClosedValue::Cancel,
                RunOutcome::Error => ClosedValue::Error,
                RunOutcome::Success => ClosedValue::Success,
            }),
            ClosedValue::Unknown,
        ),
    };
    let outcome = if arm_succeeded {
        outcome_override.unwrap_or(read_outcome)
    } else {
        RunOutcome::Error
    };
    let records = native_refresh_records(
        active.started_ms,
        state.logical_time_ms,
        active.effect,
        timer,
        read_value,
        view_value,
        arm_succeeded,
    );
    let _ = state.recorder.publish(DiagnosticRun {
        schema_version: 1,
        resource: resource_identity(),
        run_id: active.run_id.clone(),
        scenario_run_id: state.session_run_id.clone(),
        outcome,
        completeness: Completeness::Complete,
        health: RecordingHealth::Healthy,
        terminal: true,
        missing_reason: None,
        owner: None,
        retained: false,
        created_unix_ms: now_unix_ms(),
        records,
    });
}

fn native_refresh_records(
    started_ms: u64,
    completed_ms: u64,
    read: Effect,
    timer: Effect,
    read_value: ClosedValue,
    view_value: ClosedValue,
    arm_succeeded: bool,
) -> Vec<SemanticRecord> {
    let mut records = vec![
        native_record(
            Operation::StatusRefresh,
            None,
            started_ms,
            ClosedValue::Started,
        ),
        native_record(
            Operation::EffectReadStatus,
            Some(read.id.get()),
            completed_ms,
            read_value,
        ),
        native_record(
            Operation::CoreTransition,
            Some(read.id.get()),
            completed_ms,
            view_value,
        ),
        SemanticRecord {
            operation: Operation::PresenterApplyView,
            operation_id: 0,
            parent_operation_id: None,
            status: crate::diagnostics::OperationStatus::Success,
            error_type: None,
            effect_id: None,
            virtual_time_ms: completed_ms,
            end_virtual_time_ms: completed_ms,
            duration_ms: None,
            render_width: Some(320),
            render_height: Some(240),
            value: Some(view_value),
        },
        SemanticRecord {
            operation: Operation::EffectArmRefreshTimer,
            operation_id: 0,
            parent_operation_id: None,
            status: crate::diagnostics::OperationStatus::Success,
            error_type: None,
            effect_id: Some(timer.id.get()),
            virtual_time_ms: completed_ms,
            end_virtual_time_ms: completed_ms,
            duration_ms: Some(5_000),
            render_width: None,
            render_height: None,
            value: Some(if arm_succeeded {
                ClosedValue::Armed
            } else {
                ClosedValue::ArmFailed
            }),
        },
    ];
    if !arm_succeeded {
        records.push(native_record(
            Operation::CoreTransition,
            Some(timer.id.get()),
            completed_ms,
            ClosedValue::Unknown,
        ));
        records.push(SemanticRecord {
            operation: Operation::PresenterApplyView,
            operation_id: 0,
            parent_operation_id: None,
            status: crate::diagnostics::OperationStatus::Success,
            error_type: None,
            effect_id: None,
            virtual_time_ms: completed_ms,
            end_virtual_time_ms: completed_ms,
            duration_ms: None,
            render_width: Some(320),
            render_height: Some(240),
            value: Some(ClosedValue::Unknown),
        });
    }
    let elapsed = u32::try_from(completed_ms.saturating_sub(started_ms)).ok();
    for record in &mut records {
        if matches!(
            record.operation,
            Operation::StatusRefresh | Operation::EffectReadStatus
        ) {
            record.duration_ms = elapsed;
        }
    }
    finalize_operation_records(
        &mut records,
        if arm_succeeded {
            match read_value {
                ClosedValue::Available | ClosedValue::Unavailable => RunOutcome::Success,
                _ => RunOutcome::Error,
            }
        } else {
            RunOutcome::Error
        },
    );
    records
}

fn publish_interrupted_refresh(
    recorder: &Recorder,
    session_run_id: &str,
    active: ActiveRead,
    completed_ms: u64,
    outcome: RunOutcome,
) -> crate::diagnostics::Publication {
    let value = match outcome {
        RunOutcome::Cancel => ClosedValue::Cancel,
        RunOutcome::Timeout => ClosedValue::Timeout,
        RunOutcome::Success => ClosedValue::Success,
        RunOutcome::Error => ClosedValue::Error,
    };
    recorder.publish(DiagnosticRun {
        schema_version: 1,
        resource: resource_identity(),
        run_id: active.run_id,
        scenario_run_id: session_run_id.into(),
        outcome,
        completeness: Completeness::Complete,
        health: RecordingHealth::Healthy,
        terminal: true,
        missing_reason: None,
        owner: None,
        retained: false,
        created_unix_ms: now_unix_ms(),
        records: interrupted_records(active.started_ms, completed_ms, active.effect, value),
    })
}

fn interrupted_records(
    started_ms: u64,
    completed_ms: u64,
    read: Effect,
    value: ClosedValue,
) -> Vec<SemanticRecord> {
    let mut records = vec![
        native_record(
            Operation::StatusRefresh,
            None,
            started_ms,
            ClosedValue::Started,
        ),
        native_record(
            Operation::EffectReadStatus,
            Some(read.id.get()),
            completed_ms,
            value,
        ),
    ];
    let elapsed = u32::try_from(completed_ms.saturating_sub(started_ms)).ok();
    for record in &mut records {
        record.duration_ms = elapsed;
    }
    let outcome = match value {
        ClosedValue::Cancel => RunOutcome::Cancel,
        ClosedValue::Timeout => RunOutcome::Timeout,
        _ => RunOutcome::Error,
    };
    finalize_operation_records(&mut records, outcome);
    records
}

fn native_record(
    operation: Operation,
    effect_id: Option<u64>,
    virtual_time_ms: u64,
    value: ClosedValue,
) -> SemanticRecord {
    SemanticRecord {
        operation,
        operation_id: 0,
        parent_operation_id: None,
        status: crate::diagnostics::OperationStatus::Success,
        error_type: None,
        effect_id,
        virtual_time_ms,
        end_virtual_time_ms: virtual_time_ms,
        duration_ms: None,
        render_width: None,
        render_height: None,
        value: Some(value),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use application_core::{Command, Core, Input, ReadCompleted};

    use super::*;

    fn effects() -> (Effect, Effect) {
        let mut core = Core::new();
        let read = core
            .transition(Input::Command(Command::Start))
            .unwrap()
            .effect
            .unwrap();
        let timer = core
            .transition(Input::ReadCompleted(ReadCompleted {
                effect_id: read.id,
                result: Ok(Availability::Available),
            }))
            .unwrap()
            .effect
            .unwrap();
        (read, timer)
    }

    #[test]
    fn arm_failure_records_terminal_unknown_view() {
        let (read, timer) = effects();
        let records = native_refresh_records(
            0,
            250,
            read,
            timer,
            ClosedValue::Available,
            ClosedValue::Available,
            false,
        );
        assert!(records.iter().any(|record| {
            record.operation == Operation::EffectArmRefreshTimer
                && record.value == Some(ClosedValue::ArmFailed)
        }));
        assert!(records.iter().any(|record| {
            record.operation == Operation::PresenterApplyView
                && record.value == Some(ClosedValue::Unknown)
        }));
    }

    #[test]
    fn read_failure_records_failed_effect_and_unknown_view() {
        let (read, timer) = effects();
        let records = native_refresh_records(
            0,
            250,
            read,
            timer,
            ClosedValue::ReadFailed,
            ClosedValue::Unknown,
            true,
        );
        assert!(records.iter().any(|record| {
            record.operation == Operation::EffectReadStatus
                && record.value == Some(ClosedValue::ReadFailed)
        }));
        assert!(records.iter().any(|record| {
            record.operation == Operation::PresenterApplyView
                && record.value == Some(ClosedValue::Unknown)
        }));
    }

    #[test]
    fn cancel_and_timeout_terminal_records_are_distinct() {
        let (read, _) = effects();
        let cancel = interrupted_records(0, 125, read, ClosedValue::Cancel);
        let timeout = interrupted_records(0, 2_000, read, ClosedValue::Timeout);
        assert_ne!(cancel, timeout);
        assert_eq!(cancel.last().unwrap().value, Some(ClosedValue::Cancel));
        assert_eq!(timeout.last().unwrap().value, Some(ClosedValue::Timeout));
    }

    #[test]
    fn cancel_and_timeout_publish_correlated_terminal_runs() {
        let (read, _) = effects();
        for (outcome, completed_ms, expected) in [
            (RunOutcome::Cancel, 125, ClosedValue::Cancel),
            (RunOutcome::Timeout, 2_000, ClosedValue::Timeout),
        ] {
            let root = std::env::temp_dir().join(new_run_id("runtime-publication"));
            let recorder = Recorder::at_root(root.clone());
            let active = ActiveRead {
                effect: read,
                run_id: new_run_id("refresh-test"),
                started_ms: 0,
                started_at: Instant::now(),
            };
            let publication = publish_interrupted_refresh(
                &recorder,
                "native-session",
                active,
                completed_ms,
                outcome,
            );
            assert_eq!(publication.completeness, Completeness::Complete);
            assert_eq!(publication.health, RecordingHealth::Healthy);
            assert!(publication.stored);
            let saved: DiagnosticRun = serde_json::from_slice(
                &fs::read(root.join(format!("diagnostics/{}.json", publication.run_id))).unwrap(),
            )
            .unwrap();
            assert_eq!(saved.scenario_run_id, "native-session");
            assert_eq!(saved.outcome, outcome);
            assert_eq!(saved.resource, resource_identity());
            assert_eq!(saved.records[0].virtual_time_ms, 0);
            assert_eq!(
                saved.records[0].duration_ms,
                u32::try_from(completed_ms).ok()
            );
            assert_eq!(saved.records[1].virtual_time_ms, 0);
            assert_eq!(saved.records[1].end_virtual_time_ms, completed_ms);
            assert_eq!(saved.records[1].value, Some(expected));
        }
    }
}
