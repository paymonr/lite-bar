//! ETW present capture (the PresentMon approach): consumes frame-present events from Windows
//! Event Tracing and feeds them to the core `FpsTracker`. Only ETW events are read; no game
//! process is ever opened, hooked or injected into.
//!
//! Provider constants were checked against Intel PresentMon, GameTechDev/PresentMon commit
//! 9db97248df5322a0aa1581d9141d1de6b058085b (PresentData/ETW/Microsoft_Windows_{DXGI,D3D9,DxgKrnl}.h,
//! PresentData/PresentMonTraceSession.cpp, PresentData/PresentMonTraceConsumer.cpp).
//!
//! ferrisetw uses its own `windows` 0.57; no `windows` types cross that boundary here. GUIDs are
//! passed as strings and timestamps as plain integers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferrisetw::native::EvntraceNativeError;
use ferrisetw::parser::Parser;
use ferrisetw::provider::{EventFilter, Provider};
use ferrisetw::trace::{stop_trace_by_name, TraceError, TraceProperties, UserTrace};
use ferrisetw::{EventRecord, SchemaLocator};
use lite_bar_core::fps::{FpsTracker, PresentOrigin};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;

/// Name of our real-time ETW session.
pub const SESSION_NAME: &str = "LiteBar-FPS";

/// `ERROR_ACCESS_DENIED`: starting a session needs admin or Performance Log Users membership.
const ERROR_ACCESS_DENIED: u32 = 5;

/// ETW header timestamps (system time, not raw QPC; see `PresentCapture::start`) are 100 ns ticks.
const TICKS_PER_SEC: f64 = 10_000_000.0;

/// One ETW provider and the events in it that mean "a frame was presented".
#[derive(Clone, Copy, Debug)]
pub struct ProviderSpec {
    /// Human-readable provider name; used in test diagnostics and to identify the table row.
    #[allow(dead_code)]
    pub name: &'static str,
    pub guid: &'static str,
    pub present_event_ids: &'static [u16],
    /// `EnableTraceEx2` MatchAnyKeyword: the OR of the PresentMon keyword masks of the events
    /// above, so the provider emits them on every Windows version.
    pub keywords: u64,
    /// Read the payload `Flags` and skip `DXGI_PRESENT_TEST` presents (DXGI only).
    pub reads_flags: bool,
    pub origin: PresentOrigin,
}

/// DXGI, D3D9 and DxgKrnl present providers.
///
/// - DXGI `Present_Start` (0x2a = 42): every `IDXGISwapChain::Present` of D3D10/11/12 games.
///   Keyword 0x8000000000000002.
/// - D3D9 `Present_Start` (0x1): D3D9 games. Keyword 0x8000000000000002.
/// - DxgKrnl `Present_Info` (0xb8 = 184): emitted at the end of every kernel present
///   (`D3DKMTPresent`) on the presenting thread, whatever the API, so it also covers Vulkan and
///   OpenGL. PresentMon attributes DxgKrnl presents with the event header's process id and its
///   thread id (it creates presents from `hdr.ProcessId` for non-instrumented APIs), so the
///   header pid is the presenting process. Keyword 0x4000000008000001 minus the Performance bit
///   (PresentMon masks it out): 0x8000001 (Present | 0x1) on Windows 11, 0x1 before.
pub const PROVIDERS: [ProviderSpec; 3] = [
    ProviderSpec {
        name: "Microsoft-Windows-DXGI",
        guid: "CA11C036-0102-4A2D-A6AD-F03CFED5D3C9",
        present_event_ids: &[42],
        keywords: 0x8000_0000_0000_0002,
        reads_flags: true,
        origin: PresentOrigin::Api,
    },
    ProviderSpec {
        name: "Microsoft-Windows-D3D9",
        guid: "783ACA0A-790E-4D7F-8451-AA850511C6B9",
        present_event_ids: &[1],
        keywords: 0x8000_0000_0000_0002,
        reads_flags: false,
        origin: PresentOrigin::Api,
    },
    ProviderSpec {
        name: "Microsoft-Windows-DxgKrnl",
        guid: "802EC45A-1E99-4B83-9920-87C98277BA9D",
        present_event_ids: &[184],
        keywords: 0x0800_0001,
        reads_flags: false,
        origin: PresentOrigin::Kernel,
    },
];

