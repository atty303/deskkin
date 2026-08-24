use std::fmt;
use std::rc::Rc;

use application_core::{
    Availability, AvailabilityInvalidated, Command, Core, Effect, EffectRequest, Input,
    ReadCompleted, ReadError, RefreshDue, StatusView, TimerArmCompleted,
};
use serde::{Deserialize, Serialize};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

use crate::StatusWindow;
use crate::diagnostics::{
    ClosedValue, Completeness, DiagnosticRun, ErrorType, Operation, OperationStatus, Publication,
    Recorder, RecordingHealth, RecordingMode, RunOutcome, SemanticRecord,
    finalize_operation_records, in_progress_run, new_run_id, now_unix_ms, publish_scenario_result,
    resource_identity,
};
use crate::presenter::apply_view;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

thread_local! {
    static HEADLESS_WINDOW: Rc<MinimalSoftwareWindow> =
        MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
}

struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(HEADLESS_WINDOW.with(Clone::clone))
    }
}

fn setup_headless() -> Rc<MinimalSoftwareWindow> {
    let _ = slint::platform::set_platform(Box::new(HeadlessPlatform));
    HEADLESS_WINDOW.with(|window| {
        window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
        window.clone()
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScenarioName {
    PeriodicSuccess,
    PeriodicReadFailure,
    ProtocolDisconnectRecovery,
}

impl ScenarioName {
    /// Parses one of the two closed scenario names.
    ///
    /// # Errors
    ///
    /// Returns an error for any name outside the closed scenario set.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "periodic-success" => Ok(Self::PeriodicSuccess),
            "periodic-read-failure" => Ok(Self::PeriodicReadFailure),
            "protocol-disconnect-recovery" => Ok(Self::ProtocolDisconnectRecovery),
            _ => Err(format!("unknown scenario: {value}")),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::PeriodicSuccess => "periodic-success",
            Self::PeriodicReadFailure => "periodic-read-failure",
            Self::ProtocolDisconnectRecovery => "protocol-disconnect-recovery",
        }
    }
}

impl fmt::Display for ScenarioName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TraceKind {
    CommandStart,
    ReadRequested,
    ReadAvailable,
    ReadUnavailable,
    ReadFailed,
    TimerArmRequested,
    TimerArmed,
    RefreshDue,
    ViewApplied,
    SourceUnavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TraceRecord {
    virtual_time_ms: u64,
    kind: TraceKind,
    effect_id: Option<u64>,
    view: Option<ViewName>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ViewName {
    Unknown,
    Available,
    Unavailable,
}

impl From<StatusView> for ViewName {
    fn from(value: StatusView) -> Self {
        match value {
            StatusView::Unknown => Self::Unknown,
            StatusView::Available => Self::Available,
            StatusView::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Replay {
    semantic_records: Vec<TraceRecord>,
    views: Vec<ViewName>,
    virtual_timestamps_ms: Vec<u64>,
    rgb565_frames: Vec<Vec<u8>>,
}

struct RefreshEvidence {
    outcome: RunOutcome,
    records: Vec<SemanticRecord>,
}

struct ExecutedReplay {
    replay: Replay,
    refreshes: Vec<RefreshEvidence>,
}

#[derive(Serialize)]
struct ScenarioResult {
    schema_version: u8,
    result: &'static str,
    scenario: ScenarioName,
    scenario_run_id: String,
    created_unix_ms: u64,
    child_refresh_runs: Vec<Publication>,
    replay_equal: bool,
    recording_health: RecordingHealth,
    protocol_major: u8,
    selected_features: [u8; 8],
    granted_permissions: [u8; 8],
    replay: Replay,
}

/// Executes two fresh deterministic replays and atomically publishes the result.
///
/// # Errors
///
/// Returns an error for a core, presentation, rendering, replay-comparison, or
/// atomic result-publication failure.
pub fn run_scenario_command(
    scenario: ScenarioName,
    recording: RecordingMode,
) -> Result<String, String> {
    let window = setup_headless();
    let scenario_run_id = new_run_id("scenario");
    let recorder = Recorder::from_environment(recording);
    let mut first_run_ids = Vec::new();
    let first = execute_replay(scenario, &window, &mut |effect, started_ms| {
        let run_id = new_run_id("refresh");
        let _ = recorder.publish(in_progress_run(
            run_id.clone(),
            scenario_run_id.clone(),
            effect.id.get(),
            started_ms,
        ));
        first_run_ids.push((run_id, effect, started_ms));
    });
    let first = match first {
        Ok(replay) => replay,
        Err(error) => {
            terminalize_scenario_error(&recorder, &scenario_run_id, &first_run_ids);
            return Err(error);
        }
    };
    let mut second_run_ids = Vec::new();
    let second = execute_replay(scenario, &window, &mut |effect, started_ms| {
        let run_id = new_run_id("refresh");
        let _ = recorder.publish(in_progress_run(
            run_id.clone(),
            scenario_run_id.clone(),
            effect.id.get(),
            started_ms,
        ));
        second_run_ids.push((run_id, effect, started_ms));
    });
    let second = match second {
        Ok(replay) => replay,
        Err(error) => {
            terminalize_scenario_error(&recorder, &scenario_run_id, &first_run_ids);
            terminalize_scenario_error(&recorder, &scenario_run_id, &second_run_ids);
            return Err(error);
        }
    };
    if first.replay != second.replay {
        terminalize_scenario_error(&recorder, &scenario_run_id, &first_run_ids);
        terminalize_scenario_error(&recorder, &scenario_run_id, &second_run_ids);
        return Err("deterministic replay mismatch".into());
    }

    let mut child_runs = Vec::new();
    let mut health = RecordingHealth::Healthy;
    for (replay_index, (replay, run_ids)) in [
        (first.refreshes, first_run_ids),
        (second.refreshes, second_run_ids),
    ]
    .into_iter()
    .enumerate()
    {
        for (refresh_index, (refresh, (run_id, _, _))) in
            replay.into_iter().zip(run_ids).enumerate()
        {
            let run = DiagnosticRun {
                schema_version: 1,
                resource: resource_identity(),
                run_id: run_id.clone(),
                scenario_run_id: scenario_run_id.clone(),
                transaction_id: None,
                session_context_id: None,
                operation_context_id: None,
                protocol_major: Some(1),
                selected_features: Some(deskkin_protocol::AVAILABILITY_READ_V1.0),
                granted_permissions: Some(deskkin_protocol::AVAILABILITY_READ_PERMISSION.0),
                outcome: refresh.outcome,
                completeness: Completeness::Complete,
                health: RecordingHealth::Healthy,
                terminal: true,
                missing_reason: None,
                owner: None,
                retained: false,
                created_unix_ms: now_unix_ms()
                    .saturating_add(u64::try_from(replay_index * 2 + refresh_index).unwrap_or(0)),
                records: refresh.records,
            };
            let publication = recorder.publish(run);
            health = merge_health(health, publication.health);
            child_runs.push(publication);
        }
    }

    publish_replay_result(scenario, &scenario_run_id, child_runs, health, first.replay)
}

fn publish_replay_result(
    scenario: ScenarioName,
    scenario_run_id: &str,
    child_runs: Vec<Publication>,
    health: RecordingHealth,
    replay: Replay,
) -> Result<String, String> {
    let result = ScenarioResult {
        schema_version: 1,
        result: "pass",
        scenario,
        scenario_run_id: scenario_run_id.to_owned(),
        created_unix_ms: now_unix_ms(),
        child_refresh_runs: child_runs,
        replay_equal: true,
        recording_health: health,
        protocol_major: 1,
        selected_features: deskkin_protocol::AVAILABILITY_READ_V1.0,
        granted_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION.0,
        replay,
    };
    let bytes = serde_json::to_vec(&result).map_err(|error| error.to_string())?;
    let path =
        publish_scenario_result(scenario.as_str(), &bytes).map_err(|error| error.to_string())?;
    Ok(format!(
        "result=pass run_id={} result_path={}",
        scenario_run_id,
        path.display()
    ))
}

fn terminalize_scenario_error(
    recorder: &Recorder,
    scenario_run_id: &str,
    markers: &[(String, Effect, u64)],
) {
    for (run_id, effect, started_ms) in markers {
        let mut run = in_progress_run(
            run_id.clone(),
            scenario_run_id.into(),
            effect.id.get(),
            *started_ms,
        );
        run.terminal = true;
        run.completeness = Completeness::Complete;
        run.owner = None;
        for record in &mut run.records {
            record.status = OperationStatus::Error;
            record.error_type = Some(ErrorType::ScenarioFailed);
        }
        let _ = recorder.publish(run);
    }
}

const fn merge_health(left: RecordingHealth, right: RecordingHealth) -> RecordingHealth {
    match (left, right) {
        (RecordingHealth::StorageUnavailable, _) | (_, RecordingHealth::StorageUnavailable) => {
            RecordingHealth::StorageUnavailable
        }
        (RecordingHealth::CapacityExhausted, _) | (_, RecordingHealth::CapacityExhausted) => {
            RecordingHealth::CapacityExhausted
        }
        _ => RecordingHealth::Healthy,
    }
}

fn execute_replay(
    scenario: ScenarioName,
    window: &Rc<MinimalSoftwareWindow>,
    on_refresh_started: &mut impl FnMut(Effect, u64),
) -> Result<ExecutedReplay, String> {
    let ui = StatusWindow::new().map_err(|error| error.to_string())?;
    ui.show().map_err(|error| error.to_string())?;
    let mut core = Core::new();
    let mut trace = vec![TraceRecord {
        virtual_time_ms: 0,
        kind: TraceKind::CommandStart,
        effect_id: None,
        view: Some(ViewName::Unknown),
    }];
    apply_view(&ui, core.view());
    let mut frames = vec![render(window)?];
    let mut views = vec![ViewName::Unknown];
    let mut times = vec![0];
    let mut refreshes = Vec::new();

    let first_read = core
        .transition(Input::Command(Command::Start))
        .map_err(|error| format!("core start: {error:?}"))?
        .effect
        .ok_or("start did not request read")?;
    trace.push(effect_trace(0, first_read));
    on_refresh_started(first_read, 0);
    let (first_result, second_result) = scenario_results(scenario);
    let first_timer = complete_read(
        &mut core,
        &ui,
        &mut trace,
        &mut views,
        &mut times,
        &mut frames,
        window,
        250,
        first_read,
        first_result,
    )?;
    refreshes.push(arm_timer(
        &mut core,
        0,
        250,
        first_read,
        first_timer,
        first_result,
    )?);

    if scenario == ScenarioName::ProtocolDisconnectRecovery {
        apply_source_disconnect(
            &mut core,
            &ui,
            &mut trace,
            &mut views,
            &mut times,
            &mut frames,
            window,
        )?;
    }

    trace.push(TraceRecord {
        virtual_time_ms: 5_250,
        kind: TraceKind::RefreshDue,
        effect_id: Some(first_timer.id.get()),
        view: None,
    });
    let second_read = core
        .transition(Input::RefreshDue(RefreshDue {
            effect_id: first_timer.id,
        }))
        .map_err(|error| format!("refresh due: {error:?}"))?
        .effect
        .ok_or("refresh due did not request read")?;
    trace.push(effect_trace(5_250, second_read));
    on_refresh_started(second_read, 5_250);
    let second_timer = complete_read(
        &mut core,
        &ui,
        &mut trace,
        &mut views,
        &mut times,
        &mut frames,
        window,
        5_500,
        second_read,
        second_result,
    )?;
    refreshes.push(arm_timer(
        &mut core,
        5_250,
        5_500,
        second_read,
        second_timer,
        second_result,
    )?);
    ui.hide().map_err(|error| error.to_string())?;

    Ok(ExecutedReplay {
        replay: Replay {
            semantic_records: trace,
            views,
            virtual_timestamps_ms: times,
            rgb565_frames: frames,
        },
        refreshes,
    })
}

const fn scenario_results(
    scenario: ScenarioName,
) -> (
    Result<Availability, ReadError>,
    Result<Availability, ReadError>,
) {
    match scenario {
        ScenarioName::PeriodicSuccess => {
            (Ok(Availability::Available), Ok(Availability::Unavailable))
        }
        ScenarioName::PeriodicReadFailure => {
            (Err(ReadError::Unavailable), Ok(Availability::Available))
        }
        ScenarioName::ProtocolDisconnectRecovery => {
            (Ok(Availability::Available), Ok(Availability::Available))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_source_disconnect(
    core: &mut Core,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    views: &mut Vec<ViewName>,
    times: &mut Vec<u64>,
    frames: &mut Vec<Vec<u8>>,
    window: &Rc<MinimalSoftwareWindow>,
) -> Result<(), String> {
    trace.push(TraceRecord {
        virtual_time_ms: 1_000,
        kind: TraceKind::SourceUnavailable,
        effect_id: None,
        view: None,
    });
    let transition = core
        .transition(Input::AvailabilityInvalidated(
            AvailabilityInvalidated::SourceUnavailable,
        ))
        .map_err(|error| format!("source invalidation: {error:?}"))?;
    if transition.effect.is_some() {
        return Err("waiting disconnect changed the armed timer".into());
    }
    apply_view(ui, transition.view);
    trace.push(TraceRecord {
        virtual_time_ms: 1_000,
        kind: TraceKind::ViewApplied,
        effect_id: None,
        view: Some(transition.view.into()),
    });
    views.push(transition.view.into());
    times.push(1_000);
    frames.push(render(window)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn complete_read(
    core: &mut Core,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    views: &mut Vec<ViewName>,
    times: &mut Vec<u64>,
    frames: &mut Vec<Vec<u8>>,
    window: &Rc<MinimalSoftwareWindow>,
    virtual_time_ms: u64,
    read: Effect,
    result: Result<Availability, ReadError>,
) -> Result<Effect, String> {
    let kind = match result {
        Ok(Availability::Available) => TraceKind::ReadAvailable,
        Ok(Availability::Unavailable) => TraceKind::ReadUnavailable,
        Err(ReadError::Unavailable) => TraceKind::ReadFailed,
    };
    trace.push(TraceRecord {
        virtual_time_ms,
        kind,
        effect_id: Some(read.id.get()),
        view: None,
    });
    let transition = core
        .transition(Input::ReadCompleted(ReadCompleted {
            effect_id: read.id,
            result,
        }))
        .map_err(|error| format!("read completion: {error:?}"))?;
    apply_view(ui, transition.view);
    trace.push(TraceRecord {
        virtual_time_ms,
        kind: TraceKind::ViewApplied,
        effect_id: None,
        view: Some(transition.view.into()),
    });
    views.push(transition.view.into());
    times.push(virtual_time_ms);
    frames.push(render(window)?);
    let timer_effect = transition.effect.ok_or("read did not request timer")?;
    trace.push(effect_trace(virtual_time_ms, timer_effect));
    Ok(timer_effect)
}

fn arm_timer(
    core: &mut Core,
    started_ms: u64,
    completed_ms: u64,
    read: Effect,
    timer: Effect,
    read_result: Result<Availability, ReadError>,
) -> Result<RefreshEvidence, String> {
    core.transition(Input::TimerArmCompleted(TimerArmCompleted {
        effect_id: timer.id,
        result: Ok(()),
    }))
    .map_err(|error| format!("timer arm: {error:?}"))?;
    let (read_value, view_value) = match read_result {
        Ok(Availability::Available) => (ClosedValue::Available, ClosedValue::Available),
        Ok(Availability::Unavailable) => (ClosedValue::Unavailable, ClosedValue::Unavailable),
        Err(ReadError::Unavailable) => (ClosedValue::ReadFailed, ClosedValue::Unknown),
    };
    let outcome = if read_result.is_ok() {
        RunOutcome::Success
    } else {
        RunOutcome::Error
    };
    let mut records: Vec<_> = vec![
        diagnostic(
            Operation::StatusRefresh,
            None,
            started_ms,
            Some(ClosedValue::Started),
        ),
        diagnostic(
            Operation::EffectReadStatus,
            Some(read.id.get()),
            completed_ms,
            Some(read_value),
        ),
        diagnostic(
            Operation::CoreTransition,
            Some(read.id.get()),
            completed_ms,
            Some(view_value),
        ),
        diagnostic(
            Operation::PresenterApplyView,
            None,
            completed_ms,
            Some(view_value),
        ),
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
            value: Some(ClosedValue::Armed),
        },
    ]
    .into_iter()
    .map(|mut record| {
        if matches!(
            record.operation,
            Operation::StatusRefresh | Operation::EffectReadStatus
        ) {
            record.duration_ms = u32::try_from(completed_ms.saturating_sub(started_ms)).ok();
        }
        record
    })
    .collect();
    finalize_operation_records(&mut records, outcome);
    Ok(RefreshEvidence { outcome, records })
}

fn diagnostic(
    operation: Operation,
    effect_id: Option<u64>,
    virtual_time_ms: u64,
    value: Option<ClosedValue>,
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
        render_width: (operation == Operation::PresenterApplyView).then_some(WIDTH),
        render_height: (operation == Operation::PresenterApplyView).then_some(HEIGHT),
        value,
    }
}

fn effect_trace(virtual_time_ms: u64, effect: Effect) -> TraceRecord {
    let kind = match effect.request {
        EffectRequest::ReadAvailability => TraceKind::ReadRequested,
        EffectRequest::ArmRefreshTimer { .. } => TraceKind::TimerArmRequested,
    };
    TraceRecord {
        virtual_time_ms,
        kind,
        effect_id: Some(effect.id.get()),
        view: None,
    }
}

fn render(window: &Rc<MinimalSoftwareWindow>) -> Result<Vec<u8>, String> {
    window.request_redraw();
    let mut pixels = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, WIDTH as usize);
    });
    if !drawn {
        return Err("software renderer did not draw".into());
    }
    let mut bytes = Vec::with_capacity(pixels.len() * 2);
    for pixel in pixels {
        bytes.extend_from_slice(&pixel.0.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_scenarios_replay_identically_with_expected_timeline() {
        let window = setup_headless();
        for scenario in [
            ScenarioName::PeriodicSuccess,
            ScenarioName::PeriodicReadFailure,
            ScenarioName::ProtocolDisconnectRecovery,
        ] {
            let first = execute_replay(scenario, &window, &mut |_, _| {}).unwrap();
            let second = execute_replay(scenario, &window, &mut |_, _| {}).unwrap();
            assert_eq!(first.replay, second.replay);
            if scenario == ScenarioName::ProtocolDisconnectRecovery {
                assert_eq!(first.replay.virtual_timestamps_ms, [0, 250, 1_000, 5_500]);
                assert_eq!(
                    first.replay.views,
                    [
                        ViewName::Unknown,
                        ViewName::Available,
                        ViewName::Unknown,
                        ViewName::Available
                    ]
                );
            } else {
                assert_eq!(first.replay.virtual_timestamps_ms, [0, 250, 5_500]);
            }
            assert!(
                first
                    .replay
                    .semantic_records
                    .iter()
                    .any(|record| record.virtual_time_ms == 5_250
                        && record.kind == TraceKind::RefreshDue)
            );
            let first_refresh = &first.refreshes[0].records;
            assert_eq!(first_refresh[0].virtual_time_ms, 0);
            assert_eq!(first_refresh[0].duration_ms, Some(250));
            assert_eq!(first_refresh[1].virtual_time_ms, 0);
            assert_eq!(first_refresh[1].end_virtual_time_ms, 250);
            assert_eq!(first_refresh[1].duration_ms, Some(250));
        }
    }

    #[test]
    fn failure_is_unknown_then_recovers() {
        let window = setup_headless();
        let result =
            execute_replay(ScenarioName::PeriodicReadFailure, &window, &mut |_, _| {}).unwrap();
        assert_eq!(
            result.replay.views,
            [ViewName::Unknown, ViewName::Unknown, ViewName::Available]
        );
        let failed_refresh = &result.refreshes[0].records;
        assert!(failed_refresh.iter().any(|record| {
            record.operation == Operation::EffectReadStatus
                && record.value == Some(ClosedValue::ReadFailed)
        }));
        assert!(failed_refresh.iter().any(|record| {
            record.operation == Operation::PresenterApplyView
                && record.value == Some(ClosedValue::Unknown)
        }));
    }

    #[test]
    fn driver_outcomes_keep_cancel_and_timeout_distinct() {
        assert_ne!(RunOutcome::Cancel, RunOutcome::Timeout);
        assert_eq!(ClosedValue::Cancel, ClosedValue::Cancel);
        assert_eq!(ClosedValue::Timeout, ClosedValue::Timeout);
    }

    #[test]
    fn clean_scenario_failure_terminalizes_started_markers() {
        let root = std::env::temp_dir().join(new_run_id("scenario-failure"));
        let recorder = Recorder::at_root(root.clone());
        let mut core = Core::new();
        let read = core
            .transition(Input::Command(Command::Start))
            .unwrap()
            .effect
            .unwrap();
        let markers: Vec<(String, Effect, u64)> = vec![("refresh-failed".into(), read, 0)];
        let _ = recorder.publish(in_progress_run(
            markers[0].0.clone(),
            "scenario-failed".into(),
            read.id.get(),
            0,
        ));
        terminalize_scenario_error(&recorder, "scenario-failed", &markers);
        let saved: DiagnosticRun = serde_json::from_slice(
            &std::fs::read(root.join("diagnostics/refresh-failed.json")).unwrap(),
        )
        .unwrap();
        assert!(saved.terminal);
        assert!(saved.records.iter().all(|record| {
            record.status == OperationStatus::Error
                && record.error_type == Some(ErrorType::ScenarioFailed)
        }));
    }
}
