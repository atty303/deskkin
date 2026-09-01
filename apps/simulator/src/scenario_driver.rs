use std::fmt;
use std::rc::Rc;

use deskkin_application::{
    Application, ApplicationInput, ApplicationViews, Effect, EffectRequest, Lifecycle,
    availability::{
        self, Availability, Input as AvailabilityInput, ReadCompleted, ReadError, RefreshDue,
        TimerArmCompleted,
    },
    synthetic_notice,
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
        MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
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
    MultiFeatureComposition,
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
            "multi-feature-composition" => Ok(Self::MultiFeatureComposition),
            _ => Err(format!("unknown scenario: {value}")),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::PeriodicSuccess => "periodic-success",
            Self::PeriodicReadFailure => "periodic-read-failure",
            Self::ProtocolDisconnectRecovery => "protocol-disconnect-recovery",
            Self::MultiFeatureComposition => "multi-feature-composition",
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
    NoticeShow,
    NoticeArmRequested,
    NoticeArmed,
    NoticeExpired,
    SessionInvalidated,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TraceRecord {
    virtual_time_ms: u64,
    kind: TraceKind,
    effect_id: Option<u64>,
    view: Option<ViewSetName>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FeatureName {
    Availability,
    SyntheticNotice,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleName {
    Start,
    SessionInvalidated,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EffectName {
    ReadAvailability,
    ArmRefreshTimer,
    ArmNoticeExpiry,
    NoticeExpiryDue,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TransitionOutcome {
    Success,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CompositionRecord {
    virtual_time_ms: u64,
    routed_feature: Option<FeatureName>,
    lifecycle: Option<LifecycleName>,
    effect: Option<EffectName>,
    views: ViewSetName,
    outcome: TransitionOutcome,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AvailabilityViewName {
    Unknown,
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum NoticeViewName {
    CompositionCheck,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ViewSetName {
    availability: Option<AvailabilityViewName>,
    synthetic_notice: Option<NoticeViewName>,
}

impl ViewSetName {
    const UNKNOWN: Self = Self {
        availability: Some(AvailabilityViewName::Unknown),
        synthetic_notice: None,
    };
    #[cfg(test)]
    const AVAILABLE: Self = Self {
        availability: Some(AvailabilityViewName::Available),
        synthetic_notice: None,
    };
    #[cfg(test)]
    const UNAVAILABLE: Self = Self {
        availability: Some(AvailabilityViewName::Unavailable),
        synthetic_notice: None,
    };
    #[cfg(test)]
    const AVAILABLE_WITH_NOTICE: Self = Self {
        availability: Some(AvailabilityViewName::Available),
        synthetic_notice: Some(NoticeViewName::CompositionCheck),
    };
    #[cfg(test)]
    const UNAVAILABLE_WITH_NOTICE: Self = Self {
        availability: Some(AvailabilityViewName::Unavailable),
        synthetic_notice: Some(NoticeViewName::CompositionCheck),
    };
}

impl From<ApplicationViews> for ViewSetName {
    fn from(value: ApplicationViews) -> Self {
        Self {
            availability: value.availability.map(|availability| match availability {
                availability::Surface::Unknown => AvailabilityViewName::Unknown,
                availability::Surface::Available => AvailabilityViewName::Available,
                availability::Surface::Unavailable => AvailabilityViewName::Unavailable,
            }),
            synthetic_notice: value.synthetic_notice.map(|notice| match notice {
                synthetic_notice::NoticeKind::CompositionCheck => NoticeViewName::CompositionCheck,
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Replay {
    semantic_records: Vec<TraceRecord>,
    composition_records: Vec<CompositionRecord>,
    views: Vec<ViewSetName>,
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

struct ReplayExecution {
    ui: StatusWindow,
    application: Application,
    trace: Vec<TraceRecord>,
    composition_records: Vec<CompositionRecord>,
    views: Vec<ViewSetName>,
    times: Vec<u64>,
    frames: Vec<Vec<u8>>,
    refreshes: Vec<RefreshEvidence>,
    first_timer: Effect,
    second_result: Result<Availability, ReadError>,
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
            effect.id.local.get(),
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
            effect.id.local.get(),
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
            effect.id.local.get(),
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

fn initialize_replay(
    scenario: ScenarioName,
    window: &Rc<MinimalSoftwareWindow>,
    on_refresh_started: &mut impl FnMut(Effect, u64),
) -> Result<ReplayExecution, String> {
    let ui = StatusWindow::new().map_err(|error| error.to_string())?;
    ui.show().map_err(|error| error.to_string())?;
    let mut application = Application::new();
    let mut trace = vec![TraceRecord {
        virtual_time_ms: 0,
        kind: TraceKind::CommandStart,
        effect_id: None,
        view: Some(ViewSetName::UNKNOWN),
    }];
    let start = application
        .transition(ApplicationInput::Lifecycle(Lifecycle::Start))
        .map_err(|error| format!("application start: {error:?}"))?;
    apply_view(&ui, start.view);
    let mut frames = vec![render(window)?];
    let mut views = vec![start.view.into()];
    let mut times = vec![0];
    let mut refreshes = Vec::new();
    let composition_records = vec![CompositionRecord {
        virtual_time_ms: 0,
        routed_feature: None,
        lifecycle: Some(LifecycleName::Start),
        effect: Some(EffectName::ReadAvailability),
        views: start.view.into(),
        outcome: TransitionOutcome::Success,
    }];

    let first_read = start.effects.get(0).ok_or("start did not request read")?;
    trace.push(effect_trace(0, first_read));
    on_refresh_started(first_read, 0);
    let (first_result, second_result) = scenario_results(scenario);
    let first_timer = complete_read(
        &mut application,
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
        &mut application,
        0,
        250,
        first_read,
        first_timer,
        first_result,
    )?);

    Ok(ReplayExecution {
        ui,
        application,
        trace,
        composition_records,
        views,
        times,
        frames,
        refreshes,
        first_timer,
        second_result,
    })
}

fn execute_replay(
    scenario: ScenarioName,
    window: &Rc<MinimalSoftwareWindow>,
    on_refresh_started: &mut impl FnMut(Effect, u64),
) -> Result<ExecutedReplay, String> {
    let ReplayExecution {
        ui,
        mut application,
        mut trace,
        mut composition_records,
        mut views,
        mut times,
        mut frames,
        mut refreshes,
        first_timer,
        second_result,
    } = initialize_replay(scenario, window, on_refresh_started)?;

    let notice_timer = if scenario == ScenarioName::MultiFeatureComposition {
        Some(show_notice_and_arm(
            &mut application,
            &ui,
            &mut trace,
            &mut composition_records,
            &mut views,
            &mut times,
            &mut frames,
            window,
            4_000,
        )?)
    } else {
        None
    };

    if scenario == ScenarioName::ProtocolDisconnectRecovery {
        apply_source_disconnect(
            &mut application,
            &ui,
            &mut trace,
            &mut views,
            &mut times,
            &mut frames,
            window,
        )?;
    }

    let (second_timer, second_refresh) = complete_refresh_cycle(
        &mut application,
        &ui,
        &mut trace,
        &mut views,
        &mut times,
        &mut frames,
        window,
        5_250,
        5_500,
        first_timer,
        second_result,
        on_refresh_started,
    )?;
    refreshes.push(second_refresh);
    composition_records.push(CompositionRecord {
        virtual_time_ms: 5_500,
        routed_feature: Some(FeatureName::Availability),
        lifecycle: None,
        effect: Some(EffectName::ArmRefreshTimer),
        views: application.view().into(),
        outcome: TransitionOutcome::Success,
    });

    if let Some(notice) = notice_timer {
        complete_multi_feature_tail(
            &mut application,
            &ui,
            &mut trace,
            &mut composition_records,
            &mut views,
            &mut times,
            &mut frames,
            &mut refreshes,
            window,
            on_refresh_started,
            notice,
            second_timer,
        )?;
    }
    ui.hide().map_err(|error| error.to_string())?;

    Ok(ExecutedReplay {
        replay: Replay {
            semantic_records: trace,
            composition_records,
            views,
            virtual_timestamps_ms: times,
            rgb565_frames: frames,
        },
        refreshes,
    })
}

#[allow(clippy::too_many_arguments)]
fn show_notice_and_arm(
    application: &mut Application,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    composition_records: &mut Vec<CompositionRecord>,
    views: &mut Vec<ViewSetName>,
    times: &mut Vec<u64>,
    frames: &mut Vec<Vec<u8>>,
    window: &Rc<MinimalSoftwareWindow>,
    virtual_time_ms: u64,
) -> Result<Effect, String> {
    trace.push(TraceRecord {
        virtual_time_ms,
        kind: TraceKind::NoticeShow,
        effect_id: None,
        view: None,
    });
    let transition = application
        .transition(ApplicationInput::synthetic_notice_command(
            synthetic_notice::Command::Show(synthetic_notice::NoticeKind::CompositionCheck),
        ))
        .map_err(|error| format!("notice show: {error:?}"))?;
    let notice = transition
        .effects
        .get(0)
        .ok_or("notice show did not request expiry timer")?;
    trace.push(effect_trace(virtual_time_ms, notice));
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
    composition_records.push(CompositionRecord {
        virtual_time_ms,
        routed_feature: Some(FeatureName::SyntheticNotice),
        lifecycle: None,
        effect: Some(EffectName::ArmNoticeExpiry),
        views: transition.view.into(),
        outcome: TransitionOutcome::Success,
    });
    application
        .transition(ApplicationInput::synthetic_notice_effect(
            notice.id,
            synthetic_notice::Input::TimerArmCompleted(synthetic_notice::TimerArmCompleted {
                effect_id: notice.id.local,
                result: Ok(()),
            }),
        ))
        .map_err(|error| format!("notice timer arm: {error:?}"))?;
    trace.push(TraceRecord {
        virtual_time_ms,
        kind: TraceKind::NoticeArmed,
        effect_id: Some(notice.id.local.get()),
        view: None,
    });
    Ok(notice)
}

#[allow(clippy::too_many_arguments)]
fn complete_refresh_cycle(
    application: &mut Application,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    views: &mut Vec<ViewSetName>,
    times: &mut Vec<u64>,
    frames: &mut Vec<Vec<u8>>,
    window: &Rc<MinimalSoftwareWindow>,
    started_ms: u64,
    completed_ms: u64,
    previous_timer: Effect,
    result: Result<Availability, ReadError>,
    on_refresh_started: &mut impl FnMut(Effect, u64),
) -> Result<(Effect, RefreshEvidence), String> {
    trace.push(TraceRecord {
        virtual_time_ms: started_ms,
        kind: TraceKind::RefreshDue,
        effect_id: Some(previous_timer.id.local.get()),
        view: None,
    });
    let read = application
        .transition(ApplicationInput::availability(
            previous_timer.id,
            AvailabilityInput::RefreshDue(RefreshDue {
                effect_id: previous_timer.id.local,
            }),
        ))
        .map_err(|error| format!("refresh due: {error:?}"))?
        .effects
        .get(0)
        .ok_or("refresh due did not request read")?;
    trace.push(effect_trace(started_ms, read));
    on_refresh_started(read, started_ms);
    let refresh_timer = complete_read(
        application,
        ui,
        trace,
        views,
        times,
        frames,
        window,
        completed_ms,
        read,
        result,
    )?;
    let evidence = arm_timer(
        application,
        started_ms,
        completed_ms,
        read,
        refresh_timer,
        result,
    )?;
    Ok((refresh_timer, evidence))
}

#[allow(clippy::too_many_arguments)]
fn complete_multi_feature_tail(
    application: &mut Application,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    composition_records: &mut Vec<CompositionRecord>,
    views: &mut Vec<ViewSetName>,
    times: &mut Vec<u64>,
    frames: &mut Vec<Vec<u8>>,
    refreshes: &mut Vec<RefreshEvidence>,
    window: &Rc<MinimalSoftwareWindow>,
    on_refresh_started: &mut impl FnMut(Effect, u64),
    notice: Effect,
    availability_timer: Effect,
) -> Result<(), String> {
    let expired = application
        .transition(ApplicationInput::synthetic_notice_effect(
            notice.id,
            synthetic_notice::Input::ExpiryDue(synthetic_notice::ExpiryDue {
                effect_id: notice.id.local,
            }),
        ))
        .map_err(|error| format!("notice expiry: {error:?}"))?;
    apply_view(ui, expired.view);
    trace.push(TraceRecord {
        virtual_time_ms: 6_000,
        kind: TraceKind::NoticeExpired,
        effect_id: Some(notice.id.local.get()),
        view: Some(expired.view.into()),
    });
    views.push(expired.view.into());
    times.push(6_000);
    frames.push(render(window)?);
    composition_records.push(CompositionRecord {
        virtual_time_ms: 6_000,
        routed_feature: Some(FeatureName::SyntheticNotice),
        lifecycle: None,
        effect: Some(EffectName::NoticeExpiryDue),
        views: expired.view.into(),
        outcome: TransitionOutcome::Success,
    });

    let stale_notice = show_notice_and_arm(
        application,
        ui,
        trace,
        composition_records,
        views,
        times,
        frames,
        window,
        6_500,
    )?;

    let invalidated = application
        .transition(ApplicationInput::Lifecycle(Lifecycle::SessionInvalidated))
        .map_err(|error| format!("session invalidation: {error:?}"))?;
    if !invalidated.effects.is_empty() {
        return Err("waiting invalidation changed the availability timer".into());
    }
    apply_view(ui, invalidated.view);
    trace.push(TraceRecord {
        virtual_time_ms: 6_750,
        kind: TraceKind::SessionInvalidated,
        effect_id: None,
        view: Some(invalidated.view.into()),
    });
    views.push(invalidated.view.into());
    times.push(6_750);
    frames.push(render(window)?);
    composition_records.push(CompositionRecord {
        virtual_time_ms: 6_750,
        routed_feature: None,
        lifecycle: Some(LifecycleName::SessionInvalidated),
        effect: None,
        views: invalidated.view.into(),
        outcome: TransitionOutcome::Success,
    });

    reject_stale_notice(application, composition_records, stale_notice)?;

    let (_, third_refresh) = complete_refresh_cycle(
        application,
        ui,
        trace,
        views,
        times,
        frames,
        window,
        10_500,
        10_750,
        availability_timer,
        Ok(Availability::Available),
        on_refresh_started,
    )?;
    refreshes.push(third_refresh);
    composition_records.push(CompositionRecord {
        virtual_time_ms: 10_750,
        routed_feature: Some(FeatureName::Availability),
        lifecycle: None,
        effect: Some(EffectName::ReadAvailability),
        views: application.view().into(),
        outcome: TransitionOutcome::Success,
    });
    Ok(())
}

fn reject_stale_notice(
    application: &mut Application,
    composition_records: &mut Vec<CompositionRecord>,
    stale_notice: Effect,
) -> Result<(), String> {
    let before = application.view();
    if application
        .transition(ApplicationInput::synthetic_notice_effect(
            stale_notice.id,
            synthetic_notice::Input::ExpiryDue(synthetic_notice::ExpiryDue {
                effect_id: stale_notice.id.local,
            }),
        ))
        .is_ok()
        || application.view() != before
    {
        return Err("invalidated notice accepted a stale expiry".into());
    }
    composition_records.push(CompositionRecord {
        virtual_time_ms: 8_500,
        routed_feature: Some(FeatureName::SyntheticNotice),
        lifecycle: None,
        effect: Some(EffectName::NoticeExpiryDue),
        views: before.into(),
        outcome: TransitionOutcome::Rejected,
    });
    Ok(())
}

const fn scenario_results(
    scenario: ScenarioName,
) -> (
    Result<Availability, ReadError>,
    Result<Availability, ReadError>,
) {
    match scenario {
        ScenarioName::PeriodicSuccess | ScenarioName::MultiFeatureComposition => {
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
    application: &mut Application,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    views: &mut Vec<ViewSetName>,
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
    let transition = application
        .transition(ApplicationInput::Lifecycle(Lifecycle::SessionInvalidated))
        .map_err(|error| format!("source invalidation: {error:?}"))?;
    if !transition.effects.is_empty() {
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
    application: &mut Application,
    ui: &StatusWindow,
    trace: &mut Vec<TraceRecord>,
    views: &mut Vec<ViewSetName>,
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
        effect_id: Some(read.id.local.get()),
        view: None,
    });
    let transition = application
        .transition(ApplicationInput::availability(
            read.id,
            AvailabilityInput::ReadCompleted(ReadCompleted {
                effect_id: read.id.local,
                result,
            }),
        ))
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
    let timer_effect = transition
        .effects
        .get(0)
        .ok_or("read did not request timer")?;
    trace.push(effect_trace(virtual_time_ms, timer_effect));
    Ok(timer_effect)
}

fn arm_timer(
    application: &mut Application,
    started_ms: u64,
    completed_ms: u64,
    read: Effect,
    timer: Effect,
    read_result: Result<Availability, ReadError>,
) -> Result<RefreshEvidence, String> {
    application
        .transition(ApplicationInput::availability(
            timer.id,
            AvailabilityInput::TimerArmCompleted(TimerArmCompleted {
                effect_id: timer.id.local,
                result: Ok(()),
            }),
        ))
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
            Some(read.id.local.get()),
            completed_ms,
            Some(read_value),
        ),
        diagnostic(
            Operation::CoreTransition,
            Some(read.id.local.get()),
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
            effect_id: Some(timer.id.local.get()),
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
        EffectRequest::Availability(availability::EffectRequest::ReadAvailability) => {
            TraceKind::ReadRequested
        }
        EffectRequest::Availability(availability::EffectRequest::ArmRefreshTimer { .. }) => {
            TraceKind::TimerArmRequested
        }
        EffectRequest::SyntheticNotice(synthetic_notice::EffectRequest::ArmExpiryTimer {
            ..
        }) => TraceKind::NoticeArmRequested,
    };
    TraceRecord {
        virtual_time_ms,
        kind,
        effect_id: Some(effect.id.local.get()),
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
    use crate::{PetWindow, TempCleanup};
    use deskkin_presentation::{PetAnimationState, PetAnimator};

    const PET_FRAME_DIGESTS: [u64; 28] = [
        15_240_192_158_655_750_799,
        5_267_874_411_037_888_994,
        14_562_814_615_917_032_482,
        12_686_121_222_843_204_116,
        6_502_110_196_842_142_708,
        953_132_359_192_451_725,
        3_946_268_473_596_824_701,
        13_925_813_600_998_356_779,
        12_071_782_083_808_155_722,
        12_211_626_761_098_548_091,
        10_516_438_819_187_972_490,
        9_490_301_413_617_770_908,
        10_210_989_452_536_411_571,
        15_210_265_252_676_810_481,
        4_210_867_229_546_232_781,
        1_148_195_780_818_413_162,
        12_667_510_944_439_842_746,
        8_983_621_381_579_333_626,
        17_408_237_654_949_272_261,
        16_158_687_989_093_076_522,
        13_019_057_240_678_865_022,
        16_939_426_122_311_629_613,
        4_297_562_455_252_360_936,
        9_669_883_493_507_821_412,
        10_787_684_014_598_011_639,
        12_008_897_865_911_865_566,
        7_290_273_437_935_586_627,
        8_914_432_661_029_335_868,
    ];

    fn rgb565_digest(pixels: &[u8]) -> u64 {
        pixels.iter().fold(0xcbf2_9ce4_8422_2325, |digest, byte| {
            (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    #[test]
    fn shared_pet_surface_renders_every_normalized_state() {
        let window = setup_headless();
        let ui = PetWindow::new().unwrap();
        ui.show().unwrap();
        let mut animator = PetAnimator::new();
        let mut frame_digests = Vec::new();

        for state in [
            PetAnimationState::Idle,
            PetAnimationState::MoveRight,
            PetAnimationState::MoveLeft,
            PetAnimationState::Attend,
        ] {
            let mut frame = animator.set_state(state);
            for _ in 0..state.frame_count() {
                ui.set_pet_animation_state(i32::from(frame.state.loop_index()));
                ui.set_pet_frame_index(i32::from(frame.index));
                let pixels = render(&window).unwrap();
                assert_eq!(pixels, render(&window).unwrap());
                frame_digests.push(rgb565_digest(&pixels));
                frame = animator.advance(state.frame_period_ms());
            }
        }
        assert_eq!(frame_digests.len(), PET_FRAME_DIGESTS.len());
        for (index, (actual, expected)) in frame_digests
            .iter()
            .zip(PET_FRAME_DIGESTS.iter())
            .enumerate()
        {
            assert_eq!(actual, expected, "Pet frame digest {index}");
        }
        ui.hide().unwrap();
    }

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
                        ViewSetName::UNKNOWN,
                        ViewSetName::AVAILABLE,
                        ViewSetName::UNKNOWN,
                        ViewSetName::AVAILABLE
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
            [
                ViewSetName::UNKNOWN,
                ViewSetName::UNKNOWN,
                ViewSetName::AVAILABLE
            ]
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
    fn multi_feature_scenario_preempts_restores_invalidates_and_reconnects() {
        let window = setup_headless();
        let result = execute_replay(
            ScenarioName::MultiFeatureComposition,
            &window,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(
            result.replay.views,
            [
                ViewSetName::UNKNOWN,
                ViewSetName::AVAILABLE,
                ViewSetName::AVAILABLE_WITH_NOTICE,
                ViewSetName::UNAVAILABLE_WITH_NOTICE,
                ViewSetName::UNAVAILABLE,
                ViewSetName::UNAVAILABLE_WITH_NOTICE,
                ViewSetName::UNKNOWN,
                ViewSetName::AVAILABLE,
            ]
        );
        assert_eq!(result.refreshes.len(), 3);
        assert!(result.replay.composition_records.iter().any(|record| {
            record.views.synthetic_notice == Some(NoticeViewName::CompositionCheck)
                && record.views.availability.is_some()
        }));
        assert!(result.replay.composition_records.iter().any(|record| {
            record.lifecycle == Some(LifecycleName::SessionInvalidated)
                && record.outcome == TransitionOutcome::Success
        }));
        assert!(result.replay.composition_records.iter().any(|record| {
            record.routed_feature == Some(FeatureName::SyntheticNotice)
                && record.outcome == TransitionOutcome::Rejected
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
        let _root_cleanup = TempCleanup::new(&root);
        let recorder = Recorder::at_root(root.clone());
        let mut application = Application::new();
        let read = application
            .transition(ApplicationInput::Lifecycle(Lifecycle::Start))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        let markers: Vec<(String, Effect, u64)> = vec![("refresh-failed".into(), read, 0)];
        let _ = recorder.publish(in_progress_run(
            markers[0].0.clone(),
            "scenario-failed".into(),
            read.id.local.get(),
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