#[derive(thiserror::Error, Debug)]
pub enum CaptureError {
    #[error("administrator rights needed")]
    NeedsAdmin,
    #[error("{0}")]
    Other(String),
}

/// Whether the tray should offer "Restart as administrator" after a failed capture start.
pub fn offers_admin(err: &CaptureError, elevated: bool) -> bool {
    match err {
        CaptureError::NeedsAdmin => true,
        CaptureError::Other(_) => !elevated,
    }
}

/// Maps a Win32 error code from starting the trace session.
pub fn map_start_error(win32_code: u32) -> CaptureError {
    if win32_code == ERROR_ACCESS_DENIED {
        CaptureError::NeedsAdmin
    } else {
        CaptureError::Other(format!(
            "could not start ETW session (Win32 error {win32_code})"
        ))
    }
}

/// ferrisetw reports `StartTraceW`/`EnableTraceEx2` failures as `HRESULT_FROM_WIN32(code)`
/// (0x8007xxxx) and `OpenTraceW` failures as the plain code. Normalise both to the Win32 code.
fn win32_code(raw: i32) -> u32 {
    let v = raw as u32;
    if v & 0xFFFF_0000 == 0x8007_0000 {
        v & 0xFFFF
    } else {
        v
    }
}

fn map_trace_error(e: TraceError) -> CaptureError {
    match e {
        TraceError::EtwNativeError(EvntraceNativeError::IoError(io)) => match io.raw_os_error() {
            Some(raw) => match map_start_error(win32_code(raw)) {
                CaptureError::NeedsAdmin => CaptureError::NeedsAdmin,
                CaptureError::Other(_) => CaptureError::Other(io.to_string()),
            },
            None => CaptureError::Other(io.to_string()),
        },
        other => CaptureError::Other(format!("{other:?}")),
    }
}

/// `DXGI_PRESENT_TEST`: a probe (e.g. "am I fullscreen?") that presents nothing. PresentMon
/// ignores it (PresentMonTraceConsumer.cpp `RuntimePresentStart`).
const DXGI_PRESENT_TEST: u32 = 0x1;

/// Whether a DXGI `Present_Start` with payload `flags` is a real frame. `None` means the flags
/// could not be read; fail open and count it rather than drop frames on a parse error.
fn counts_as_frame(flags: Option<u32>) -> bool {
    flags.is_none_or(|f| f & DXGI_PRESENT_TEST == 0)
}

/// What the DXGI `Flags` filter should say in the log, at most once each per process.
#[derive(Debug, PartialEq, Eq)]
enum FlagsLog {
    /// The first time `Flags` could not be read (schema or parse failure).
    Unreadable,
    /// The first time `Flags` was read successfully.
    Readable,
}

static FLAGS_UNREADABLE_LOGGED: AtomicBool = AtomicBool::new(false);
static FLAGS_READABLE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Decides whether this `Flags` result deserves a log line. After each kind has been reported
/// once this is two relaxed atomic ops and no allocation, so it is fine on the ETW callback
/// thread; the one-off log line allocates once.
fn flags_log_event(
    flags: Option<u32>,
    unreadable_logged: &AtomicBool,
    readable_logged: &AtomicBool,
) -> Option<FlagsLog> {
    let (flag, event) = match flags {
        None => (unreadable_logged, FlagsLog::Unreadable),
        Some(_) => (readable_logged, FlagsLog::Readable),
    };
    (!flag.swap(true, Ordering::Relaxed)).then_some(event)
}

/// Logs the first unreadable and the first readable DXGI `Flags` (so a log shows whether the
/// TEST-present filter works), then stays silent and fails open.
fn note_flags_result(flags: Option<u32>) {
    match flags_log_event(flags, &FLAGS_UNREADABLE_LOGGED, &FLAGS_READABLE_LOGGED) {
        Some(FlagsLog::Unreadable) => crate::log::warn(
            "DXGI present flags unreadable (schema or parse failure); counting presents unfiltered",
        ),
        Some(FlagsLog::Readable) => crate::log::info("DXGI present flags readable"),
        None => {}
    }
}

/// Current system time in the event time base: FILETIME seconds, as ETW header timestamps
/// (see `record_present`). Focus changes are stamped with it.
pub fn etw_now_secs() -> f64 {
    // SAFETY: no arguments; returns a plain struct.
    let t = unsafe { GetSystemTimePreciseAsFileTime() };
    let ticks = u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
    ticks as f64 / TICKS_PER_SEC
}

/// Hot path: runs once per present event, at the game's frame rate. Allocation-free, never
/// panics, and recovers a poisoned tracker mutex instead of unwinding into ETW.
fn record_present(
    tracker: &Mutex<FpsTracker>,
    pid: u32,
    origin: PresentOrigin,
    raw_ticks: i64,
    clock: Instant,
) {
    let event_time = raw_ticks as f64 / TICKS_PER_SEC;
    let received_at = clock.elapsed().as_secs_f64();
    tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record_present(pid, origin, event_time, received_at);
}

fn build_provider(
    spec: &ProviderSpec,
    tracker: &Arc<Mutex<FpsTracker>>,
    clock: Instant,
) -> Provider {
    let tracker = Arc::clone(tracker);
    let ids = spec.present_event_ids;
    let origin = spec.origin;
    let reads_flags = spec.reads_flags;
    Provider::by_guid(spec.guid)
        .any(spec.keywords)
        .add_filter(EventFilter::ByEventIds(ids.to_vec()))
        .add_callback(move |record: &EventRecord, locator: &SchemaLocator| {
            // The kernel-side event id filter should already have dropped everything else; this
            // is a cheap guard for systems where that filter is not honoured.
            if !ids.contains(&record.event_id()) {
                return;
            }
            // DXGI only: read `Flags` to skip DXGI_PRESENT_TEST. Hot-path cost: a cached schema
            // lookup (one uncontended mutex + Arc clone after the first event) and a Parser that
            // allocates a little per event, on the ETW consumer thread, never the game's. D3D9 and
            // DxgKrnl skip all of this.
            if reads_flags {
                let flags = locator.event_schema(record).ok().and_then(|schema| {
                    Parser::create(record, &schema)
                        .try_parse::<u32>("Flags")
                        .ok()
                });
                note_flags_result(flags);
                if !counts_as_frame(flags) {
                    return;
                }
            }
            record_present(
                &tracker,
                record.process_id(),
                origin,
                record.raw_timestamp(),
                clock,
            );
        })
        .build()
}

/// A running ETW session feeding present events into an `FpsTracker`. Dropping it stops it.
pub struct PresentCapture {
    trace: UserTrace,
}

impl PresentCapture {
    /// Starts the `LiteBar-FPS` session. `clock` is the app's monotonic clock (the tracker's
    /// `received_at` base).
    pub fn start(
        tracker: Arc<Mutex<FpsTracker>>,
        clock: Instant,
    ) -> Result<PresentCapture, CaptureError> {
        // Clear a stale session left behind by a crash. Failure (none exists, or no rights) is
        // expected and ignored; a real problem surfaces from the start below.
        let _ = stop_trace_by_name(SESSION_NAME);

        // Header timestamps are system time in 100 ns ticks because ferrisetw does not request
        // PROCESS_TRACE_MODE_RAW_TIMESTAMP. A 1 s flush timer caps how stale delivered events get.
        let properties = TraceProperties {
            flush_timer: Duration::from_secs(1),
            ..TraceProperties::default()
        };
        let mut builder = UserTrace::new()
            .named(SESSION_NAME.into())
            .set_trace_properties(properties);
        for spec in &PROVIDERS {
            builder = builder.enable(build_provider(spec, &tracker, clock));
        }
        match builder.start_and_process() {
            Ok(trace) => Ok(PresentCapture { trace }),
            Err(e) => {
                // StartTrace may have succeeded before a later step failed; do not leave the
                // session running with no owner.
                let _ = stop_trace_by_name(SESSION_NAME);
                Err(map_trace_error(e))
            }
        }
    }

    /// Stops the session. Dropping does the same.
    pub fn stop(self) {
        let _ = self.trace.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrisetw::native::EvntraceNativeError;
    use ferrisetw::trace::TraceError;
    use lite_bar_core::fps::{FpsTracker, PresentOrigin};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[test]
    fn needs_admin_always_offers_the_admin_restart() {
        assert!(offers_admin(&CaptureError::NeedsAdmin, false));
        assert!(offers_admin(&CaptureError::NeedsAdmin, true));
    }

    #[test]
    fn other_errors_offer_the_admin_restart_only_when_not_elevated() {
        // A stale session from a crashed elevated run fails a non-elevated start with a code
        // that is not access-denied; restarting elevated can clear it.
        let other = CaptureError::Other("x".into());
        assert!(offers_admin(&other, false));
        assert!(!offers_admin(&other, true));
    }

    #[test]
    fn provider_guids_are_valid() {
        for p in PROVIDERS {
            assert!(windows::core::GUID::try_from(p.guid).is_ok(), "{}", p.name);
            assert!(!p.present_event_ids.is_empty());
        }
    }

    #[test]
    fn capture_error_maps_access_denied() {
        assert!(matches!(map_start_error(5), CaptureError::NeedsAdmin));
        assert!(matches!(map_start_error(183), CaptureError::Other(_)));
    }

    #[test]
    fn start_without_admin_reports_needs_admin_or_succeeds() {
        // Runs in both elevated and non-elevated shells; must never panic or hang.
        let t = Arc::new(Mutex::new(FpsTracker::new()));
        match PresentCapture::start(t, Instant::now()) {
            Ok(c) => {
                assert!(crate::elevation::is_elevated());
                c.stop();
            }
            Err(CaptureError::NeedsAdmin) => assert!(!crate::elevation::is_elevated()),
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    fn io_error(raw: i32) -> TraceError {
        TraceError::EtwNativeError(EvntraceNativeError::IoError(
            std::io::Error::from_raw_os_error(raw),
        ))
    }

    #[test]
    fn trace_error_with_hresult_access_denied_needs_admin() {
        // ferrisetw reports StartTraceW failures as HRESULT_FROM_WIN32(code).
        assert!(matches!(
            map_trace_error(io_error(0x8007_0005_u32 as i32)),
            CaptureError::NeedsAdmin
        ));
    }

    #[test]
    fn trace_error_with_plain_win32_access_denied_needs_admin() {
        // OpenTrace failures carry the plain GetLastError() value.
        assert!(matches!(
            map_trace_error(io_error(5)),
            CaptureError::NeedsAdmin
        ));
    }

    #[test]
    fn other_trace_errors_are_not_needs_admin() {
        assert!(matches!(
            map_trace_error(io_error(0x8007_00B7_u32 as i32)),
            CaptureError::Other(_)
        ));
        assert!(matches!(
            map_trace_error(TraceError::EtwNativeError(
                EvntraceNativeError::AlreadyExist
            )),
            CaptureError::Other(_)
        ));
    }

    #[test]
    fn etw_now_is_system_time_in_the_event_base() {
        // FILETIME counts from 1601-01-01; the Unix epoch is 11_644_473_600 s later.
        const UNIX_EPOCH_AS_FILETIME_SECS: f64 = 11_644_473_600.0;
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let now = etw_now_secs();
        assert!(
            (now - (unix + UNIX_EPOCH_AS_FILETIME_SECS)).abs() < 5.0,
            "etw_now_secs {now}, SystemTime {unix}"
        );
    }

    #[test]
    fn present_ticks_are_100ns_and_feed_the_tracker() {
        let tracker = Mutex::new(FpsTracker::new());
        let clock = Instant::now();
        // Two presents 10 ms apart (100_000 ticks of 100 ns) is 100 fps.
        let base = 133_000_000_000_000_000_i64;
        record_present(&tracker, 42, PresentOrigin::Api, base, clock);
        record_present(&tracker, 42, PresentOrigin::Api, base + 100_000, clock);
        let fps = tracker
            .lock()
            .unwrap()
            .fps(42, clock.elapsed().as_secs_f64())
            .expect("fps");
        assert!((fps - 100.0).abs() < 0.5, "fps = {fps}");
    }

    #[test]
    fn record_present_survives_a_poisoned_tracker_mutex() {
        let tracker = Arc::new(Mutex::new(FpsTracker::new()));
        let t2 = Arc::clone(&tracker);
        let _ = std::thread::spawn(move || {
            let _guard = t2.lock().unwrap();
            panic!("poison the mutex");
        })
        .join();
        assert!(tracker.is_poisoned());
        let clock = Instant::now();
        record_present(&tracker, 7, PresentOrigin::Kernel, 1, clock);
        record_present(&tracker, 7, PresentOrigin::Kernel, 5_000_001, clock);
        let fps = tracker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fps(7, clock.elapsed().as_secs_f64());
        assert!(fps.is_some());
    }

    #[test]
    fn present_test_flag_is_not_a_frame() {
        assert!(!counts_as_frame(Some(DXGI_PRESENT_TEST)));
        assert!(!counts_as_frame(Some(DXGI_PRESENT_TEST | 0x8)));
    }

    #[test]
    fn plain_and_other_flags_are_frames() {
        assert!(counts_as_frame(Some(0)));
        // DO_NOT_SEQUENCE, RESTART, DO_NOT_WAIT, STEREO_*, RESTRICT_TO_OUTPUT: no TEST bit.
        assert!(counts_as_frame(Some(0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40)));
    }

    #[test]
    fn unparsed_flags_fail_open_and_count() {
        assert!(counts_as_frame(None));
    }

    #[test]
    fn first_unreadable_flags_warn_once_then_stay_silent() {
        let (warned, read) = (AtomicBool::new(false), AtomicBool::new(false));
        assert_eq!(
            flags_log_event(None, &warned, &read),
            Some(FlagsLog::Unreadable)
        );
        assert_eq!(flags_log_event(None, &warned, &read), None);
        assert_eq!(flags_log_event(None, &warned, &read), None);
    }

    #[test]
    fn first_readable_flags_log_once_then_stay_silent() {
        let (warned, read) = (AtomicBool::new(false), AtomicBool::new(false));
        assert_eq!(
            flags_log_event(Some(0), &warned, &read),
            Some(FlagsLog::Readable)
        );
        assert_eq!(flags_log_event(Some(0), &warned, &read), None);
        assert_eq!(flags_log_event(Some(1), &warned, &read), None);
    }

    #[test]
    fn unreadable_and_readable_are_tracked_independently() {
        let (warned, read) = (AtomicBool::new(false), AtomicBool::new(false));
        assert_eq!(
            flags_log_event(None, &warned, &read),
            Some(FlagsLog::Unreadable)
        );
        assert_eq!(
            flags_log_event(Some(0), &warned, &read),
            Some(FlagsLog::Readable)
        );
    }

    #[test]
    fn only_dxgi_reads_present_flags() {
        for p in PROVIDERS {
            assert_eq!(
                p.reads_flags,
                p.name == "Microsoft-Windows-DXGI",
                "{}",
                p.name
            );
        }
    }
}
